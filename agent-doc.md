# Qobi Project — Agent Documentation

## Current Status
- **v0.1.0 released 2026-09-23** (tag `v0.1.0`, pushed to `origin/master`): README + CHANGELOG shipped, release binary verified live on pty.
- Requirements: `sys-p/0.md`, `sys-p/1.md`. Optimized plan: `sys-p/2.md`. Decisions locked in `PROJECT-CONTEXT.md`.
- Next: v0.2 planning — live spectrum EQ, native Kitty/iTerm art, settings UI, dedup, brew/AUR, 20k bench, terminal matrix.

## Last Known Good State
- Chunks 0 + E + B + A + C-min + D + F-basic + H-min implemented and green 2026-09-23: 62 tests pass (+1 ignored hw probe), `clippy -D warnings` clean, `fmt --check` clean.
- Live loop wired: `run_tui` (crossterm raw + alternate screen, key thread, IPC task, 100ms tick). pty smoke (100×30): boot → autoplay tone.wav → tabs + title + `0:00 / 0:01` + 290-cell mosaic, all bg=49 (transparent) → `q` quit clean.
- Crate: `qobi` v0.1.0, modules `cli`/`config`/`engine`/`error`/`ipc`/`library`/`tui` in `src/`, bin `qobi` + lib `qobi_lib`. Deps add: rodio 0.22 (CoreAudio construct proven on this Mac via ignored probe test), ratatui 0.30 + crossterm 0.29.
- E2E smoke (isolated HOME): primary `qobi <dir>` scanned 2 tracks + saved cache; secondary `qobi <file>` → `Queued: a.mp3`; bare `qobi` → `already running`. No new window, no crash.
- Known issues: EQ static (G, v0.2), Kitty/iTerm native passthrough deferred to F-polish (mosaic everywhere for v0.1.0); 20k-file bench + multi-terminal matrix untested.

## Locked Decisions (2026-09-23)
- D1 Scope: nothing cut from vision; phased delivery only.
- D2 Single-instance: second invocation enqueues, never spawns new window (see below).
- D3 MVP: v0.1.0 = A+B+E + C-minimal (play file/dir, volume, persistence, basic queue). Defer EQ polish, advanced search, full settings, packaging if gates at risk.
- D4 Perf-first: choose fastest option on every tradeoff; benchmarks block merge.

## Single-Instance Enqueue Spec (D2)
- Mechanism: pidfile + Unix socket in `~/.config/qobi/` (e.g. `qobi.sock`). First instance holds lock + listens.
- Behavior:
  - `qobi` with no args while running → focus existing tab (no-op, exit 0, print "already running").
  - `qobi <file>` → send `enqueue-files [path]` via socket, exit 0. Running instance appends to current queue, shows toast "Queued: <name>", stays on current view. User tracks it in queue/playlist tab.
  - `qobi <dir>` → send `enqueue-dir [path]`, same toast + queue append (recursive audio scan, lazy).
  - Socket unreachable but pidfile stale → remove stale, become primary.
  - Socket error / permission error → exit 2 with "Qobi already running but unreachable — remove ~/.config/qobi/qobi.sock or kill PID".
- No new terminal window, no new process playback. Single audio engine owns output.
- Tests: lock contention, stale socket, enqueue file/dir, corrupt socket message, permission denied.

## Project Chunks
- A: audio engine (load/play/pause/stop/seek/volume) — v0.1.0 core.
- B: scan + metadata/art cache (lazy) — v0.1.0 core (perf-critical).
- E: config + first-run setup — v0.1.0 core.
- C: TUI shell transparent + queue tab + toast — v0.1.0 minimal.
- D: keyboard input + enqueue toast handling — v0.1.0 minimal.
- F: art display (fallback matrix) — v0.1.0 basic, polish v0.2.
- G: EQ visualization — v0.2 unless free after gates.
- H: playlist ops (clear/shuffle/repeat) — v0.1.0 basic queue, full v0.2.
- I: settings (EQ toggle, art toggle) — v0.2.
- J: integration + polish + packaging — v0.2.

## Architecture Quick Reference
- Flow: `cli → single-instance-guard → library-index → player-engine → tui-shell`.
- Engine/UI via message channel (enqueue events, state updates). Engine owns rodio/symphonia sink.
- Perf-first crate direction (to confirm with search-first spike): ratatui + crossterm, symphonia decode + rodio output (or cpal if rodio spectrum tap blocks EQ), lofty metadata, clap CLI, serde+TOML config, thiserror.
- Transparency: no background fill; theme-aware colors; art via Kitty → Sixel → block fallback.

## Code Structure (target)
- `src/main.rs` → CLI + single-instance guard.
- `src/engine/` → playback, sink trait + mock for CI.
- `src/library/` → scan, metadata cache, queue.
- `src/config/` → load/save, first-run.
- `src/tui/` → shell, queue view, toast, help.
- `src/ipc.rs` → socket protocol (`enqueue-files`, `enqueue-dir`, `ping`).

## Testing Strategy
- `cargo test` + TestBackend snapshots for TUI; dummy AudioSink for headless CI.
- Fixtures: corrupt TOML, missing/unsupported files, 50k-file fake lib for startup bench.
- Gates: 80%+ on engine/config/queue, clippy -D warnings, fmt --check, startup <300ms warm / <5s cold-to-music baselines TBD.

## Next Immediate Steps
1. Chunk C-min: TUI shell (ratatui + crossterm, transparent Now Playing + Queue + toast + `?` help, TestBackend snapshots).
2. Chunk D: keyboard input map + IPC enqueue → toast path.
3. Tag v0.1.0 per Chunk-Map gates, then G → I+H-full → J.

## Blockers & Decisions Needed
- None blocking. Open: socket path XDG vs ~/.config/qobi; dir-enqueue order (sorted vs walk order); toast duration.

## Key Files & Locations
- Specs: `sys-p/0.md`, `sys-p/1.md`, `sys-p/2.md`. Context: `PROJECT-CONTEXT.md`.
- Config/cache (runtime): `~/.config/qobi/config.toml`, `qobi.sock`, `qobi.pid`, `cache/`.

## Building & Running Locally
- TBD after scaffold: `cargo run -- --help`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.

## Performance Baselines
- Target: startup <300ms warm, open-to-music <5s cold.
- To measure: cold 0/1k/20k/50k-file libs, SSD reference on M-series + Arch box. Blockers if exceeded.

## Cross-Platform Notes
- macOS: CoreAudio via rodio; Terminal.app weakest art support → block fallback.
- Arch: ALSA/Pulse/PipeWire variance → sink trait + CI mock. Test foot/WezTerm/Alacritty/Kitty.

## Dependencies (directional, pin after spike)
- ratatui, crossterm, rodio, symphonia, lofty, clap, serde, toml, thiserror, anyhow.

## Known Limitations
- v0.1.0: foreground only, no streaming, no DSP beyond volume + basic EQ viz later, no daemon.
