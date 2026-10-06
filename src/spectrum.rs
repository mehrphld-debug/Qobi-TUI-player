//! Background audio spectrum analysis for the live EQ.
//!
//! [`analyze`] decodes a file with symphonia and precomputes one 16-band
//! magnitude vector per [`WINDOW_SECS`] of audio using rustfft. The UI then
//! renders the vector matching the current playback position on every tick,
//! so the bars are audio-reactive without ever tapping the output device.
//! Unsupported/corrupt files yield an empty timeline — never an error that
//! could disturb playback.

use std::path::Path;

/// EQ bars per window (matches the TUI row).
pub const EQ_BANDS: usize = 16;

/// Analysis window = UI tick cadence (100ms).
pub const WINDOW_SECS: f64 = 0.1;

/// Frequency range covered by the bands (log-spaced).
const FREQ_MIN: f64 = 40.0;
const FREQ_MAX: f64 = 16_000.0;

/// Decode `path` and return per-window band magnitudes in 0..=1.
/// Each window is normalized by its own peak (plus a small floor), so quiet
/// and loud passages both animate across the full bar range.
pub fn analyze(path: &Path) -> Vec<[f32; EQ_BANDS]> {
    let (samples, rate) = match decode_mono(path) {
        Some((s, r)) if !s.is_empty() => (s, r),
        _ => return Vec::new(),
    };
    let window = (rate as f64 * WINDOW_SECS).round() as usize;
    if window < 64 {
        return Vec::new();
    }
    let mut planner = rustfft::FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(window);
    let hann: Vec<f32> = (0..window)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / window as f32).cos())
        .collect();
    let bin_hz = rate as f64 / window as f64;
    // Log-spaced band edges.
    let edges: Vec<f64> = (0..=EQ_BANDS)
        .map(|b| FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf(b as f64 / EQ_BANDS as f64))
        .collect();

    let mut timeline = Vec::with_capacity(samples.len() / window + 1);
    let mut scratch = vec![rustfft::num_complex::Complex::new(0.0f32, 0.0); window];
    for chunk in samples.chunks(window) {
        if chunk.len() < window {
            break;
        }
        for (i, s) in chunk.iter().enumerate() {
            scratch[i] = rustfft::num_complex::Complex::new(s * hann[i], 0.0);
        }
        fft.process(&mut scratch);
        let mut bands = [0.0f32; EQ_BANDS];
        for b in 0..EQ_BANDS {
            let lo = ((edges[b] / bin_hz).floor() as usize).max(1);
            let hi = ((edges[b + 1] / bin_hz).ceil() as usize)
                .min(window / 2)
                .max(lo + 1);
            let mut peak = 0.0f32;
            for c in scratch.iter().take(hi).skip(lo) {
                peak = peak.max(c.norm());
            }
            bands[b] = peak;
        }
        // Per-window peak normalization with a floor: silence stays flat,
        // anything audible uses the full range.
        let max = bands.iter().cloned().fold(1e-6f32, f32::max);
        for v in &mut bands {
            *v = (*v / max).clamp(0.0, 1.0);
        }
        timeline.push(bands);
    }
    timeline
}

/// Pick the band vector for a playback position.
pub fn at_position(timeline: &[[f32; EQ_BANDS]], pos: std::time::Duration) -> [f32; EQ_BANDS] {
    if timeline.is_empty() {
        return [0.0; EQ_BANDS];
    }
    let idx = (pos.as_secs_f64() / WINDOW_SECS).floor() as usize;
    timeline[idx.min(timeline.len() - 1)]
}

/// Decode the default audio track to mono f32 samples. Best-effort.
fn decode_mono(path: &Path) -> Option<(Vec<f32>, u32)> {
    use symphonia::core::audio::{AudioBufferRef, Signal};
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;

    let mut mono: Vec<f32> = Vec::new();
    let mut rate: Option<u32> = None;
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // Truncated tail: stop cleanly and keep every sample decoded so far.
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            // One corrupt frame must not kill the whole analysis. The old
            // `decoder.decode(&packet).ok()?` discarded everything decoded
            // so far, which left these tracks' EQ permanently flat.
            Err(e) => {
                tracing::debug!("spectrum: skipping undecodable packet: {e}");
                continue;
            }
        };
        if rate.is_none() {
            rate = decoded.spec().rate.into();
        }
        // Mix down to mono.
        let frames = decoded.frames();
        let channels = decoded.spec().channels.count();
        if channels == 0 {
            continue;
        }
        match &decoded {
            AudioBufferRef::F32(buf) => {
                for f in 0..frames {
                    let mut sum = 0.0f32;
                    for c in 0..channels {
                        sum += buf.chan(c)[f];
                    }
                    mono.push(sum / channels as f32);
                }
            }
            AudioBufferRef::S16(buf) => {
                for f in 0..frames {
                    let mut sum = 0i32;
                    for c in 0..channels {
                        sum += i32::from(buf.chan(c)[f]);
                    }
                    mono.push(sum as f32 / (channels as f32 * 32768.0));
                }
            }
            AudioBufferRef::S24(buf) => {
                for f in 0..frames {
                    let mut sum = 0i32;
                    for c in 0..channels {
                        sum += buf.chan(c)[f].0;
                    }
                    mono.push(sum as f32 / (channels as f32 * 8_388_608.0));
                }
            }
            AudioBufferRef::S32(buf) => {
                for f in 0..frames {
                    let mut sum = 0i64;
                    for c in 0..channels {
                        sum += i64::from(buf.chan(c)[f]);
                    }
                    mono.push(sum as f32 / (channels as f32 * 2_147_483_648.0));
                }
            }
            AudioBufferRef::U8(buf) => {
                for f in 0..frames {
                    let mut sum = 0i32;
                    for c in 0..channels {
                        sum += i32::from(buf.chan(c)[f]) - 128;
                    }
                    mono.push(sum as f32 / (channels as f32 * 128.0));
                }
            }
            _ => {}
        }
        if mono.len() > 60 * 60 * 192_000 {
            break; // 1h @ 192kHz cap: never balloon memory on weird files
        }
    }
    Some((mono, rate?))
}

/// Sample rate of the default track (needed before windowing).
/// Test-only probe helper; production uses the rate from [`decode_mono`].
#[cfg(test)]
fn sample_rate_of(path: &Path) -> Option<u32> {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    probed.format.default_track()?.codec_params.sample_rate
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a mono 16-bit WAV with a pure sine at `freq`.
    fn sine_wav(dir: &std::path::Path, name: &str, freq: f32, secs: u64) -> std::path::PathBuf {
        let rate = 44_100u32;
        let n = (rate as u64 * secs) as usize;
        let mut data = Vec::with_capacity(44 + n * 2);
        let write_u32 = |v: u32, out: &mut Vec<u8>| out.extend_from_slice(&v.to_le_bytes());
        let write_u16 = |v: u16, out: &mut Vec<u8>| out.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(b"RIFF");
        write_u32(36 + (n * 2) as u32, &mut data);
        data.extend_from_slice(b"WAVEfmt ");
        write_u32(16, &mut data);
        write_u16(1, &mut data);
        write_u16(1, &mut data);
        write_u32(rate, &mut data);
        write_u32(rate * 2, &mut data);
        write_u16(2, &mut data);
        write_u16(16, &mut data);
        data.extend_from_slice(b"data");
        write_u32((n * 2) as u32, &mut data);
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
            data.extend_from_slice(&((s * 20000.0) as i16).to_le_bytes());
        }
        let p = dir.join(name);
        std::fs::write(&p, &data).expect("wav fixture");
        p
    }

    #[test]
    fn garbage_file_yields_empty_timeline() {
        let dir = std::env::temp_dir().join("qobi-spec-garbage");
        std::fs::create_dir_all(&dir).expect("tmp");
        let p = dir.join("fake.mp3");
        std::fs::write(&p, b"garbage").expect("write");
        assert!(analyze(&p).is_empty());
        assert!(sample_rate_of(&p).is_none());
    }

    #[test]
    fn sine_energy_lands_in_the_right_band() {
        let dir = std::env::temp_dir().join("qobi-spec-sine");
        std::fs::create_dir_all(&dir).expect("tmp");
        // 440Hz falls in one of the low-mid log bands; 8s → 80 windows.
        let p = sine_wav(&dir, "a440.wav", 440.0, 8);
        let tl = analyze(&p);
        assert_eq!(tl.len(), 80, "one vector per 100ms");
        // Every window peaks at the same band with full-scale value 1.0.
        let mut peak_band = None;
        for w in &tl {
            let (b, v) = w
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap();
            assert!((v - 1.0).abs() < 1e-6, "window normalized to peak 1.0");
            if let Some(pb) = peak_band {
                assert_eq!(b, pb, "sine must stay in one band");
            } else {
                peak_band = Some(b);
            }
        }
        // 440Hz with 40Hz..16k log edges lands in band 6 (378..550Hz).
        assert_eq!(peak_band, Some(6));
    }

    #[test]
    fn truncated_file_keeps_partial_timeline() {
        // A file cut off mid-stream (bad download, torn write) must still
        // animate the decodable prefix — never a permanently flat EQ.
        let dir = std::env::temp_dir().join("qobi-spec-trunc");
        std::fs::create_dir_all(&dir).expect("tmp");
        let full = sine_wav(&dir, "full.wav", 440.0, 8);
        let bytes = std::fs::read(&full).expect("read");
        let cut = dir.join("cut.wav");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).expect("write");
        let partial = analyze(&cut);
        assert!(
            !partial.is_empty(),
            "truncated file must keep its decodable prefix"
        );
        assert!(
            partial.len() < 80,
            "partial timeline must be shorter than the full 80 windows"
        );
        assert!(
            partial.iter().all(|w| w.iter().any(|&v| v > 0.0)),
            "every kept window must carry signal"
        );
    }

    #[test]
    fn at_position_indexes_by_time() {
        let tl = vec![[0.1; EQ_BANDS], [0.9; EQ_BANDS]];
        assert_eq!(
            at_position(&tl, std::time::Duration::from_millis(10))[0],
            0.1
        );
        assert_eq!(
            at_position(&tl, std::time::Duration::from_millis(150))[0],
            0.9
        );
        assert_eq!(at_position(&tl, std::time::Duration::from_secs(99))[0], 0.9);
        assert_eq!(at_position(&[], std::time::Duration::ZERO), [0.0; EQ_BANDS]);
    }
}
