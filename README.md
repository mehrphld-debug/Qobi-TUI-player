# Qobi — TUI Local Music Player

Beautiful, fast, local-only music player for your terminal. macOS + Arch Linux. Rust.

Open it, find music, play, get back to work. Come back anytime to check the cover,
queue, and what's playing — all without leaving the terminal.

![status](https://img.shields.io/badge/status-v0.1.0-blue)
![license](https://img.shields.io/badge/license-MIT-green)

## Features (v0.1.0)

- **Instant local playback** — `qobi`, `qobi <dir>`, `qobi <file>`; auto-plays first track
- **Single instance** — a second `qobi <file>` enqueues into the running player, never opens a new window
- **Transparent TUI** — no background fill, follows your terminal theme (Apple/Notion-inspired restraint)
- **Cover mosaic** — embedded art where present (cached, 500 MiB LRU), deterministic placeholder covers otherwise
- **Queue** — cursor play, clear, shuffle, repeat off/all/one, `/` filter search
- **Keyboard-first** — full map below, `?` help page built in
- **Offline & private** — no network, no daemon, config in `~/.config/qobi/`

## Install

Qobi v0.1.0 installs from source with Cargo. Prebuilt binaries and
Homebrew/AUR packages are planned — see [CHANGELOG](CHANGELOG.md).

### 1. Install Rust

macOS and Linux both use [rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

macOS also needs the Xcode command line tools (CoreAudio headers):

```sh
xcode-select --install
```

Linux needs ALSA development headers at **build time**
(runtime works with ALSA, PulseAudio, or PipeWire):

```sh
# Debian / Ubuntu
sudo apt install build-essential libasound2-dev pkg-config
# Arch Linux
sudo pacman -S base-devel alsa-lib
# Fedora
sudo dnf install gcc alsa-lib-devel pkgconf-pkg-config
```

### 2. Build and install Qobi

```sh
git clone git@github.com:mehrphld-debug/Qobi-TUI-player.git
cd Qobi-TUI-player
cargo install --path . --locked
```

This places the `qobi` binary in `~/.cargo/bin/`. Make sure it is on your `PATH`
(Cargo's installer usually does this for new shells; otherwise add it once):

```sh
# macOS (zsh) and most Linux shells — add to ~/.zshrc or ~/.bashrc:
export PATH="$HOME/.cargo/bin:$PATH"
```

Verify:

```sh
qobi --version   # → qobi 0.1.0
```

Point it at music:

```sh
qobi ~/Music
```

### 3. Where things live

| What | Location |
|------|----------|
| Binary | `~/.cargo/bin/qobi` |
| Settings | `~/.config/qobi/config.toml` |
| Scan + cover cache | `~/.config/qobi/cache/` |
| Single-instance socket | `~/.config/qobi/qobi.sock` |

First run with no arguments and no configured directory prints a hint —
just run `qobi <dir>` or `qobi <file>` once to get going.

### 4. Updating

```sh
cd Qobi-TUI-player
git pull origin master
cargo install --path . --locked
```

### 5. Uninstall

Remove the binary (both OSes):

```sh
cargo uninstall qobi
# or, if installed manually: rm ~/.cargo/bin/qobi
```

Optionally remove settings, cache, and socket (both OSes):

```sh
rm -rf ~/.config/qobi
```

That is everything Qobi creates — no background services, no daemons,
no files outside `~/.cargo/bin` and `~/.config/qobi`.

## Usage

```sh
qobi                 # play the configured music directory
qobi ~/Music        # play all audio in a directory (remembers nothing, just plays)
qobi song.mp3       # play one file
```

First run asks for nothing — just point it at music. The configured directory,
volume, and toggles live in `~/.config/qobi/config.toml`; the scan cache and
cover cache live under `~/.config/qobi/cache/`.

While Qobi runs, any new invocation enqueues instead of opening:

```sh
qobi other-song.mp3  # → "Queued: other-song.mp3" in the running player
```

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `space` | play / pause |
| `→` / `←` | seek ±5s |
| `↑` / `↓` | navigate queue |
| `Enter` | play selected |
| `/` | search queue (`Esc`/`Enter` exits) |
| `+` / `-` | volume |
| `e` | toggle equalizer |
| `a` | toggle cover art |
| `c` | clear queue |
| `s` | shuffle queue |
| `r` | repeat off → all → one |
| `?` | help |
| `q` / `Ctrl+C` | quit |

## Building & testing

```sh
cargo test                          # 60+ unit + snapshot tests
cargo test -- --ignored rodio       # hardware audio probe (needs a device)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Scope & limits (v0.1.0)

- Foreground only, no background service or daemon
- Volume-only audio (uses host output config), no DSP/effects
- Static EQ preview; live spectrum lands in v0.2
- Cover mosaic everywhere; native Kitty/iTerm inline graphics land in v0.2
- No streaming (Spotify/SoundCloud are future work, local-only by design)

See [Architecture.md](Architecture.md), [Chunk-Map.md](Chunk-Map.md), and [CHANGELOG](CHANGELOG.md).
