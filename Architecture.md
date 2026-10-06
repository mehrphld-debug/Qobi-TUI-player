# Architecture.md — Qobi TUI Local Music Player

Source: `sys-p/0.md`, `sys-p/1.md`, optimized plan `sys-p/2.md`. Decisions: `PROJECT-CONTEXT.md`, `agent-doc.md` (D1–D4).
Stack direction: Rust 2021 + ratatui + crossterm + symphonia/rodio + lofty + clap + serde/TOML + thiserror. Pin exact versions in scaffold spike.

## 1. System overview

```
cli (clap: qobi | qobi <dir> | qobi <file>)
 → single-instance-guard (pidfile + Unix socket ~/.config/qobi/)
 → primary: library-index + player-engine + tui-shell
 → secondary: IPC enqueue → exit 0 (never spawns window)
```

- `cli → guard → index → engine → tui`. Engine owns audio output. UI talks to engine via mpsc channel (commands down, state up). IPC messages from secondary instances enter the same channel as local input.
- Offline only. No network, no daemon, no streaming. Foreground single audio owner.

## 2. CLI + single-instance enqueue (D2, locked)

- Socket: `~/.config/qobi/qobi.sock`, lock: `qobi.pid`. Primary holds both.
- `qobi` (no args, primary running) → print `Qobi already running`, exit 0, no-op (user returns to existing tab).
- `qobi <file>` → `{"op":"enqueue-files","paths":[...]}` → primary appends to current queue, toast `Queued: <name>`, stays on current view. Exit 0.
- `qobi <dir>` → `{"op":"enqueue-dir","path":...}` → lazy recursive scan (audio extensions only), sorted walk order, append, toast `Queued N tracks from <dir>`. Exit 0.
- Stale pidfile + dead socket → remove, become primary. Socket error → exit 2: `Qobi already running but unreachable — remove ~/.config/qobi/qobi.sock or kill PID`.
- Protocol: newline-delimited JSON, ops `ping`, `enqueue-files`, `enqueue-dir`. Max message 1 MiB. Malformed → ignore + log, never crash.
- Tests: lock contention, stale takeover, file/dir enqueue, corrupt message, permission denied.

## 3. Library index (perf-critical, D4)

- Lazy by design. Startup never blocks on full scan: load `cache/index.json` (path, mtime, size, metadata) → render in <300ms warm → background walker refreshes.
- Walker: `walkdir` sorted, audio extensions `mp3 flac wav ogg oga opus m4a aac wma aiff`, symlink follow off, permission errors skipped + counted.
- Metadata: `lofty` only for new/changed (mtime+size). Cover art extracted once → `cache/art/<hash>.bin`, LRU cap 500 MiB.
- Queue model: `VecDeque<Track>` + cursor. Dir-enqueue = append sorted. File-enqueue = append. No dedup in v0.1.0 (dedup v0.2).
- Benches block merge: cold 0/1k/20k/50k-file libs; warm cached start; scan throughput. Targets: warm <300ms, cold-to-music <5s on M-series + Arch SSD reference.

## 4. Player engine

- Traits (mockable for headless CI):
  ```rust
  trait AudioSink { fn play(&mut self, src: DecodedStream); fn pause(); fn resume(); fn stop(); fn set_volume(f32); fn position() -> Duration; }
  trait Decoder { fn decode(path) -> DecodedStream + SpectrumTap; }
  ```
- v0.1.0: volume-only (0.0–1.0, host output config passthrough). No DSP/effects.
- Decode: `symphonia` (all formats) → `rodio` output (CoreAudio macOS, ALSA/Pulse/PipeWire Linux). If rodio spectrum tap blocks EQ, fall back to `cpal` + direct symphonia pipeline (spike decides, perf wins per D4).
- SpectrumTap: FFT (e.g. 64 bands → downsample to 16/24 bars for TUI) on decode thread, lock-free ring to UI at ~30fps. Disabled = zero FFT cost (settings toggle).
- State machine: `Stopped → Loading → Playing ⇄ Paused → Ended → next`. Missing/unsupported file → skip + toast, back to picker, never crash.

## 5. TUI shell (ratatui + crossterm)

- Views (tabs): `Now Playing | Queue | Browser | Help`. Queue tab is where enqueued tracks appear (D2 tracking requirement).
- Now Playing (ui-design/Main page.png): centered column — art (whole mosaic scaled to the space left by the 11-row chrome via even row sampling + half-block color averaging; hidden only below 4 rows) → playback status (`▶ PLAYING` / `‖ PAUSED` / `○ STOPPED`) → title (bold) / artist — album (dim) → EQ bars (16, toggleable) → `elapsed / total` timestamp → centered key hints (short form on narrow screens). No linear progress bar by design; progress lives in the EQ + timestamp. Status bar stays empty here (no toast) so hints are not doubled.
- Queue (ui-design/Playlist section.png): header `QUEUE <nn> tracks` + `repeat <mode>  shuffle off`; table rows `▸ nn title artist album year duration format qid` (≥72 cols, title flexes, cells truncated with `…`), filename-only fallback below that. Playing row number mint, `FLAC` mint / other formats dim, `qid` (`q01`…) amber and stable across search filtering.
- Search (ui-design/Search section.png): dedicated section while filtering — `SEARCH` header, `/ <query>▏ in title, artist, album, path` + `<n> matches · esc clear`, mint divider, `<filtered> / <total>` + `sort: queue order` (queue order is kept; no re-sort). Matches raw title/artist/album/path (display fallbacks excluded so everyday bigrams don't match everything).
- Help (ui-design/Help section.png): `HELP` header + intro paragraph, grouped `NAVIGATION / PLAYBACK / QUEUE / SESSION` sections listing the real input map only, offline-first footer.
- Input: keyboard-first. Map is **frozen** (ui-design pass was visuals-only): `space play/pause, →/← seek ±5s, ↑/↓ navigate, Enter play selected, Tab now playing/queue, / search (Enter plays the match, Esc exits), q quit, ? help, +/- volume, e EQ toggle, a art toggle, c clear queue, s shuffle, r repeat`. The designs sketch `p/n` prev/next, `j/k`, `gg/G`, `Ctrl+X/K`, `Q` queue, `a` enqueue — all deferred to a future keymap change; the hint lines advertise bound keys only.
- Rendering rules: **never set background fill** — transparent by omission. All panels `Style::default()` bg `Reset`. Theme-aware: derive fg from terminal (light/dark detect via `$COLORFGBG` fallback dark). Minimum 80x24, responsive shrink (art collapses first).
- Input: keyboard-first. Draft map (audit vs terminal before scaffold): `space play/pause, →/← seek ±5s, ↑/↓ navigate, Enter play selected, space select, / search, q quit, ? help, +/- volume, e EQ toggle, a art toggle, c clear queue, s shuffle, r repeat`. `Ctrl+C` = quit (terminal standard). `Ctrl+P/X` from old spec **rejected** (shell conflicts) — remapped above.
- Mouse: secondary only. Hover over EQ = highlight bar + tooltip band freq; click = nothing destructive. Disabled when `crossterm` mouse unsupported.

## 6. Album art + EQ fallback matrix (must spec before F/G)

| Terminal | Graphics | Art result | EQ |
|---|---|---|---|
| Kitty, WezTerm | Kitty protocol | full image | full bars + color |
| iTerm2 | inline images | full image | full bars + color |
| foot, VTE Sixel | Sixel | downscaled | bars, limited color |
| Terminal.app, Alacritty (no img) | none | generated placeholder (light random color / math pattern per spec §1) | block bars `▁▂▃▄▅▆▇█` |
- Placeholder: deterministic hash → hue, never purple-blue default gradient (anti-slop). Art cache LRU. EQ colors disableable in settings (monochrome bars).

## 7. TUI design tokens (design-system Mode 1, TUI-adapted)

No CSS exists (greenfield) — tokens derived from Apple/Notion + transparency constraint. No gradients, no glassmorphism, no gratuitous rounding (TUI has no rounding; use thin borders `─│┌┐└┘` sparingly).

- `color.bg`: `transparent` (never fill). `color.fg`: `terminal-fg`. `color.dim`: `terminal-fg 60%`. `color.accent`: mint `Rgb(126, 224, 176)` bold (ui-design; was ANSI cyan). `color.qid`: amber `Rgb(232, 184, 96)` for queue ids. `color.warn`: `ANSI yellow/red` only for errors/toasts.
- `type.title`: bold, 1 line. `type.body`: normal. `type.caption`: dim. Hierarchy: title > progress/time > list > caption. No custom fonts (terminal font wins).
- `space.unit`: 1 cell. Rhythm: 1/2/4 cells padding. Breathing room: art gets ≥2 cells margin; list density comfortable (1 line per track).
- `motion.eq`: 30fps max, pauses when paused/muted; respects `NO_COLOR` + settings toggle.
- Anti-slop: one accent only, no rainbow EQ default (single-hue bars, color mode opt-in), no hero/gradient, no scroll animations.

## 8. Config + persistence

- `~/.config/qobi/config.toml`: `music_dir`, `volume`, `eq_enabled`, `art_enabled`, `theme (auto/light/dark)`. Corrupt → backup `config.toml.bak` + regenerate defaults + toast (never crash).
- First run: prompt for `music_dir` (validate readable dir, rescan). Persist on change.
- Cache: `cache/index.json` + `cache/art/`. Atomic writes (tmp + rename).

## 9. Errors (never crash)

Missing file, unsupported codec, permission denied, corrupt config/art, dead socket → toast + log to `~/.config/qobi/qobi.log`, return to picker/queue. Exit codes: 0 ok/enqueued, 1 usage error, 2 IPC unreachable.

## 10. Cross-platform

- macOS: CoreAudio via rodio; test iTerm2, Terminal.app (fallback), Kitty, WezTerm, Alacritty, Ghostty.
- Arch: ALSA/PipeWire/Pulse via rodio; test foot, Kitty, WezTerm, Alacritty. CI uses mock sink (no audio hw).
- Transparency verified by screenshot (no bg pixels) + `NO_COLOR` path.

## 11. Crate direction (perf wins, pin in scaffold)

`ratatui 0.29+`, `crossterm`, `symphonia`, `rodio` (or `cpal` if EQ tap fails), `lofty`, `clap 4`, `serde`, `toml`, `walkdir`, `thiserror`, `anyhow`, `tokio` or `std::thread` (prefer std unless async needed — fewer deps, faster build). Spike before Chunk A: bench decode + startup.

## 12. What this unlocks next

Chunk-Map.md implements in order: guard+CLI → config → index/scan → engine → TUI-min → art-basic → queue-ops → EQ → settings → packaging, each with /tdd + /verify gates.
