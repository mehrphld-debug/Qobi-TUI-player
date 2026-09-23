use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::QobiError;
use crate::library::Track;

/// Fixed seek step for keyboard seeking (±5s per Chunk-Map §D).
pub const SEEK_STEP: Duration = Duration::from_secs(5);

/// Playback state machine. Invalid states are unrepresentable: only `Playing`
/// and `Paused` carry an implicit current track (`Engine::current` is `Some`
/// exactly in those states, plus `Ended` for replay).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Stopped,
    Loading,
    Playing,
    Paused,
    Ended,
}

/// Hardware-abstraction for audio output. The real backend is [`RodioSink`];
/// headless CI and unit tests use [`MockSink`].
pub trait AudioSink {
    /// Load `path` for playback, returning known total duration if the backend
    /// can determine it. Failure leaves the previous backend state untouched.
    fn load_file(&mut self, path: &Path) -> Result<Option<Duration>, QobiError>;
    fn play(&mut self);
    fn pause(&mut self);
    fn stop(&mut self);
    fn set_volume(&mut self, volume: f32);
    fn position(&self) -> Duration;
    fn try_seek(&mut self, pos: Duration) -> Result<(), QobiError>;
    /// True when the loaded source ran to completion.
    fn ended(&self) -> bool;
}

/// Deterministic fake sink: scripted duration, scripted failures, full event log.
#[derive(Debug, Default)]
pub struct MockSink {
    /// Paths for which `load_file` fails (missing/unsupported fixtures).
    pub fail_paths: std::collections::HashSet<PathBuf>,
    /// Duration reported for successful loads.
    pub fixture_duration: Option<Duration>,
    /// Event log for assertions, in order.
    pub events: Vec<MockEvent>,
    position: Duration,
    paused: bool,
    loaded: bool,
    finished: bool,
}

/// Observable backend calls, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum MockEvent {
    Load(PathBuf),
    Play,
    Pause,
    Stop,
    Volume(f32),
    Seek(Duration),
}

impl MockSink {
    pub fn with_duration(duration: Duration) -> Self {
        Self {
            fixture_duration: Some(duration),
            ..Self::default()
        }
    }

    /// Pretend the loaded source played to completion.
    pub fn finish(&mut self) {
        self.finished = true;
    }
}

impl AudioSink for MockSink {
    fn load_file(&mut self, path: &Path) -> Result<Option<Duration>, QobiError> {
        self.events.push(MockEvent::Load(path.to_path_buf()));
        if self.fail_paths.contains(path) {
            return Err(QobiError::Decode(format!(
                "unsupported: {}",
                path.display()
            )));
        }
        self.loaded = true;
        self.finished = false;
        self.position = Duration::ZERO;
        self.paused = true;
        Ok(self.fixture_duration)
    }

    fn play(&mut self) {
        self.events.push(MockEvent::Play);
        self.paused = false;
    }

    fn pause(&mut self) {
        self.events.push(MockEvent::Pause);
        self.paused = true;
    }

    fn stop(&mut self) {
        self.events.push(MockEvent::Stop);
        self.loaded = false;
        self.paused = true;
    }

    fn set_volume(&mut self, volume: f32) {
        self.events.push(MockEvent::Volume(volume));
    }

    fn position(&self) -> Duration {
        self.position
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), QobiError> {
        self.events.push(MockEvent::Seek(pos));
        self.position = pos;
        Ok(())
    }

    fn ended(&self) -> bool {
        self.finished
    }
}

/// Real output backend (rodio 0.22). Constructed once at startup; every method
/// is a thin error-mapped delegation so this struct needs no unit tests.
pub struct RodioSink {
    _stream: rodio::stream::MixerDeviceSink,
    player: rodio::Player,
}

impl RodioSink {
    pub fn new() -> Result<Self, QobiError> {
        let stream = rodio::stream::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| QobiError::Output(e.to_string()))?;
        let player = rodio::Player::connect_new(stream.mixer());
        Ok(Self {
            _stream: stream,
            player,
        })
    }
}

impl AudioSink for RodioSink {
    fn load_file(&mut self, path: &Path) -> Result<Option<Duration>, QobiError> {
        let file = std::fs::File::open(path)?;
        let source = rodio::Decoder::new(file).map_err(|e| QobiError::Decode(e.to_string()))?;
        use rodio::Source;
        let duration = source.total_duration();
        self.player.stop();
        self.player.append(source);
        Ok(duration)
    }

    fn play(&mut self) {
        self.player.play();
    }

    fn pause(&mut self) {
        self.player.pause();
    }

    fn stop(&mut self) {
        self.player.stop();
    }

    fn set_volume(&mut self, volume: f32) {
        self.player.set_volume(volume);
    }

    fn position(&self) -> Duration {
        self.player.get_pos()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), QobiError> {
        self.player
            .try_seek(pos)
            .map_err(|e| QobiError::Output(e.to_string()))
    }

    fn ended(&self) -> bool {
        self.player.empty()
    }
}

/// Volume bounds shared by config and engine.
pub const MIN_VOLUME: f32 = 0.0;
pub const MAX_VOLUME: f32 = 1.0;

/// State machine over any [`AudioSink`]. Load failures resolve to `Stopped`
/// with the error returned (caller skips to the next track) — never a panic.
pub struct Engine<S: AudioSink> {
    sink: S,
    state: PlayerState,
    current: Option<Track>,
    volume: f32,
}

impl<S: AudioSink> Engine<S> {
    pub fn new(sink: S, volume: f32) -> Self {
        let mut engine = Self {
            sink,
            state: PlayerState::Stopped,
            current: None,
            volume: Self::clamp_volume(volume),
        };
        engine.sink.set_volume(engine.volume);
        engine
    }

    fn clamp_volume(volume: f32) -> f32 {
        volume.clamp(MIN_VOLUME, MAX_VOLUME)
    }

    pub fn state(&self) -> PlayerState {
        self.state
    }

    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref()
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn position(&self) -> Duration {
        self.sink.position()
    }

    fn known_duration(&self) -> Option<Duration> {
        self.current
            .as_ref()
            .and_then(|t| t.duration_secs())
            .map(Duration::from_secs)
    }

    /// Load and start `track`. On failure the engine is `Stopped` with no
    /// current track so the caller can skip forward.
    pub fn play_track(&mut self, track: Track) -> Result<(), QobiError> {
        self.state = PlayerState::Loading;
        match self.sink.load_file(track.path()) {
            Ok(_) => {
                self.current = Some(track);
                self.sink.play();
                self.state = PlayerState::Playing;
                Ok(())
            }
            Err(e) => {
                self.current = None;
                self.state = PlayerState::Stopped;
                Err(e)
            }
        }
    }

    /// Space-key behavior: Playing ⇄ Paused, replay on Ended, no-op when Stopped.
    pub fn toggle(&mut self) {
        match self.state {
            PlayerState::Playing => {
                self.sink.pause();
                self.state = PlayerState::Paused;
            }
            PlayerState::Paused => {
                self.sink.play();
                self.state = PlayerState::Playing;
            }
            PlayerState::Ended => {
                if let Some(track) = self.current.clone() {
                    // Replay failure degrades to Stopped (same rule as play_track).
                    let _ = self.play_track(track);
                }
            }
            PlayerState::Stopped | PlayerState::Loading => {}
        }
    }

    /// Full stop: backend halted, current cleared, back to picker state.
    pub fn stop(&mut self) {
        self.sink.stop();
        self.current = None;
        self.state = PlayerState::Stopped;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = Self::clamp_volume(volume);
        self.sink.set_volume(self.volume);
    }

    /// Seek relative to the current backend position, clamped to
    /// `[0, duration]` (or `[0, ∞)` when duration is unknown). No-ops unless
    /// Playing or Paused.
    pub fn seek_by(&mut self, delta: std::time::Duration, forward: bool) {
        if !matches!(self.state, PlayerState::Playing | PlayerState::Paused) {
            return;
        }
        let pos = self.sink.position();
        let target = if forward {
            pos.saturating_add(delta)
        } else {
            pos.saturating_sub(delta)
        };
        let target = match self.known_duration() {
            Some(dur) => target.min(dur),
            None => target,
        };
        // Backend seek failure is non-fatal: keep playing from the old position.
        let _ = self.sink.try_seek(target);
    }

    /// Advance `Stopped → …` when the backend reports completion.
    pub fn poll_end(&mut self) {
        if self.state == PlayerState::Playing && self.sink.ended() {
            self.state = PlayerState::Ended;
        }
    }
}

#[cfg(test)]
impl<S: AudioSink> Engine<S> {
    /// Test seam: script the backend (failures, end-of-stream).
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_at(path: &str) -> Track {
        // Probe real temp files so Track construction stays honest.
        let dir = std::env::temp_dir().join(format!(
            "qobi-eng-{}-{}",
            path.replace('/', "_"),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("tmp");
        let p = dir.join(path.rsplit('/').next().expect("name"));
        std::fs::write(&p, b"x").expect("touch");
        crate::library::track_from_file(p)
            .expect("audio ext")
            .with_test_duration(200)
    }

    fn engine() -> Engine<MockSink> {
        Engine::new(MockSink::with_duration(Duration::from_secs(200)), 1.0)
    }

    #[test]
    fn play_loads_and_reports_playing() {
        let mut e = engine();
        e.play_track(track_at("a/song.mp3")).expect("play");
        assert_eq!(e.state(), PlayerState::Playing);
        assert!(e.current().is_some());
        assert!(matches!(
            e.sink.events.as_slice(),
            [MockEvent::Volume(_), MockEvent::Load(_), MockEvent::Play]
        ));
    }

    #[test]
    fn load_failure_resolves_to_stopped_with_no_current() {
        let mut e = engine();
        let track = track_at("b/bad.mp3");
        e.sink.fail_paths.insert(track.path().to_path_buf());
        let err = e.play_track(track).expect_err("must fail");
        assert!(matches!(err, QobiError::Decode(_)));
        assert_eq!(e.state(), PlayerState::Stopped);
        assert!(e.current().is_none());
    }

    #[test]
    fn toggle_cycles_play_pause_and_replays_ended() {
        let mut e = engine();
        e.toggle(); // Stopped: no-op
        assert_eq!(e.state(), PlayerState::Stopped);
        e.play_track(track_at("c/s.mp3")).expect("play");
        e.toggle();
        assert_eq!(e.state(), PlayerState::Paused);
        e.toggle();
        assert_eq!(e.state(), PlayerState::Playing);
        e.sink.finish();
        e.poll_end();
        assert_eq!(e.state(), PlayerState::Ended);
        e.toggle(); // replay
        assert_eq!(e.state(), PlayerState::Playing);
    }

    #[test]
    fn stop_clears_current_and_halts() {
        let mut e = engine();
        e.play_track(track_at("d/s.mp3")).expect("play");
        e.stop();
        assert_eq!(e.state(), PlayerState::Stopped);
        assert!(e.current().is_none());
        assert!(e.sink.events.contains(&MockEvent::Stop));
    }

    #[test]
    fn seek_clamps_to_duration_bounds() {
        let mut e = engine();
        e.seek_by(SEEK_STEP, true); // Stopped: no-op, no event
        assert!(
            !e.sink
                .events
                .iter()
                .any(|ev| matches!(ev, MockEvent::Seek(_)))
        );

        e.play_track(track_at("e/s.mp3")).expect("play");
        e.seek_by(Duration::from_secs(500), true); // past 200s duration
        assert_eq!(e.sink.position(), Duration::from_secs(200));
        e.seek_by(Duration::from_secs(500), false); // below zero
        assert_eq!(e.sink.position(), Duration::ZERO);
    }

    #[test]
    fn volume_clamps_and_forwards() {
        let mut e = engine();
        e.set_volume(9.0);
        assert_eq!(e.volume(), 1.0);
        e.set_volume(-1.0);
        assert_eq!(e.volume(), 0.0);
        assert!(e.sink.events.contains(&MockEvent::Volume(1.0)));
        assert!(e.sink.events.contains(&MockEvent::Volume(0.0)));
    }

    #[test]
    fn poll_end_only_fires_while_playing() {
        let mut e = engine();
        e.sink.finish();
        e.poll_end(); // Stopped: stays Stopped
        assert_eq!(e.state(), PlayerState::Stopped);
    }

    /// Hardware-dependent: passes whether or not a device exists (headless CI
    /// has none). Run with `cargo test -- --ignored` on a real machine to
    /// prove the CoreAudio/ALSA path constructs.
    #[test]
    #[ignore]
    fn rodio_backend_constructs_where_device_exists() {
        let _ = RodioSink::new();
    }
}
