use crate::config::Config;
use crate::engine::{AudioSink, Engine, PlayerState, SEEK_STEP};
use crate::input::Action;
use crate::ipc::IpcMessage;
use crate::library::{Queue, Track, track_from_file};
use crate::spectrum::EQ_BANDS;
use crate::tui::{Playback, TrackInfo, TrackRow, UiState, View};

use ratatui::text::Line;

const VOLUME_STEP: f32 = 0.05;

/// Toast lifetime (Architecture §2: enqueue toasts live 3s).
const TOAST_DURATION: std::time::Duration = std::time::Duration::from_secs(3);

/// Repeat behavior at end of track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }
}

/// Controller: owns [`Engine`] + [`Queue`], translates [`Action`]s and IPC
/// messages into state changes. Rendering stays in `tui` via [`sync_ui`].
pub struct App<S: AudioSink> {
    engine: Engine<S>,
    queue: Queue,
    config: Config,
    /// Set when the config needs persisting; the UI loop drains it via
    /// [`App::take_dirty_config`] and saves atomically.
    config_dirty: bool,
    view: View,
    prev_view: View,
    search: Option<String>,
    toast: Option<String>,
    toast_at: Option<std::time::Instant>,
    art_enabled: bool,
    eq_enabled: bool,
    repeat: RepeatMode,
    cache_path: std::path::PathBuf,
    /// Memoized cover lines: (track key, lines). Decoding runs only on track change.
    art_memo: std::cell::RefCell<(Option<String>, Vec<Line<'static>>)>,
    /// Background spectrum analysis: worker threads decode+FFT off the UI
    /// thread and post timelines here; [`App::resolve_eq`] drains them.
    spec_tx: std::sync::mpsc::Sender<(u64, Vec<[f32; EQ_BANDS]>)>,
    spec_rx: std::sync::mpsc::Receiver<(u64, Vec<[f32; EQ_BANDS]>)>,
    spec_state: std::cell::RefCell<SpecState>,
}

/// Spectrum cache for the current track.
#[derive(Debug, Default)]
struct SpecState {
    /// Track key the cached timeline belongs to.
    key: Option<u64>,
    /// Track key with analysis in flight (spawned once per track).
    pending: Option<u64>,
    /// Per-100ms band vectors for [`SpecState::key`].
    data: Vec<[f32; EQ_BANDS]>,
}

/// Cache identity shared with cover art: stable id + mtime + size.
fn track_key(track: &Track) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    track.stable_id().hash(&mut h);
    track.mtime_secs().hash(&mut h);
    track.size().hash(&mut h);
    h.finish()
}

impl<S: AudioSink> App<S> {
    pub fn new(
        engine: Engine<S>,
        queue: Queue,
        config: Config,
        cache_path: std::path::PathBuf,
    ) -> Self {
        let eq_enabled = config.eq_enabled();
        let art_enabled = config.art_enabled();
        let (spec_tx, spec_rx) = std::sync::mpsc::channel();
        Self {
            engine,
            queue,
            config,
            config_dirty: false,
            view: View::NowPlaying,
            prev_view: View::NowPlaying,
            search: None,
            toast: None,
            toast_at: None,
            art_enabled,
            eq_enabled,
            repeat: RepeatMode::Off,
            cache_path,
            art_memo: std::cell::RefCell::new((None, Vec::new())),
            spec_tx,
            spec_rx,
            spec_state: std::cell::RefCell::new(SpecState::default()),
        }
    }

    /// Mark the config for persistence on the next UI-loop drain.
    fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
    }

    /// Take the pending config snapshot for saving (resets the dirty flag).
    pub fn take_dirty_config(&mut self) -> Option<Config> {
        if self.config_dirty {
            self.config_dirty = false;
            Some(self.config.clone())
        } else {
            None
        }
    }

    /// Toast with a 3s lifetime (cleared by [`App::tick`]).
    fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(msg.into());
        self.toast_at = Some(std::time::Instant::now());
    }

    /// Handle one action. Returns true when the app should quit.
    pub fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::TogglePlay => self.engine.toggle(),
            Action::SeekForward => self.engine.seek_by(SEEK_STEP, true),
            Action::SeekBackward => self.engine.seek_by(SEEK_STEP, false),
            Action::CursorUp => self.move_cursor(-1),
            Action::CursorDown => self.move_cursor(1),
            Action::PlaySelected => self.play_selected(),
            Action::VolumeUp => self.bump_volume(VOLUME_STEP),
            Action::VolumeDown => self.bump_volume(-VOLUME_STEP),
            Action::ToggleEq => {
                // Toggles the eq preview row now; live spectrum bars stay in v0.2.
                self.eq_enabled = !self.eq_enabled;
                self.config = self.config.clone().with_eq_enabled(self.eq_enabled);
                self.mark_config_dirty();
            }
            Action::ToggleArt => {
                self.art_enabled = !self.art_enabled;
                self.config = self.config.clone().with_art_enabled(self.art_enabled);
                self.mark_config_dirty();
            }
            Action::ToggleHelp => self.toggle_help(),
            Action::CycleView => self.cycle_view(),
            Action::StartSearch => {
                self.search = Some(String::new());
                self.view = View::Queue;
            }
            Action::SearchChar(c) => {
                if let Some(q) = self.search.as_mut() {
                    q.push(c);
                }
                self.snap_cursor_to_filter();
            }
            Action::SearchBackspace => {
                if let Some(q) = self.search.as_mut() {
                    q.pop();
                }
                self.snap_cursor_to_filter();
            }
            Action::ExitSearch => self.search = None,
            Action::ConfirmSearch => {
                // `Enter` on a match plays it immediately — previously `Enter`
                // only left search mode, so selecting music took two Enters
                // and read as "can't select from search".
                if self.filtered_indices().is_empty() {
                    self.set_toast("no matches");
                } else {
                    self.play_selected();
                }
                self.search = None;
            }
            Action::ClearQueue => {
                self.queue.clear();
                self.set_toast("queue cleared");
            }
            Action::ShuffleQueue => {
                let current = self.engine.current().cloned();
                self.queue.shuffle(current.as_ref());
                self.set_toast("queue shuffled");
            }
            Action::CycleRepeat => {
                self.repeat = self.repeat.cycle();
                self.set_toast(format!("repeat: {}", self.repeat.label()));
            }
            Action::Quit => return true,
            Action::Ignored => {}
        }
        false
    }

    /// IPC enqueue: append + toast, never steal the current view (D2).
    /// `EnqueueDir` is handled upstream: the UI loop runs the scan on the
    /// blocking pool and feeds the result back via [`App::enqueue_scanned`],
    /// so a big directory never freezes the interface.
    pub fn handle_ipc(&mut self, msg: &IpcMessage) {
        match msg {
            IpcMessage::Ping => {}
            IpcMessage::EnqueueFiles { paths } => {
                let found: Vec<Track> = paths
                    .iter()
                    .filter_map(|p| track_from_file(p.clone()))
                    .collect();
                let n = found.len();
                self.queue.append(found);
                self.set_toast(if n == 1 {
                    format!(
                        "Queued: {}",
                        paths[0]
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    )
                } else {
                    format!("Queued {n} tracks")
                });
            }
            IpcMessage::EnqueueDir { path } => {
                tracing::warn!(
                    "unexpected sync dir enqueue (ignored; async path handles it): {}",
                    path.display()
                );
            }
        }
    }

    /// Append tracks produced by the background dir scan (IPC enqueue-dir).
    pub fn enqueue_scanned(&mut self, tracks: Vec<Track>) {
        let n = tracks.len();
        self.queue.append(tracks);
        self.set_toast(format!("Queued {n} tracks"));
    }

    /// True while search mode captures typing.
    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// Periodic housekeeping: expire toasts, then end-of-track →
    /// repeat/advance per [`RepeatMode`]. `Off` stops on the last track;
    /// `All` wraps to the head; `One` replays.
    pub fn tick(&mut self) {
        use crate::engine::PlayerState;
        if let Some(at) = self.toast_at
            && at.elapsed() >= TOAST_DURATION
        {
            self.toast = None;
            self.toast_at = None;
        }
        self.engine.poll_end();
        if self.engine.state() != PlayerState::Ended {
            return;
        }
        match self.repeat {
            RepeatMode::One => {
                if let Some(track) = self.engine.current().cloned() {
                    let _ = self.engine.play_track(track);
                }
            }
            RepeatMode::All => {
                let last = self.queue.len().saturating_sub(1);
                if self.queue.selected_index() >= last {
                    self.queue.reset_cursor();
                } else {
                    self.queue.advance();
                }
                if let Some(next) = self.queue.get(self.queue.selected_index()).cloned() {
                    let _ = self.engine.play_track(next);
                }
            }
            RepeatMode::Off => {
                let idx = self.queue.selected_index();
                let last = self.queue.len().saturating_sub(1);
                // Advance only when NOT on the last track; otherwise stay Ended.
                if idx < last
                    && let Some(next) = self.queue.advance().cloned()
                {
                    let _ = self.engine.play_track(next);
                }
            }
        }
    }

    /// Start playback on boot: first playable track wins, unsupported files
    /// are skipped with a toast. No-op on an empty queue.
    pub fn autoplay(&mut self) {
        while self.engine.current().is_none() {
            let idx = self.queue.selected_index();
            let Some(track) = self.queue.get(idx).cloned() else {
                break;
            };
            if let Err(e) = self.engine.play_track(track.clone()) {
                self.set_toast(format!("Skipping {}: {e:#}", track.display_filename()));
                self.queue.move_cursor(1);
                if self.queue.selected_index() == idx {
                    break; // cursor cannot advance: avoid a loop
                }
            }
        }
    }

    /// Build the render snapshot: engine state + (search-filtered) queue.
    pub fn sync_ui(&self) -> UiState {
        let track = self.engine.current().map(|t| TrackInfo {
            title: t.display_title(),
            artist: t.display_artist(),
            album: t.display_album(),
        });
        let playback = match self.engine.state() {
            PlayerState::Playing => Playback::Playing,
            PlayerState::Paused => Playback::Paused,
            _ => Playback::Stopped,
        };
        let (progress, elapsed, total) = match self.engine.current() {
            Some(t) => {
                let total = t.duration_secs();
                let elapsed = self.engine.position().as_secs();
                let progress = total
                    .map(|d| {
                        if d == 0 {
                            0.0
                        } else {
                            elapsed as f32 / d as f32
                        }
                    })
                    .unwrap_or(0.0);
                (progress, elapsed, total)
            }
            None => (0.0, 0, None),
        };
        let names: Vec<TrackRow> = self
            .filtered_indices()
            .into_iter()
            .filter_map(|i| {
                self.queue.get(i).map(|t| TrackRow {
                    title: t.display_title(),
                    artist: t.display_artist(),
                    album: t.display_album(),
                    year: t.display_year(),
                    duration: t.display_duration(),
                    format: t.display_format(),
                    // Queue position id, stable across filtering so a
                    // search row keeps the id it has in the Queue view.
                    qid: format!("q{:02}", i + 1),
                })
            })
            .collect();
        let cursor = self.displayed_cursor();
        UiState {
            view: self.view,
            track,
            playback,
            progress,
            elapsed_secs: elapsed,
            total_secs: total,
            total_tracks: self.queue.len(),
            repeat_label: self.repeat.label().to_string(),
            queue: names,
            cursor,
            toast: self.toast.clone(),
            eq_enabled: self.eq_enabled,
            eq_bars: self.resolve_eq(),
            search_query: self.search.clone(),
            art: self.resolve_art(),
            art_enabled: self.art_enabled,
        }
    }

    /// Cover lines for the current track (memoized per track).
    /// Embedded art → disk cache → 24×12 mosaic; missing art → deterministic
    /// placeholder. Respects the visibility toggle.
    fn resolve_art(&self) -> Vec<Line<'static>> {
        use crate::art::{
            ART_CACHE_CAP_BYTES, MOSAIC_PIXEL_H, MOSAIC_W, evict_over_cap, extract_embedded,
            load_cached, monochrome, placeholder, store_cached, thumbnail,
        };
        let Some(track) = self.engine.current() else {
            return Vec::new();
        };
        if !self.art_enabled {
            return Vec::new();
        }
        let key = format!(
            "{}-{}-{}",
            track.stable_id(),
            track.mtime_secs(),
            track.size()
        );
        if let (Some(k), lines) = &*self.art_memo.borrow()
            && *k == key
        {
            return lines.clone();
        }
        let art_dir = self.cache_path.parent().map(|p| p.join("art"));
        let bytes = art_dir.as_deref().and_then(|dir| {
            load_cached(dir, track.stable_id(), track.mtime_secs(), track.size()).or_else(|| {
                let raw = extract_embedded(track.path())?;
                let _ = store_cached(
                    dir,
                    track.stable_id(),
                    track.mtime_secs(),
                    track.size(),
                    &raw,
                );
                evict_over_cap(dir, ART_CACHE_CAP_BYTES);
                Some(raw)
            })
        });
        // Sample at cell aspect (40x40 px → 40x20 cells): half-blocks render
        // two pixel rows per cell so the cover is square, not stretched.
        let img = bytes
            .as_deref()
            .and_then(|b| thumbnail(b, MOSAIC_W, MOSAIC_PIXEL_H))
            .unwrap_or_else(|| placeholder(track.stable_id(), MOSAIC_W, MOSAIC_PIXEL_H));
        let lines = img.to_lines(!monochrome());
        *self.art_memo.borrow_mut() = (Some(key), lines.clone());
        lines
    }

    /// Live EQ levels for the current playback position.
    /// Analysis runs on a worker thread per track (spawned once); while it
    /// is pending — or disabled, or the file is undecodable — this returns
    /// flat zeros and the UI renders dim idle bars. Never blocks the UI.
    fn resolve_eq(&self) -> [f32; EQ_BANDS] {
        if !self.eq_enabled {
            return [0.0; EQ_BANDS];
        }
        let Some(track) = self.engine.current() else {
            return [0.0; EQ_BANDS];
        };
        let key = track_key(track);
        let mut st = self.spec_state.borrow_mut();
        for (k, timeline) in self.spec_rx.try_iter() {
            if st.pending == Some(k) {
                st.pending = None;
            }
            if k == key {
                st.key = Some(k);
                st.data = timeline;
            }
        }
        if st.key != Some(key) && st.pending != Some(key) {
            st.pending = Some(key);
            let path = track.path().to_path_buf();
            let tx = self.spec_tx.clone();
            std::thread::spawn(move || {
                // Never leave `pending` stuck: a panicking analysis would
                // otherwise wedge this track's EQ flat forever (the UI only
                // clears `pending` on receive).
                let timeline = std::panic::catch_unwind(|| crate::spectrum::analyze(&path))
                    .unwrap_or_default();
                let _ = tx.send((key, timeline));
            });
            return [0.0; EQ_BANDS];
        }
        if st.key == Some(key) && !st.data.is_empty() {
            return crate::spectrum::at_position(&st.data, self.engine.position());
        }
        [0.0; EQ_BANDS]
    }

    /// Queue indices that match the active search filter (all when not filtering).
    /// Matches across title, artist, album, and path per the search design.
    fn filtered_indices(&self) -> Vec<usize> {
        let all: Vec<usize> = (0..self.queue.len()).collect();
        let Some(q) = self.search.as_deref().filter(|q| !q.is_empty()) else {
            return all;
        };
        let needle = q.to_lowercase();
        all.into_iter()
            .filter(|&i| {
                self.queue
                    .get(i)
                    // Raw metadata only: the display fallbacks ("Unknown
                    // artist/album") would match everyday bigrams like "al".
                    .map(|t| {
                        format!(
                            "{} {} {} {}",
                            t.title().unwrap_or_default(),
                            t.artist().unwrap_or_default(),
                            t.album().unwrap_or_default(),
                            t.path().to_string_lossy()
                        )
                        .to_lowercase()
                        .contains(&needle)
                    })
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Cursor row **as rendered**: the position of the selected track inside
    /// the (possibly filtered) list — never an index into a different list.
    fn displayed_cursor(&self) -> usize {
        if self.search.as_deref().is_some_and(|q| !q.is_empty()) {
            self.filtered_indices()
                .iter()
                .position(|&i| i == self.queue.selected_index())
                .unwrap_or(0)
        } else {
            self.queue.selected_index()
        }
    }

    /// Move the cursor one row of the **rendered** list. With an active filter,
    /// this walks the matching subset only (bug fix: the cursor used to step
    /// through the unfiltered queue, so Enter played a track that was not
    /// the one under the marker).
    fn move_cursor(&mut self, delta: i32) {
        if self.search.as_deref().is_some_and(|q| !q.is_empty()) {
            let idxs = self.filtered_indices();
            if idxs.is_empty() {
                return;
            }
            let pos = idxs
                .iter()
                .position(|&i| i == self.queue.selected_index())
                .unwrap_or(0);
            let next = ((pos as i64) + i64::from(delta)).clamp(0, (idxs.len() - 1) as i64) as usize;
            self.queue.set_cursor(idxs[next]);
        } else {
            self.queue.move_cursor(delta);
        }
    }

    /// After a filter edit, keep the selection inside the visible subset.
    fn snap_cursor_to_filter(&mut self) {
        let idxs = self.filtered_indices();
        if !idxs.is_empty() && !idxs.contains(&self.queue.selected_index()) {
            self.queue.set_cursor(idxs[0]);
        }
    }

    fn play_selected(&mut self) {
        let idx = self.queue.selected_index();
        match self.queue.get(idx).cloned() {
            Some(track) => {
                if let Err(e) = self.engine.play_track(track.clone()) {
                    self.set_toast(format!("Cannot play {}: {e:#}", track.display_filename()));
                }
            }
            None => self.set_toast("queue empty"),
        }
    }

    fn bump_volume(&mut self, delta: f32) {
        self.engine.set_volume(self.engine.volume() + delta);
        self.config = self.config.clone().with_volume(self.engine.volume());
        self.mark_config_dirty();
    }

    fn toggle_help(&mut self) {
        if self.view == View::Help {
            self.view = self.prev_view;
        } else {
            self.prev_view = self.view;
            self.view = View::Help;
        }
    }

    /// `Tab` cycles the content views. Without this the Queue view is only
    /// reachable via search — enqueued tracks would be invisible.
    fn cycle_view(&mut self) {
        match self.view {
            View::NowPlaying => self.view = View::Queue,
            View::Queue => self.view = View::NowPlaying,
            View::Help => self.view = self.prev_view,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MockSink;
    use std::time::Duration;

    fn track_named(dir: &std::path::Path, name: &str) -> Track {
        let p = dir.join(name);
        std::fs::write(&p, b"x").expect("touch");
        track_from_file(p).expect("audio").with_test_duration(200)
    }

    fn app() -> (App<MockSink>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "qobi-app-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("tmp");
        let engine = Engine::new(MockSink::with_duration(Duration::from_secs(200)), 1.0);
        let cache = dir.join("cache.json");
        (
            App::new(engine, Queue::default(), Config::default(), cache),
            dir,
        )
    }

    #[test]
    fn toggle_and_seek_drive_engine() {
        let (mut a, dir) = app();
        a.queue.append(vec![track_named(&dir, "s.mp3")]);
        assert!(!a.handle_action(Action::PlaySelected));
        assert!(!a.handle_action(Action::TogglePlay));
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Paused);
        assert!(!a.handle_action(Action::SeekForward));
        assert_eq!(a.engine.position(), Duration::from_secs(5));
        assert!(!a.handle_action(Action::SeekBackward));
        assert_eq!(a.engine.position(), Duration::ZERO);
    }

    #[test]
    fn quit_and_ignored() {
        let (mut a, _) = app();
        assert!(a.handle_action(Action::Quit));
        assert!(!a.handle_action(Action::Ignored));
    }

    #[test]
    fn tab_cycles_content_views_and_leaves_help() {
        let (mut a, _) = app();
        assert_eq!(a.sync_ui().view, View::NowPlaying);
        a.handle_action(Action::CycleView);
        assert_eq!(a.sync_ui().view, View::Queue);
        a.handle_action(Action::CycleView);
        assert_eq!(a.sync_ui().view, View::NowPlaying);
        a.handle_action(Action::ToggleHelp);
        assert_eq!(a.sync_ui().view, View::Help);
        a.handle_action(Action::CycleView);
        assert_eq!(a.sync_ui().view, View::NowPlaying);
    }

    #[test]
    fn bad_track_toasts_and_stays_stopped() {
        let (mut a, dir) = app();
        let t = track_named(&dir, "bad.mp3");
        a.engine
            .sink_mut()
            .fail_paths
            .insert(t.path().to_path_buf());
        a.queue.append(vec![t]);
        a.handle_action(Action::PlaySelected);
        let ui = a.sync_ui();
        assert!(
            ui.toast
                .as_deref()
                .unwrap_or("")
                .starts_with("Cannot play bad.mp3"),
            "toast: {:?}",
            ui.toast
        );
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Stopped);
        assert!(a.engine.current().is_none());
    }

    #[test]
    fn ipc_enqueue_appends_toasts_and_keeps_view() {
        let (mut a, dir) = app();
        a.view = View::Help;
        let f = dir.join("n.mp3");
        std::fs::write(&f, b"x").expect("touch");
        a.handle_ipc(&IpcMessage::EnqueueFiles { paths: vec![f] });
        assert_eq!(a.queue.len(), 1);
        assert_eq!(a.sync_ui().toast.as_deref(), Some("Queued: n.mp3"));
        assert_eq!(a.sync_ui().view, View::Help, "view must not steal");
    }

    #[test]
    fn search_filters_queue_and_exits_cleanly() {
        let (mut a, dir) = app();
        a.queue.append(vec![
            track_named(&dir, "alpha.mp3"),
            track_named(&dir, "beta.flac"),
        ]);
        a.handle_action(Action::StartSearch);
        assert_eq!(a.sync_ui().view, View::Queue);
        a.handle_action(Action::SearchChar('a'));
        a.handle_action(Action::SearchChar('l'));
        let ui = a.sync_ui();
        assert_eq!(ui.search_query.as_deref(), Some("al"));
        assert_eq!(
            ui.queue.iter().map(|r| r.title.clone()).collect::<Vec<_>>(),
            vec!["alpha".to_string()]
        );
        a.handle_action(Action::SearchBackspace);
        assert_eq!(a.sync_ui().search_query.as_deref(), Some("a"));
        a.handle_action(Action::ExitSearch);
        let ui = a.sync_ui();
        assert_eq!(ui.search_query, None);
        assert_eq!(ui.queue.len(), 2);
    }

    #[test]
    fn tick_auto_advances_on_end() {
        let (mut a, dir) = app();
        a.queue.append(vec![
            track_named(&dir, "one.mp3"),
            track_named(&dir, "two.mp3"),
        ]);
        a.handle_action(Action::PlaySelected);
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "one.mp3"
        );
        a.tick(); // still playing: no-op
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "one.mp3"
        );
        a.engine.sink_mut().finish();
        a.tick(); // ended: advance to next
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Playing);
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "two.mp3"
        );
    }

    #[test]
    fn cursor_nav_clamps() {
        let (mut a, dir) = app();
        a.queue
            .append(vec![track_named(&dir, "a.mp3"), track_named(&dir, "b.mp3")]);
        a.handle_action(Action::CursorDown);
        a.handle_action(Action::CursorDown);
        a.handle_action(Action::CursorDown);
        assert_eq!(a.sync_ui().cursor, 1);
        a.handle_action(Action::CursorUp);
        a.handle_action(Action::CursorUp);
        a.handle_action(Action::CursorUp);
        assert_eq!(a.sync_ui().cursor, 0);
    }

    #[test]
    fn playing_track_resolves_placeholder_art() {
        let (mut a, dir) = app();
        assert!(a.sync_ui().art.is_empty(), "no track → no art");
        a.queue.append(vec![track_named(&dir, "s.mp3")]);
        a.handle_action(Action::PlaySelected);
        let ui = a.sync_ui();
        assert_eq!(ui.art.len(), 20, "mosaic rows");
        assert!(ui.art_enabled);
        // Memoized: second sync is identical without re-decoding.
        assert_eq!(a.sync_ui().art, ui.art);
    }

    #[test]
    fn art_toggle_hides_cover() {
        let (mut a, dir) = app();
        a.queue.append(vec![track_named(&dir, "s.mp3")]);
        a.handle_action(Action::PlaySelected);
        assert!(!a.sync_ui().art.is_empty());
        a.handle_action(Action::ToggleArt);
        let ui = a.sync_ui();
        assert!(!ui.art_enabled);
        assert!(ui.art.is_empty());
    }

    #[test]
    fn autoplay_starts_first_and_skips_bad() {
        let (mut a, dir) = app();
        a.autoplay(); // empty queue: no-op
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Stopped);
        let bad = track_named(&dir, "bad.mp3");
        let good = track_named(&dir, "good.mp3");
        a.engine
            .sink_mut()
            .fail_paths
            .insert(bad.path().to_path_buf());
        a.queue.append(vec![bad, good]);
        a.autoplay();
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Playing);
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "good.mp3"
        );
        assert!(
            a.sync_ui()
                .toast
                .as_deref()
                .unwrap_or("")
                .starts_with("Skipping bad.mp3")
        );
    }

    #[test]
    fn clear_empties_queue_but_keeps_playing() {
        let (mut a, dir) = app();
        a.queue
            .append(vec![track_named(&dir, "a.mp3"), track_named(&dir, "b.mp3")]);
        a.handle_action(Action::PlaySelected);
        a.handle_action(Action::ClearQueue);
        assert!(a.queue.is_empty());
        assert_eq!(a.sync_ui().toast.as_deref(), Some("queue cleared"));
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Playing);
    }

    #[test]
    fn shuffle_preserves_set_and_keeps_current_first() {
        let (mut a, dir) = app();
        let names = ["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"];
        for n in names {
            a.queue.append(vec![track_named(&dir, n)]);
        }
        a.handle_action(Action::PlaySelected);
        let current = a.engine.current().cloned().expect("playing");
        a.handle_action(Action::ShuffleQueue);
        let after: Vec<String> = (0..a.queue.len())
            .filter_map(|i| a.queue.get(i).map(|t| t.display_filename()))
            .collect();
        let mut sorted = after.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]);
        assert_eq!(after[0], current.display_filename(), "current stays head");
        assert_eq!(a.sync_ui().toast.as_deref(), Some("queue shuffled"));
    }

    #[test]
    fn volume_and_toggles_mark_config_dirty() {
        let (mut a, _) = app();
        a.handle_action(Action::VolumeDown);
        let cfg = a
            .take_dirty_config()
            .expect("volume change must dirty config");
        assert!((cfg.volume() - 0.95).abs() < 1e-6);
        assert!(a.take_dirty_config().is_none(), "dirty flag must reset");

        a.handle_action(Action::ToggleArt);
        let cfg = a.take_dirty_config().expect("art toggle must dirty config");
        assert!(!cfg.art_enabled());
        assert!(!a.sync_ui().art_enabled);

        a.handle_action(Action::ToggleEq);
        let cfg = a.take_dirty_config().expect("eq toggle must dirty config");
        assert!(!cfg.eq_enabled());
        assert!(
            !a.sync_ui().eq_enabled,
            "eq preview must hide when disabled"
        );
        assert!(
            (a.config.volume() - 0.95).abs() < 1e-6,
            "earlier volume change stays"
        );
    }

    #[test]
    fn toast_expires_after_three_seconds() {
        let (mut a, dir) = app();
        a.queue.append(vec![track_named(&dir, "s.mp3")]);
        a.handle_action(Action::ClearQueue);
        a.queue.append(vec![track_named(&dir, "again.mp3")]);
        a.handle_action(Action::ShuffleQueue);
        assert!(a.sync_ui().toast.is_some());
        // Fast-forward: pretend the toast was set long ago.
        a.toast_at = Some(std::time::Instant::now() - TOAST_DURATION - Duration::from_secs(1));
        a.tick();
        assert!(a.sync_ui().toast.is_none(), "toast must expire");
    }

    #[test]
    fn toast_survives_within_lifetime() {
        let (mut a, _) = app();
        a.handle_action(Action::CycleRepeat);
        a.tick();
        assert_eq!(a.sync_ui().toast.as_deref(), Some("repeat: all"));
    }

    #[test]
    fn search_enter_plays_match_and_exits_search() {
        let (mut a, dir) = app();
        a.queue.append(vec![
            track_named(&dir, "alpha.mp3"),
            track_named(&dir, "beach.flac"),
            track_named(&dir, "beat.wav"),
        ]);
        a.handle_action(Action::StartSearch);
        a.handle_action(Action::SearchChar('b'));
        a.handle_action(Action::SearchChar('e'));
        a.handle_action(Action::CursorDown); // beach -> beat
        assert_eq!(a.sync_ui().cursor, 1);
        a.handle_action(Action::ConfirmSearch);
        let ui = a.sync_ui();
        assert_eq!(ui.search_query, None, "search mode must exit");
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "beat.wav",
            "Enter must play the marked match"
        );
    }

    #[test]
    fn search_enter_on_empty_filter_toasts_and_plays_nothing() {
        let (mut a, dir) = app();
        a.queue.append(vec![track_named(&dir, "alpha.mp3")]);
        a.handle_action(Action::StartSearch);
        a.handle_action(Action::SearchChar('z'));
        a.handle_action(Action::SearchChar('z'));
        assert!(a.sync_ui().queue.is_empty());
        a.handle_action(Action::ConfirmSearch);
        let ui = a.sync_ui();
        assert_eq!(ui.search_query, None);
        assert_eq!(ui.toast.as_deref(), Some("no matches"));
        assert!(a.engine.current().is_none(), "nothing must play");
    }

    #[test]
    fn search_cursor_walks_filtered_list_and_plays_the_marked_track() {
        let (mut a, dir) = app();
        a.queue.append(vec![
            track_named(&dir, "alpha.mp3"),
            track_named(&dir, "beach.flac"),
            track_named(&dir, "beat.wav"),
        ]);
        a.handle_action(Action::StartSearch);
        a.handle_action(Action::SearchChar('b'));
        a.handle_action(Action::SearchChar('e'));
        let ui = a.sync_ui();
        assert_eq!(ui.queue.len(), 2, "filter shows beach+beat");
        assert_eq!(ui.cursor, 0, "snap puts cursor on first match");
        // Cursor moves inside the filtered subset (alpha is hidden).
        a.handle_action(Action::CursorDown);
        assert_eq!(a.sync_ui().cursor, 1);
        a.handle_action(Action::CursorDown);
        assert_eq!(a.sync_ui().cursor, 1, "clamped to filtered end");
        // The track under the marker is what Enter plays.
        a.handle_action(Action::ExitSearch);
        a.handle_action(Action::PlaySelected);
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "beat.wav"
        );
    }

    #[test]
    fn enqueue_scanned_appends_and_toasts() {
        let (mut a, dir) = app();
        let tracks = vec![track_named(&dir, "bg1.mp3"), track_named(&dir, "bg2.mp3")];
        a.enqueue_scanned(tracks);
        assert_eq!(a.queue.len(), 2);
        assert_eq!(a.sync_ui().toast.as_deref(), Some("Queued 2 tracks"));
    }

    #[test]
    fn repeat_off_stops_on_last_track() {
        let (mut a, dir) = app();
        a.queue.append(vec![track_named(&dir, "one.mp3")]);
        a.handle_action(Action::PlaySelected);
        a.engine.sink_mut().finish();
        a.tick();
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Ended);
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "one.mp3"
        );
    }

    #[test]
    fn repeat_all_wraps_and_one_replays() {
        let (mut a, dir) = app();
        a.queue.append(vec![
            track_named(&dir, "one.mp3"),
            track_named(&dir, "two.mp3"),
        ]);
        a.handle_action(Action::PlaySelected);
        // Off (default): advances mid-queue…
        a.engine.sink_mut().finish();
        a.tick();
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "two.mp3"
        );
        // …but stops at the end.
        a.engine.sink_mut().finish();
        a.tick();
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Ended);

        // All: wraps to head.
        a.handle_action(Action::CycleRepeat);
        assert_eq!(a.sync_ui().toast.as_deref(), Some("repeat: all"));
        a.tick();
        assert_eq!(a.engine.state(), crate::engine::PlayerState::Playing);
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "one.mp3"
        );

        // One: replays the same track.
        a.handle_action(Action::CycleRepeat);
        assert_eq!(a.sync_ui().toast.as_deref(), Some("repeat: one"));
        a.engine.sink_mut().finish();
        a.tick();
        assert_eq!(
            a.engine.current().expect("cur").display_filename(),
            "one.mp3"
        );

        // Back to Off.
        a.handle_action(Action::CycleRepeat);
        assert_eq!(a.sync_ui().toast.as_deref(), Some("repeat: off"));
    }
}
