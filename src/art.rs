use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Disk cache budget for extracted covers (Chunk-Map §F).
pub const ART_CACHE_CAP_BYTES: u64 = 500 * 1024 * 1024;

/// Mosaic size in terminal cells (1 cell = 1 pixel, full-block fg only).
pub const MOSAIC_W: u32 = 24;
pub const MOSAIC_H: u32 = 12;

/// Image-capable terminal protocols, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtProtocol {
    Kitty,
    Iterm2,
    Sixel,
    /// Universal fallback: colored block mosaic (fg only, bg stays `Reset`).
    Blocks,
}

/// Detect the protocol from an env snapshot (pure — testable without a terminal).
/// `NO_COLOR` always wins and forces [`ArtProtocol::Blocks`] in monochrome.
pub fn detect_protocol_from(env: &HashMap<String, String>) -> ArtProtocol {
    let get = |k: &str| env.get(k).map(String::as_str).unwrap_or("");
    if env.contains_key("NO_COLOR") {
        return ArtProtocol::Blocks;
    }
    if env.contains_key("KITTY_WINDOW_ID") || get("TERM") == "xterm-kitty" {
        return ArtProtocol::Kitty;
    }
    if get("TERM_PROGRAM") == "iTerm.app" {
        return ArtProtocol::Iterm2;
    }
    if get("TERM") == "foot" || get("TERM") == "foot-extra" {
        return ArtProtocol::Sixel;
    }
    ArtProtocol::Blocks
}

/// Detect from the real process environment.
pub fn detect_protocol() -> ArtProtocol {
    detect_protocol_from(&std::env::vars().collect())
}

/// True when colors must be suppressed (`NO_COLOR` present, any value).
pub fn monochrome() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

/// First embedded picture bytes for `path`, if the container has one.
/// Garbage/unknown files yield `None` — never an error.
pub fn extract_embedded(path: &Path) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    use lofty::prelude::TaggedFileExt;
    tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())?
        .pictures()
        .first()
        .map(|pic| pic.data().to_vec())
}

/// Decoded RGB thumbnail, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<(u8, u8, u8)>,
}

/// Decode + thumbnail any `image`-supported bytes (png/jpeg/webp/bmp/…).
/// Returns `None` for undecodable input.
pub fn thumbnail(raw: &[u8], width: u32, height: u32) -> Option<ArtImage> {
    let img = image::load_from_memory(raw).ok()?;
    let thumb = img.thumbnail(width, height).to_rgb8();
    let (w, h) = (thumb.width(), thumb.height());
    let pixels = thumb.pixels().map(|p| (p[0], p[1], p[2])).collect();
    Some(ArtImage {
        width: w,
        height: h,
        pixels,
    })
}

/// Deterministic placeholder cover from a seed (track id or 0).
/// Flat single hue + bit-pattern mask — never a gradient, never the
/// purple-blue AI default.
pub fn placeholder(seed: u64, width: u32, height: u32) -> ArtImage {
    let hue_idx = placeholder_color_idx(seed);
    let (r, g, b) = ansi256_to_rgb(hue_idx);
    // Second tone: same hue, darkened — pattern from seed bits (waves).
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let bit = (seed >> ((x + y * 3) % 61)) & 1 == 1;
                if bit {
                    (r, g, b)
                } else {
                    (r / 3, g / 3, b / 3)
                }
            })
        })
        .collect();
    ArtImage {
        width,
        height,
        pixels,
    }
}

/// 256-palette index for a seed. Skips grayscale (16..31) and the
/// purple-blue band (92..101) — the generic AI gradient look.
pub fn placeholder_color_idx(seed: u64) -> u8 {
    let idx = 17 + (seed % 214) as u8; // 17..=230
    if (92..102).contains(&idx) {
        idx.saturating_add(12)
    } else {
        idx
    }
}

/// Approximate RGB for a 256-color cube index (used for the placeholder tones).
pub fn ansi256_to_rgb(idx: u8) -> (u8, u8, u8) {
    let steps = [0u8, 95, 135, 175, 215, 255];
    let i = (idx as usize).saturating_sub(16);
    (
        steps[(i / 36).min(5)],
        steps[((i % 36) / 6).min(5)],
        steps[(i % 6).min(5)],
    )
}

/// Map RGB to the nearest 256-palette cube entry.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let q = |c: u8| (u32::from(c) * 5 / 255).min(5) as u8;
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

impl ArtImage {
    /// Render rows of full-block cells. `colored=false` (NO_COLOR) uses
    /// density runes with default fg — still no background anywhere.
    pub fn to_lines(&self, colored: bool) -> Vec<Line<'static>> {
        self.pixels
            .chunks(self.width as usize)
            .map(|row| {
                let spans: Vec<Span> = row
                    .iter()
                    .map(|&(r, g, b)| {
                        if colored {
                            Span::styled(
                                "█",
                                Style::default().fg(Color::Indexed(rgb_to_ansi256(r, g, b))),
                            )
                        } else {
                            let lum = u16::from(r) + u16::from(g) + u16::from(b);
                            let ch = match lum / 3 {
                                0..=64 => '░',
                                65..=170 => '▒',
                                _ => '▓',
                            };
                            Span::raw(ch.to_string())
                        }
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
    }
}

/// Cache file for a track key (id + mtime + size auto-invalidates on change).
pub fn cache_file(cache_dir: &Path, id: u64, mtime_secs: u64, size: u64) -> PathBuf {
    cache_dir.join(format!("{id:016x}-{mtime_secs}-{size}.cover"))
}

/// Load cached cover bytes, if present.
pub fn load_cached(cache_dir: &Path, id: u64, mtime_secs: u64, size: u64) -> Option<Vec<u8>> {
    std::fs::read(cache_file(cache_dir, id, mtime_secs, size)).ok()
}

/// Store cover bytes (atomic tmp + rename).
pub fn store_cached(
    cache_dir: &Path,
    id: u64,
    mtime_secs: u64,
    size: u64,
    bytes: &[u8],
) -> std::io::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let dest = cache_file(cache_dir, id, mtime_secs, size);
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Evict oldest files (by mtime) until `dir` fits `cap_bytes`.
/// Returns the number of files removed.
pub fn evict_over_cap(dir: &Path, cap_bytes: u64) -> usize {
    let mut entries: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in rd.filter_map(Result::ok) {
        let path = entry.path();
        let (meta, mtime) = match (
            entry.metadata(),
            entry.metadata().and_then(|m| m.modified()),
        ) {
            (Ok(meta), Ok(mtime)) => (meta, mtime),
            _ => continue,
        };
        total += meta.len();
        entries.push((mtime, meta.len(), path));
    }
    entries.sort_by_key(|a| a.0);
    let mut removed = 0;
    for (_, size, path) in entries {
        if total <= cap_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn protocol_detection_order() {
        assert_eq!(
            detect_protocol_from(&env(&[("KITTY_WINDOW_ID", "1")])),
            ArtProtocol::Kitty
        );
        assert_eq!(
            detect_protocol_from(&env(&[("TERM", "xterm-kitty")])),
            ArtProtocol::Kitty
        );
        assert_eq!(
            detect_protocol_from(&env(&[("TERM_PROGRAM", "iTerm.app")])),
            ArtProtocol::Iterm2
        );
        assert_eq!(
            detect_protocol_from(&env(&[("TERM", "foot")])),
            ArtProtocol::Sixel
        );
        assert_eq!(detect_protocol_from(&env(&[])), ArtProtocol::Blocks);
    }

    #[test]
    fn no_color_forces_blocks() {
        assert_eq!(
            detect_protocol_from(&env(&[("KITTY_WINDOW_ID", "1"), ("NO_COLOR", "1")])),
            ArtProtocol::Blocks
        );
    }

    #[test]
    fn placeholder_is_deterministic_and_not_purple() {
        let a = placeholder(12345, 8, 4);
        let b = placeholder(12345, 8, 4);
        assert_eq!(a, b);
        assert_ne!(placeholder(1, 8, 4), placeholder(2, 8, 4));
        for seed in 0..500 {
            let idx = placeholder_color_idx(seed);
            assert!(!(92..102).contains(&idx), "purple band at seed {seed}");
            assert!((17..=230).contains(&idx) || (104..=242).contains(&idx));
        }
    }

    fn png_fixture(w: u32, h: u32, rgb: (u8, u8, u8)) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([rgb.0, rgb.1, rgb.2]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode");
        buf
    }

    #[test]
    fn thumbnail_decodes_png_and_scales() {
        let raw = png_fixture(64, 32, (200, 40, 40));
        let thumb = thumbnail(&raw, 24, 12).expect("decode");
        assert!(thumb.width <= 24 && thumb.height <= 12);
        assert_eq!(thumb.pixels.len(), (thumb.width * thumb.height) as usize);
        // Roughly red survives downscale.
        let (r, g, b) = thumb.pixels[0];
        assert!(r > 150 && g < 100 && b < 100);
    }

    #[test]
    fn thumbnail_rejects_garbage() {
        assert!(thumbnail(b"not an image", 24, 12).is_none());
    }

    #[test]
    fn missing_art_extracts_none() {
        let dir = std::env::temp_dir().join("qobi-art-missing");
        std::fs::create_dir_all(&dir).expect("tmp");
        let p = dir.join("fake.mp3");
        std::fs::write(&p, b"garbage").expect("write");
        assert!(extract_embedded(&p).is_none());
    }

    #[test]
    fn cache_roundtrip_and_evict_oldest() {
        let dir = std::env::temp_dir().join(format!(
            "qobi-art-cache-{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        store_cached(&dir, 1, 10, 5, b"11111").expect("store");
        std::thread::sleep(std::time::Duration::from_millis(5));
        store_cached(&dir, 2, 10, 5, b"22222").expect("store");
        assert_eq!(
            load_cached(&dir, 1, 10, 5).as_deref(),
            Some(b"11111".as_slice())
        );
        // Stale key (changed mtime) misses.
        assert!(load_cached(&dir, 1, 11, 5).is_none());
        // Cap fits one 5-byte file: oldest (id 1) goes.
        assert_eq!(evict_over_cap(&dir, 5), 1);
        assert!(load_cached(&dir, 1, 10, 5).is_none());
        assert!(load_cached(&dir, 2, 10, 5).is_some());
    }

    #[test]
    fn rgb_ansi_roundtrip_sane() {
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        assert_eq!(ansi256_to_rgb(16), (0, 0, 0));
    }

    #[test]
    fn mosaic_lines_use_fg_only() {
        let img = placeholder(7, 4, 2);
        for line in img.to_lines(true) {
            for span in &line.spans {
                assert!(span.style.bg.is_none(), "mosaic must not set bg");
            }
        }
        // Monochrome: default fg, density runes.
        let text: String = img
            .to_lines(false)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.clone()))
            .collect();
        assert!(text.chars().all(|c| "░▒▓".contains(c)));
    }
}
