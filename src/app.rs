use crate::engine::{AudioSink, Engine, SEEK_STEP};
use crate::input::Action;
use crate::ipc::IpcMessage;
use crate::library::{Queue, Track, save_cache, scan_with_cache, track_from_file};
use crate::tui::{TrackInfo, UiState, View};

use ratatui::text::Line;

const VOLUME_STEP: f32 = 0.05;

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

    fn label(self) -> &'static str {
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
    view: View,
    prev_view: View,
    search: Option<String>,
    toast: Option<String>,
    art_enabled: bool,
    repeat: RepeatMode,
    cache_path: std::path::PathBuf,
    /// Memoized cover lines: (track key, lines). Decoding runs only on track change.
    art_memo: std::cell::RefCell<(Option<String>, Vec<Line<'static>>)>,
}

impl<S: AudioSink> App<S> {
    pub fn new(engine: Engine<S>, queue: Queue, cache_path: std::path::PathBuf) -> Self {
        Self {
            engine,
            queue,
            view: View::NowPlaying,
            prev_view: View::NowPlaying,
            search: None,
            toast: None,
            art_enabled: true,
            repeat: RepeatMode::Off,
            cache_path,
            art_memo: std::cell::RefCell::new((None, Vec::new())),
        }
    }

    /// Handle one action. Returns true when the app should quit.
    pub fn handle_action(&mut self, action: Action) -> bool {
        match action {
            Action::TogglePlay => self.engine.toggle(),
            Action::SeekForward => self.engine.seek_by(SEEK_STEP, true),
            Action::SeekBackward => self.engine.seek_by(SEEK_STEP, false),
            Action::CursorUp => self.queue.move_cursor(-1),
            Action::CursorDown => self.queue.move_cursor(1),
            Action::PlaySelected => self.play_selected(),
            Action::VolumeUp => self.bump_volume(VOLUME_STEP),
            Action::VolumeDown => self.bump_volume(-VOLUME_STEP),
            Action::ToggleEq => {
                // Persisted in Chunk I; ack only.
                self.toast = Some("eq toggle lands in Chunk I".to_string());
            }
            Action::ToggleArt => {
                self.art_enabled = !self.art_enabled;
            }
            Action::ToggleHelp => self.toggle_help(),
            Action::StartSearch => {
                self.search = Some(String::new());
                self.view = View::Queue;
            }
            Action::SearchChar(c) => {
                if let Some(q) = self.search.as_mut() {
                    q.push(c);
                }
            }
            Action::SearchBackspace => {
                if let Some(q) = self.search.as_mut() {
                    q.pop();
                }
            }
            Action::ExitSearch => self.search = None,
            Action::ClearQueue => {
                self.queue.clear();
                self.toast = Some("queue cleared".to_string());
            }
            Action::ShuffleQueue => {
                let current = self.engine.current().cloned();
                self.queue.shuffle(current.as_ref());
                self.toast = Some("queue shuffled".to_string());
            }
            Action::CycleRepeat => {
                self.repeat = self.repeat.cycle();
                self.toast = Some(format!("repeat: {}", self.repeat.label()));
            }
            Action::Quit => return true,
            Action::Ignored => {}
        }
        false
    }

    /// IPC enqueue: append + toast, never steal the current view (D2).
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
                self.toast = Some(if n == 1 {
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
                let (tracks, _) = scan_with_cache(path, &self.cache_path);
                let n = tracks.len();
                if let Err(e) = save_cache(&tracks, &self.cache_path) {
                    tracing::warn!("cache save failed: {e}");
                }
                self.queue.append(tracks);
                self.toast = Some(format!("Queued {n} tracks"));
            }
        }
    }

    /// True while search mode captures typing.
    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// Periodic housekeeping: end-of-track → repeat/advance per [`RepeatMode`].
    /// `Off` stops on the last track; `All` wraps to the head; `One` replays.
    pub fn tick(&mut self) {
        use crate::engine::PlayerState;
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
                self.toast = Some(format!("Skipping {}: {e:#}", track.display_filename()));
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
        let names: Vec<String> = self
            .filtered_tracks()
            .iter()
            .map(|t| t.display_filename())
            .collect();
        let cursor = self
            .queue
            .selected_index()
            .min(names.len().saturating_sub(1));
        UiState {
            view: self.view,
            track,
            progress,
            elapsed_secs: elapsed,
            total_secs: total,
            queue: names,
            cursor,
            toast: self.toast.clone(),
            eq_enabled: true,
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
            ART_CACHE_CAP_BYTES, MOSAIC_H, MOSAIC_W, evict_over_cap, extract_embedded, load_cached,
            monochrome, placeholder, store_cached, thumbnail,
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
        let img = bytes
            .as_deref()
            .and_then(|b| thumbnail(b, MOSAIC_W, MOSAIC_H))
            .unwrap_or_else(|| placeholder(track.stable_id(), MOSAIC_W, MOSAIC_H));
        let lines = img.to_lines(!monochrome());
        *self.art_memo.borrow_mut() = (Some(key), lines.clone());
        lines
    }

    fn filtered_tracks(&self) -> Vec<&Track> {
        let all: Vec<&Track> = (0..self.queue.len())
            .filter_map(|i| self.queue.get(i))
            .collect();
        let Some(q) = self.search.as_deref().filter(|q| !q.is_empty()) else {
            return all;
        };
        let needle = q.to_lowercase();
        all.into_iter()
            .filter(|t| t.display_filename().to_lowercase().contains(&needle))
            .collect()
    }

    fn play_selected(&mut self) {
        let idx = self.queue.selected_index();
        match self.queue.get(idx).cloned() {
            Some(track) => {
                if let Err(e) = self.engine.play_track(track.clone()) {
                    self.toast = Some(format!("Cannot play {}: {e:#}", track.display_filename()));
                }
            }
            None => self.toast = Some("queue empty".to_string()),
        }
    }

    fn bump_volume(&mut self, delta: f32) {
        self.engine.set_volume(self.engine.volume() + delta);
    }

    fn toggle_help(&mut self) {
        if self.view == View::Help {
            self.view = self.prev_view;
        } else {
            self.prev_view = self.view;
            self.view = View::Help;
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
        (App::new(engine, Queue::default(), cache), dir)
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
        assert_eq!(ui.queue, vec!["alpha.mp3".to_string()]);
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
        assert_eq!(ui.art.len(), 12, "mosaic rows");
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
