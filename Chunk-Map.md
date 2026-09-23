# Chunk-Map.md — Qobi build order + gates

Decisions D1–D4 apply. v0.1.0 = minimal shippable slice; full vision retained, rest v0.2.
Gate per chunk: `/tdd` → implement → `/code-review` → `/verify` (`cargo test`, `clippy -- -D warnings`, `fmt --check`, relevant bench/snapshot).

## Dependency graph

```
E (config) ─┐
Guard/CLI ──┼─→ B (scan/index) → A (engine) → C (tui-min) → D (input) → F (art-basic) → H-min (queue)
                                                                    └─→ G (EQ) → I (settings) → J (integration/packaging)
```

Build order: 0 Guard/CLI → 1 E → 2 B → 3 A → 4 C → 5 D → 6 F → 7 H-min → v0.1.0 tag → 8 G → 9 I+H-full → 10 J.

## Chunk specs

### 0. Guard/CLI + IPC (new, split from C) — v0.1.0, first
- In: argv (`qobi`, `qobi <dir>`, `qobi <file>`). Out: primary boot or IPC send + exit code.
- Interface: `ipc.rs` (`ensure_primary()`, `send_enqueue()`, JSON ops `ping/enqueue-files/enqueue-dir`).
- Done: second invocation enqueues, no new window, toast path wired; stale-takeover works; exit codes 0/1/2 correct.
- Tests: lock contention, stale socket takeover, file/dir enqueue roundtrip, corrupt message ignored, permission denied.

### E. Config — v0.1.0
- In: `~/.config/qobi/config.toml`. Out: validated `Config` + first-run prompt for `music_dir`.
- Done: corrupt → `.bak` + defaults + toast; atomic writes; missing dir → re-prompt, never crash.
- Tests: load/save roundtrip, corrupt recovery, first-run flow, unwritable dir.

### B. Scan/index — v0.1.0, perf-critical
- In: dir path. Out: `Track[]` + `cache/index.json` + queue append (sorted, lazy).
- Done: warm start <300ms via cache; background walker; 20k-file cold <5s-to-music; skips + counts errors.
- Tests: empty/nested/symlink/permission dirs, mtime cache hit, 1k synthetic bench, art-hash stable.

### A. Engine — v0.1.0
- In: `Track`. Out: audio + `PlayerState` + `SpectrumTap` (dormant if EQ off).
- Interface: `AudioSink` + `Decoder` traits, mock sink for CI.
- Done: play/pause/stop/seek±5s/volume; missing/unsupported → skip + toast; gapless not required.
- Tests: state machine, seek bounds, volume clamp, skip-on-error, mock-sink 80%+.

### C. TUI-min — v0.1.0
- In: engine state + queue. Out: transparent Now Playing + Queue tab + toast + `?` help.
- Done: no bg fill (snapshot asserts `bg == Reset`); 80x24 usable; art area collapses gracefully.
- Tests: TestBackend snapshots (all tabs, toast, empty queue, long titles), resize 80x24/120x40/200x60.

### D. Input — v0.1.0
- Map: `space play/pause, →/← seek, ↑/↓ nav, Enter play, space select in browser, / search, +/- volume, ? help, q quit, Ctrl+C quit`. `Ctrl+P/X` rejected.
- Done: IPC enqueue → toast without view steal; all shortcuts in help; conflicts audited.
- Tests: key→action table, enqueue-toast path, search filter, quit paths.

### F. Art-basic — v0.1.0 basic, polish v0.2
- Done: Kitty/iTerm full, Sixel downscaled, fallback placeholder (hash-hue, no gradient); LRU 500 MiB; toggle.
- Tests: jpeg/png/webp/bmp, missing art → placeholder, cache eviction, `NO_COLOR` path.

### H-min. Queue ops — v0.1.0
- Done: append (IPC + browser), cursor play, clear; Queue tab shows enqueued items (D2 tracking).
- Tests: enqueue order, clear, play-cursor, 10k-item list perf.

### v0.1.0 TAG — ship slice
- `cargo test` green 80%+ (engine/config/queue), clippy/fmt clean, warm <300ms, cold <5s, manual matrix: macOS iTerm2/Terminal.app/Kitty + Arch foot/Alacritty.

### G. EQ — v0.2 (unless free)
- Done: 16/24 bars @30fps from SpectrumTap, hover highlight, `e` toggle + settings, zero cost when off.
- Tests: FFT determinism on fixture wav, fps budget, toggle persistence.

### I. Settings + H-full — v0.2
- Done: settings UI (volume, EQ/art toggles, theme auto/light/dark), shuffle/repeat, dedup, persistence.
- Tests: setting roundtrip, shuffle distribution smoke, repeat modes.

### J. Integration/packaging — v0.2
- Done: long-play soak, 50k-lib soak, memory-leak check, README + brew + AUR, release notes.
- Tests: e2e script (play dir → enqueue file from 2nd CLI → queue shows → pause/seek/quit), cross-terminal screenshots.

## Anti-scope (v0.1.0)
No streaming, no DSP beyond volume, no daemon, no DB, no new crates without spike, no gradient/glass UI.
