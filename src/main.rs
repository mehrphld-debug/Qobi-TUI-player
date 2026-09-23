use clap::Parser;
use std::path::PathBuf;

use qobi_lib::{
    app::App,
    cli::Target,
    config::Config,
    engine::{Engine, RodioSink},
    error::QobiError,
    input::{Action, map_key, map_search_key},
    ipc::{self, IpcMessage},
    library::{Queue, default_cache_path, save_cache, scan_with_cache, track_from_file},
    tui::render,
};

/// Qobi — local-only TUI music player (Chunk 0: CLI + single-instance guard).
#[derive(Debug, Parser)]
#[command(
    name = "qobi",
    version,
    about = "Beautiful, fast, local-only TUI music player"
)]
struct Cli {
    /// Audio file or music directory. Defaults to the configured music dir.
    target: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("qobi=info")),
        )
        .init();
    match run().await {
        Ok(()) => Ok(()),
        Err(e)
            if matches!(
                e.downcast_ref::<QobiError>(),
                Some(QobiError::StaleInstance(_))
            ) =>
        {
            eprintln!("Error: {e:#}");
            std::process::exit(2);
        }
        Err(e) => Err(e),
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let target = Target::classify(cli.target);
    let sock = ipc::socket_path()?;

    match ipc::resolve_role(&sock).await? {
        ipc::Role::Secondary => become_secondary(&sock, &target).await,
        ipc::Role::Primary(listener) => become_primary(listener, &target).await,
    }
}

/// Second invocation: enqueue into the running instance, never open a window (D2).
async fn become_secondary(sock: &std::path::Path, target: &Target) -> anyhow::Result<()> {
    let msg = match target {
        Target::DefaultDir => {
            println!("Qobi already running");
            return Ok(());
        }
        Target::File(path) => ipc::IpcMessage::EnqueueFiles {
            paths: vec![path.clone()],
        },
        Target::Directory(path) => ipc::IpcMessage::EnqueueDir { path: path.clone() },
    };
    ipc::send_message(sock, &msg).await?;
    match &msg {
        ipc::IpcMessage::EnqueueFiles { paths } => {
            let names: Vec<String> = paths
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect();
            println!("Queued: {}", names.join(", "));
        }
        ipc::IpcMessage::EnqueueDir { path } => {
            println!("Queued directory: {}", path.display());
        }
        ipc::IpcMessage::Ping => {}
    }
    Ok(())
}

/// First instance: seed queue, own playback, run the TUI event loop.
async fn become_primary(listener: tokio::net::UnixListener, target: &Target) -> anyhow::Result<()> {
    let cfg = Config::load().await?;
    let seed_dir = match target {
        Target::DefaultDir => cfg.music_dir().map(std::path::Path::to_path_buf),
        Target::Directory(dir) => Some(dir.clone()),
        Target::File(_) => None,
    }
    .filter(|d| d.is_dir());
    if matches!(target, Target::DefaultDir) && cfg.needs_first_run() && seed_dir.is_none() {
        anyhow::bail!(
            "no music directory configured yet — run `qobi <dir>` or `qobi <file>` first"
        );
    }

    let cache_path = default_cache_path()?;
    let mut queue = Queue::default();
    match target {
        Target::File(path) => {
            let found: Vec<_> = track_from_file(path.clone()).into_iter().collect();
            if found.is_empty() {
                tracing::warn!("unsupported or missing file: {}", path.display());
            }
            queue.append(found);
        }
        Target::Directory(_) | Target::DefaultDir => {
            if let Some(dir) = seed_dir {
                let (tracks, stats) = scan_with_cache(&dir, &cache_path);
                tracing::info!(
                    "scanned {}: {} tracks ({} cache hits, {} skipped)",
                    dir.display(),
                    stats.tracks,
                    stats.cache_hits,
                    stats.skipped_errors
                );
                if let Err(e) = save_cache(&tracks, &cache_path) {
                    tracing::warn!("cache save failed: {e}");
                }
                queue.append(tracks);
            }
        }
    }
    tracing::info!("Qobi primary: {} queued", queue.len());

    let sink = RodioSink::new().map_err(|e| anyhow::anyhow!("no audio output device: {e:#}"))?;
    let engine = Engine::new(sink, cfg.volume());
    let mut app = App::new(engine, queue, cache_path);
    app.autoplay();
    run_tui(app, listener).await
}

/// Live loop: keyboard (blocking reader thread) + IPC socket + 100ms tick.
/// Terminal state is always restored, even on error.
async fn run_tui(
    mut app: App<RodioSink>,
    listener: tokio::net::UnixListener,
) -> anyhow::Result<()> {
    use crossterm::{
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let result = run_loop(&mut app, listener).await;
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    result
}

async fn run_loop(
    app: &mut App<RodioSink>,
    listener: tokio::net::UnixListener,
) -> anyhow::Result<()> {
    use crossterm::event::{Event, KeyEvent};
    use ratatui::{Terminal, backend::CrosstermBackend};

    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel::<KeyEvent>();
    std::thread::spawn(move || {
        while crossterm::event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            match crossterm::event::read() {
                Ok(Event::Key(k)) => {
                    if key_tx.send(k).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let (ipc_tx, mut ipc_rx) = tokio::sync::mpsc::unbounded_channel::<IpcMessage>();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            match ipc::read_message(&mut stream).await {
                Ok(Some(msg)) => {
                    if ipc_tx.send(msg).is_err() {
                        break;
                    }
                }
                Ok(None) => tracing::warn!("ipc: corrupt line ignored"),
                Err(e) => tracing::warn!("ipc read failed: {e:#}"),
            }
        }
    });

    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        terminal.draw(|f| render(f, &app.sync_ui()))?;
        tokio::select! {
            biased;
            Some(k) = key_rx.recv() => {
                let action: Action = if app.is_searching() {
                    map_search_key(k)
                } else {
                    map_key(k)
                };
                if app.handle_action(action) {
                    return Ok(());
                }
            }
            Some(msg) = ipc_rx.recv() => app.handle_ipc(&msg),
            _ = ticker.tick() => app.tick(),
            else => return Ok(()),
        }
    }
}
