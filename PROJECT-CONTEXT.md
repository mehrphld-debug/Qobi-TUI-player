# PROJECT-CONTEXT.md — Qobi TUI Local Music Player

## Project name and purpose
Qobi — the most beautiful and fast local-only TUI music player for terminal. Open fast, find/select/play in seconds, get back to work, return anytime to check art/EQ/playlist.

## Tech stack
Rust (edition 2021) + ratatui + crossterm + rodio/symphonia for playback/decoding + lofty for metadata/cover art. TOML config in `~/.config/qobi/`. No DB for v0.1.0 (file cache only).

## Current phase
Greenfield. Requirements in `sys-p/0.md` + `sys-p/1.md`, optimized EPIC prompt in `sys-p/2.md`. No code yet. Dev-team review done 2026-09-23. Next: Architecture.md + Chunk-Map.md, then scaffold.

## Key constraints
- Fully transparent TUI (no background), Apple + Notion philosophy, keyboard-first, mouse secondary, built-in help.
- Offline only, single-instance foreground only, no daemon, no streaming (Spotify/SoundCloud deferred).
- macOS + Arch Linux parity. Volume-only audio (host output config).
- Perf: <5s terminal-open to music, <300ms startup target (cold vs warm + lib size to be baselined).
- Graceful errors: missing/unsupported/permission → back to picker, never crash.

## What "done" looks like (v0.1.0)
- `cargo test` green, 80%+ on engine/config/playlist; `clippy -- -D warnings` + `fmt --check` clean.
- CLI: `qobi` (configured dir), `qobi <dir>`, `qobi <file>` all working.
- Transparent layout + art + EQ visible or graceful fallback (Kitty/Sixel/block matrix).
- Help page lists all shortcuts. Brew + AUR packaging.

## Locked team decisions (2026-09-23)
1. **Scope (PM Q): nothing cut from vision.** Full vision retained. Delivery is phased, not cut.
2. **Single-instance (Arch Q): enqueue.** Second `qobi <file|dir>` adds to current queue of the running instance. Never opens a new Qobi window. User tracks it in the Qobi tab/queue view after.
3. **Phased MVP (Dev Q): yes.** v0.1.0 = engine + minimal TUI slice (Chunks A+B+E + C-minimal: play file/dir + volume + persistence + basic queue). Art full-fidelity / EQ polish / search-advanced / settings-full / packaging-slip to v0.2 only if perf gates at risk. Vision unchanged.
4. **Perf-first (QA Q): choose best for performance.** All crate/arch choices optimize for startup latency, scan speed, idle CPU/memory. Lazy index + background walker + cache mandatory. Benchmarks block merge.
