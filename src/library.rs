use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cli::is_audio_file;

/// A single scannable audio file. Metadata is best-effort: `None` until enriched.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Track {
    path: PathBuf,
    mtime_secs: u64,
    size: u64,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_secs: Option<u64>,
}

impl Track {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn artist(&self) -> Option<&str> {
        self.artist.as_deref()
    }

    pub fn album(&self) -> Option<&str> {
        self.album.as_deref()
    }

    pub fn duration_secs(&self) -> Option<u64> {
        self.duration_secs
    }

    /// Cache identity: mtime + size (auto-invalidates on change).
    pub fn mtime_secs(&self) -> u64 {
        self.mtime_secs
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Deterministic id from the canonical path string (stable across rescans).
    pub fn stable_id(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.path.to_string_lossy().hash(&mut h);
        h.finish()
    }

    fn probe(path: PathBuf) -> Option<Self> {
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.file_type().is_file() {
            return None;
        }
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Some(Self {
            path,
            mtime_secs,
            size: meta.len(),
            title: None,
            artist: None,
            album: None,
            duration_secs: None,
        })
    }

    /// Best-effort metadata enrichment via `lofty`. Never fails: garbage in → `None` fields.
    pub fn enrich(mut self) -> Self {
        use lofty::prelude::{AudioFile, TaggedFileExt};
        let Ok(tagged) = lofty::read_from_path(&self.path) else {
            return self;
        };
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
        if let Some(tag) = tag {
            use lofty::prelude::Accessor;
            self.title = tag.title().map(|t| t.to_string()).filter(|s| !s.is_empty());
            self.artist = tag
                .artist()
                .map(|t| t.to_string())
                .filter(|s| !s.is_empty());
            self.album = tag.album().map(|t| t.to_string()).filter(|s| !s.is_empty());
        }
        self.duration_secs = Some(tagged.properties().duration().as_secs());
        self
    }

    fn cache_key(&self) -> (u64, u64) {
        (self.mtime_secs, self.size)
    }

    /// Display title: metadata title, else file stem, else raw file name.
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title() {
            return t.to_string();
        }
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.display_filename())
    }

    /// Display artist/album with graceful fallback (never empty on screen).
    pub fn display_artist(&self) -> String {
        self.artist().unwrap_or("Unknown artist").to_string()
    }

    pub fn display_album(&self) -> String {
        self.album().unwrap_or("Unknown album").to_string()
    }

    /// File name for queue rows and toasts.
    pub fn display_filename(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
impl Track {
    /// Fixture helper: pretend metadata enrichment found this duration.
    pub fn with_test_duration(mut self, secs: u64) -> Self {
        self.duration_secs = Some(secs);
        self
    }
}

/// Scan statistics: errors are skipped + counted, never fatal.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanStats {
    pub tracks: usize,
    pub cache_hits: usize,
    pub skipped_errors: usize,
}

/// Sorted recursive scan for audio files. Symlinks to directories are not
/// followed (loop-safe); unreadable entries are skipped + counted.
pub fn scan_dir(dir: &Path) -> (Vec<Track>, ScanStats) {
    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(e) => {
                (e.file_type().is_file() && is_audio_file(e.path())).then(|| e.path().to_path_buf())
            }
            Err(_) => None,
        })
        .collect();
    paths.sort();
    let mut stats = ScanStats::default();
    let tracks: Vec<Track> = paths
        .into_iter()
        .filter_map(|p| match Track::probe(p) {
            Some(t) => {
                stats.tracks += 1;
                Some(t)
            }
            None => {
                stats.skipped_errors += 1;
                None
            }
        })
        .collect();
    (tracks, stats)
}

/// On-disk metadata cache so warm startup skips re-reading unchanged files.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct IndexCache {
    entries: HashMap<PathBuf, Track>,
}

/// Scan with cache: unchanged files (same mtime + size) reuse cached metadata.
pub fn scan_with_cache(dir: &Path, cache_path: &Path) -> (Vec<Track>, ScanStats) {
    let cache: IndexCache = std::fs::read(cache_path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    let (fresh, mut stats) = scan_dir(dir);
    let tracks = fresh
        .into_iter()
        .map(|t| match cache.entries.get(&t.path) {
            Some(cached) if cached.cache_key() == t.cache_key() => {
                stats.cache_hits += 1;
                cached.clone()
            }
            _ => t.enrich(),
        })
        .collect();
    (tracks, stats)
}

/// Persist the scan result for the next warm start (atomic tmp + rename).
pub fn save_cache(tracks: &[Track], cache_path: &Path) -> std::io::Result<()> {
    let cache = IndexCache {
        entries: tracks.iter().map(|t| (t.path.clone(), t.clone())).collect(),
    };
    let raw = serde_json::to_vec(&cache).map_err(std::io::Error::other)?;
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = cache_path.with_extension("json.tmp");
    std::fs::write(&tmp, raw)?;
    std::fs::rename(&tmp, cache_path)?;
    Ok(())
}

/// Probe a single file path (directory enqueue and CLI `<file>` handling).
pub fn track_from_file(path: PathBuf) -> Option<Track> {
    if !is_audio_file(&path) {
        return None;
    }
    Track::probe(path).map(Track::enrich)
}

/// Default on-disk index cache: `~/.config/qobi/cache/index.json`.
pub fn default_cache_path() -> Result<PathBuf, crate::error::QobiError> {
    crate::ipc::qobi_dir().map(|d| d.join("cache").join("index.json"))
}

/// Minimal play queue (Chunk H extends with shuffle/repeat/dedup).
#[derive(Debug, Default)]
pub struct Queue {
    tracks: VecDeque<Track>,
    cursor: usize,
}

impl Queue {
    pub fn append(&mut self, tracks: Vec<Track>) {
        self.tracks.extend(tracks);
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn current(&self) -> Option<&Track> {
        self.tracks.get(self.cursor)
    }

    pub fn advance(&mut self) -> Option<&Track> {
        self.cursor = self
            .cursor
            .saturating_add(1)
            .min(self.tracks.len().saturating_sub(1));
        self.current()
    }

    /// Cursor index for the UI (clamped).
    pub fn selected_index(&self) -> usize {
        self.cursor.min(self.tracks.len().saturating_sub(1))
    }

    /// Drop the cursor onto a specific index (search navigation).
    pub fn set_cursor(&mut self, index: usize) {
        if self.tracks.is_empty() {
            self.cursor = 0;
            return;
        }
        self.cursor = index.min(self.tracks.len() - 1);
    }

    /// Move the cursor by a signed delta, clamped to `[0, len-1]`.
    pub fn move_cursor(&mut self, delta: i32) {
        let len = self.tracks.len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let next = self.cursor as i64 + i64::from(delta);
        self.cursor = next.clamp(0, len as i64 - 1) as usize;
    }

    /// Track at an arbitrary index (selected playback).
    pub fn get(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index)
    }

    /// Back to the head (repeat-all wrap).
    pub fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    /// Fisher–Yates shuffle with a time-seeded xorshift (no extra deps).
    /// When `keep_first` is `Some`, that track stays at position 0 so the
    /// currently playing song is not yanked away. Cursor resets to 0.
    pub fn shuffle(&mut self, keep_first: Option<&Track>) {
        let mut order: Vec<usize> = (0..self.tracks.len()).collect();
        let mut rng = XorShift::seeded();
        for i in (1..order.len()).rev() {
            let j = (rng.next() as usize) % (i + 1);
            order.swap(i, j);
        }
        let sorted: Vec<Track> = order
            .into_iter()
            .filter_map(|i| self.tracks.get(i).cloned())
            .collect();
        self.tracks = sorted.into();
        if let Some(keep) = keep_first
            && let Some(pos) = self.tracks.iter().position(|t| t == keep)
        {
            self.tracks.swap(0, pos);
        }
        self.cursor = 0;
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.cursor = 0;
    }
}

/// Minimal time-seeded xorshift64* (queue shuffling only — not crypto).
struct XorShift(u64);

impl XorShift {
    fn seeded() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15);
        Self(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qobi-lib-{name}-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        dir
    }

    fn touch(dir: &Path, rel: &str) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("parents");
        }
        std::fs::write(&p, b"x").expect("touch");
        p
    }

    #[test]
    fn empty_dir_scans_empty() {
        let dir = tmp_dir("empty");
        let (tracks, stats) = scan_dir(&dir);
        assert!(tracks.is_empty());
        assert_eq!(stats.tracks, 0);
    }

    #[test]
    fn nested_scan_is_sorted_and_audio_only() {
        let dir = tmp_dir("nested");
        touch(&dir, "b.mp3");
        touch(&dir, "a.flac");
        touch(&dir, "sub/c.OGG");
        touch(&dir, "notes.txt");
        touch(&dir, "cover.jpg");
        let (tracks, stats) = scan_dir(&dir);
        let names: Vec<String> = tracks
            .iter()
            .map(|t| {
                t.path()
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["a.flac", "b.mp3", "c.OGG"]);
        assert_eq!(stats.tracks, 3);
    }

    #[test]
    fn dir_symlink_is_not_followed() {
        let dir = tmp_dir("symlink");
        touch(&dir, "real/song.mp3");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).expect("symlink");
        let (tracks, _) = scan_dir(&dir);
        assert_eq!(tracks.len(), 1, "symlinked dir must not double-scan");
    }

    #[test]
    fn corrupt_cache_falls_back_to_fresh_scan() {
        let dir = tmp_dir("badcache");
        touch(&dir, "s.mp3");
        let cache = dir.join("cache").join("index.json");
        std::fs::create_dir_all(cache.parent().expect("parent")).expect("mkdir");
        std::fs::write(&cache, b"{not json").expect("write");
        let (tracks, _) = scan_with_cache(&dir, &cache);
        assert_eq!(tracks.len(), 1);
    }

    #[test]
    fn second_scan_hits_cache() {
        let dir = tmp_dir("cachehit");
        touch(&dir, "one.mp3");
        touch(&dir, "two.flac");
        let cache = dir.join("cache").join("index.json");
        let (first, _) = scan_with_cache(&dir, &cache);
        save_cache(&first, &cache).expect("save");
        let (second, stats) = scan_with_cache(&dir, &cache);
        assert_eq!(second.len(), 2);
        assert_eq!(stats.cache_hits, 2);
    }

    #[test]
    fn changed_file_misses_cache() {
        let dir = tmp_dir("cachemiss");
        let p = touch(&dir, "song.mp3");
        let cache = dir.join("cache").join("index.json");
        let (first, _) = scan_with_cache(&dir, &cache);
        save_cache(&first, &cache).expect("save");
        std::fs::write(&p, b"bigger-content-here").expect("rewrite");
        let (_, stats) = scan_with_cache(&dir, &cache);
        assert_eq!(stats.cache_hits, 0);
    }

    #[test]
    fn enrich_garbage_file_never_panics() {
        let dir = tmp_dir("garbage");
        let p = touch(&dir, "fake.mp3");
        let track = Track::probe(p).expect("probe").enrich();
        assert!(track.title().is_none());
        let id = track.stable_id();
        assert_eq!(id, track.stable_id(), "stable id must be deterministic");
    }

    #[test]
    fn queue_append_current_advance_clear() {
        let dir = tmp_dir("queue");
        let a = touch(&dir, "a.mp3");
        let b = touch(&dir, "b.mp3");
        let mk = |p: PathBuf| Track::probe(p).expect("probe");
        let mut q = Queue::default();
        assert!(q.is_empty());
        q.append(vec![mk(a), mk(b)]);
        assert_eq!(q.len(), 2);
        assert!(q.current().expect("cur").path().ends_with("a.mp3"));
        assert!(q.advance().expect("next").path().ends_with("b.mp3"));
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn thousand_file_scan_stays_fast() {
        let dir = tmp_dir("bench1k");
        for i in 0..1000 {
            touch(&dir, &format!("t{i:04}.mp3"));
        }
        let start = SystemTime::now();
        let (tracks, _) = scan_dir(&dir);
        let elapsed = start.elapsed().expect("clock");
        assert_eq!(tracks.len(), 1000);
        assert!(
            elapsed.as_secs() < 5,
            "1k-file cold scan took {elapsed:?}, budget 5s"
        );
    }
}
