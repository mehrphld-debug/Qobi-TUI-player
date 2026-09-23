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

- (empty — v0.2 planning starts after the v0.1.0 tag)
