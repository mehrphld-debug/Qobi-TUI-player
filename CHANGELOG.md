# Changelog

All notable changes to Qobi. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] — 2026-09-23

First shippable slice: open → play → queue in the terminal.

### Added

- CLI: `qobi`, `qobi <dir>`, `qobi <file>` with autoplay-first and skip-bad-files
- Single-instance guard: Unix socket + stale takeover; second invocation enqueues (`enqueue-files` / `enqueue-dir`) with queue-tab toast, never a new window
- Config: TOML in `~/.config/qobi/` with first-run flow, corrupt-file backup, atomic saves
- Lazy library index: sorted recursive scan, mtime+size cache, background-friendly budgets (1k files well under 5s cold)
- Audio engine: rodio 0.22 output + symphonia decode path, `AudioSink` trait with `MockSink` for headless CI, state machine (Stopped/Loading/Playing/Paused/Ended), ±5s clamped seek, volume clamp, end-of-track auto-advance
- Transparent TUI (ratatui 0.30): Now Playing / Queue / Help views, thin progress bar, toast line, TestBackend snapshots asserting zero background fill at 60×15–200×60
- Input: full keyboard map (`space`, arrows, `/` search with live filter, `+/-`, `e`, `a`, `c`, `s`, `r`, `?`, `q`)
- Covers: embedded-art extraction (lofty), 500 MiB LRU disk cache, 24×12 fg-only mosaic, deterministic non-purple placeholders, `NO_COLOR` monochrome mode
- Queue ops: cursor play, clear (keeps playing), shuffle (keeps current head), repeat off/all/one
- Live loop: raw-mode alternate-screen TUI, 100ms tick, IPC accept task, always-restored terminal

### Verified

- 62 unit + snapshot tests green, `clippy -D warnings` clean, `fmt --check` clean
- pty smoke: boot → autoplay → mosaic + progress → `q` quit, no panic; IPC enqueue across processes

### Known limits → v0.2

- Live spectrum EQ (static preview only), native Kitty/iTerm inline art, settings persistence UI, queue dedup, Homebrew/AUR packaging, 20k-file bench, multi-terminal matrix

## [Unreleased]

### Fixed

- Cover art on short screens: the mosaic scales to fit (even row sampling
  with pinned endpoints + half-block color averaging) instead of cropping
  mid-image — the whole cover stays visible at 80x24 and the art hides only
  on truly tiny screens; over-narrow terminals also shrink it instead of
  line-wrapping it into noise
- Search selection: `Enter` on a match now plays it and leaves search mode
  (previously `Enter` only exited search, so picking music took two Enters);
  `Enter` on zero matches toasts `no matches` and plays nothing
- Live EQ going permanently flat on some tracks: one corrupt audio packet
  used to abort the whole background analysis and discard every sample
  decoded so far (3 of 106 tracks in a real library flatlined this way).
  Bad packets are now skipped (truncated tails stop cleanly), partial
  results are kept, and a panicking analysis can no longer wedge a track's
  EQ flat forever
- Now Playing hints: short form on narrow screens (no mid-hint wrapping) and
  the bottom status line no longer repeats the in-content hints

Applies the `ui-design/` screens (visual restyle only — the keymap is frozen).

### Added

- Track metadata: release year (`lofty` tag date) and codec/container label
  (`FLAC`/`MP3`/`OGG`/`OPUS`/`M4A`/`AAC`/`WAV`/…, extension fallback for
  formats lofty cannot parse; old `index.json` caches still load)
- Queue/search tables: title · artist · album · year · duration · format ·
  stable `q01…` ids, `QUEUE <nn> tracks` + `repeat <mode>` header, dedicated
  `SEARCH` section with match counts and `<filtered> / <total>`; search
  matches raw title/artist/album/path (display fallbacks excluded)
- Now Playing: centered column with playback status, mint EQ + timestamp,
  centered key hints (bound keys only); Help regrouped into
  `NAVIGATION / PLAYBACK / QUEUE / SESSION` with the offline-first footer

### Changed

- Accent ANSI cyan → mint `Rgb(126, 224, 176)` (+ amber queue ids) per the
  designs; transparency invariant (fg-only, zero background fill) unchanged
- Linear progress bar removed from Now Playing (progress lives in EQ + time)

### Known deltas vs the mockups (deferred, not bugs)

- No `p`/`n` prev/next, `j/k`, `gg/G`, `Ctrl+X`/`Ctrl+K`, `Q` queue, `a`
  enqueue — keymap changes were out of scope; `a` still toggles cover art
- Search keeps queue order (`sort: queue order`); no re-sort toggle
- `.m4a` shows as `M4A` (container) rather than guessing `ALAC` vs `AAC`
- Table needs ≥72 columns, else filename-only rows; art still collapses first
