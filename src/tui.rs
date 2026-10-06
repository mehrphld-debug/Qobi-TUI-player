use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::spectrum::EQ_BANDS;

/// TUI views. `Browser` lands in Chunk D; C-min ships these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    NowPlaying,
    Queue,
    Help,
}

/// Playback status for the Now Playing header (`ui-design/Main page.png`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Playback {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Playback {
    fn label(self) -> &'static str {
        match self {
            Self::Playing => "▶ PLAYING",
            Self::Paused => "‖ PAUSED",
            Self::Stopped => "○ STOPPED",
        }
    }
}

/// Display-only track snapshot. The TUI never owns audio state (Chunk A owns it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// One rendered playlist / search row (`ui-design/Playlist section.png`).
/// Plain data — the TUI never owns library state (Chunk B owns it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRow {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub duration: String,
    pub format: String,
    /// Stable queue id (`q01`, `q02`, …) from the unfiltered queue position,
    /// so filtered search rows keep the id they have in the Queue view.
    pub qid: String,
}

/// Full UI state for one frame. Plain data — trivially snapshot-testable.
#[derive(Debug, Clone)]
pub struct UiState {
    pub view: View,
    pub track: Option<TrackInfo>,
    pub playback: Playback,
    pub progress: f32,
    pub elapsed_secs: u64,
    pub total_secs: Option<u64>,
    pub queue: Vec<TrackRow>,
    /// Unfiltered queue length, for the `QUEUE 03 tracks` / `8 / 324` headers.
    pub total_tracks: usize,
    /// Repeat mode label owned by the controller (`off` / `all` / `one`).
    pub repeat_label: String,
    pub cursor: usize,
    pub toast: Option<String>,
    pub eq_enabled: bool,
    /// Live per-band levels 0..=1 for the current track position.
    /// All zeros = analysis pending/unavailable (renders flat dim bars).
    pub eq_bars: [f32; EQ_BANDS],
    /// Active search query (`None` = search mode off). Rendered, never edited here.
    pub search_query: Option<String>,
    /// Pre-rendered cover mosaic (fg-only cells). Empty = no art area.
    pub art: Vec<Line<'static>>,
    /// Cover visibility toggle (Chunk I persists it).
    pub art_enabled: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            view: View::NowPlaying,
            track: None,
            playback: Playback::Stopped,
            progress: 0.0,
            elapsed_secs: 0,
            total_secs: None,
            queue: Vec::new(),
            total_tracks: 0,
            repeat_label: "off".to_string(),
            cursor: 0,
            toast: None,
            eq_enabled: true,
            eq_bars: [0.0; EQ_BANDS],
            search_query: None,
            art: Vec::new(),
            art_enabled: true,
        }
    }
}

/// Mint accent from the ui-design screens. Theme-aware by omission: every
/// style below sets fg only, so the terminal background (transparent) always
/// shows through.
fn accent() -> Style {
    Style::default()
        .fg(Color::Rgb(126, 224, 176))
        .add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// Amber queue ids (`q01` …) from the playlist / search designs.
fn amber() -> Style {
    Style::default().fg(Color::Rgb(232, 184, 96))
}

fn warn() -> Style {
    Style::default().fg(Color::Yellow)
}

fn fmt_time(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Centered `elapsed / total` timestamp from the Main page design.
/// The linear bar is gone on purpose: progress lives in the EQ + timestamp.
fn time_line(elapsed: u64, total: Option<u64>) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{} / {}",
            fmt_time(elapsed),
            total.map(fmt_time).unwrap_or_else(|| "--:--".to_string())
        ),
        dim(),
    ))
    .centered()
}

/// Actual key hints, centered. Only keys that exist in the input map are
/// advertised — the designs sketch `Prev [p]` / `Next [n]`, which have no
/// binding (keymap changes were deferred), so they stay out of the hint.
/// Narrow screens get the short form so the line never wraps mid-hint.
fn controls_hint(width: usize) -> Line<'static> {
    if width >= 78 {
        Line::from(vec![
            Span::styled("space", accent()),
            Span::styled(" play/pause · ", dim()),
            Span::styled("←/→", accent()),
            Span::styled(" seek · ", dim()),
            Span::styled("tab", accent()),
            Span::styled(" queue · ", dim()),
            Span::styled("/", accent()),
            Span::styled(" search · ", dim()),
            Span::styled("?", accent()),
            Span::styled(" help · ", dim()),
            Span::styled("q", accent()),
            Span::styled(" quit", dim()),
        ])
        .centered()
    } else {
        Line::from(vec![
            Span::styled("space", accent()),
            Span::styled(" play/pause · ", dim()),
            Span::styled("tab", accent()),
            Span::styled(" queue · ", dim()),
            Span::styled("/", accent()),
            Span::styled(" search · ", dim()),
            Span::styled("q", accent()),
            Span::styled(" quit", dim()),
        ])
        .centered()
    }
}

/// Live EQ row: 16 bars from per-band levels. Flat dim bars while the
/// background analysis is pending; accent bars once audio data arrives.
fn eq_line(bars: &[f32; EQ_BANDS]) -> Line<'static> {
    const GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let live = bars.iter().any(|v| *v > 0.0);
    let text: String = bars
        .iter()
        .map(|v| GLYPHS[(v.clamp(0.0, 1.0) * 7.0).round() as usize])
        .collect();
    Line::from(Span::styled(text, if live { accent() } else { dim() }))
}

fn tabs_line(active: View) -> Line<'static> {
    let tab = |name: &'static str, v: View| {
        Span::styled(
            format!(" {name} "),
            if v == active { accent() } else { dim() },
        )
    };
    Line::from(vec![
        tab("Now Playing", View::NowPlaying),
        Span::styled("|", dim()),
        tab("Queue", View::Queue),
        Span::styled("|", dim()),
        tab("Help (?)", View::Help),
    ])
}

/// How many cover rows fit: the whole mosaic scales to the space left by
/// the Now Playing chrome instead of cropping mid-image (the old rule cut
/// the cover to 12/20 rows at 80x24, which read as a broken picture).
/// Chrome = status+title+artist+EQ+time+hints+blanks (11 rows when a track
/// is loaded); below [`ART_MIN_ROWS`] the mosaic stops reading as a picture
/// and hides entirely.
const NP_CHROME_ROWS: u16 = 11;
const ART_MIN_ROWS: usize = 4;

/// Scale the mosaic to fit `max_rows` × `max_width` cells: rows are evenly
/// sampled with endpoints pinned (first and last always survive), and
/// over-wide rows merge adjacent cell pairs by averaging their half-block
/// colors, so the whole cover stays visible on short/narrow screens.
/// Monochrome density runes merge by keeping the denser of the pair.
fn fit_art(lines: &[Line<'static>], max_rows: usize, max_width: usize) -> Vec<Line<'static>> {
    if lines.is_empty() || max_rows == 0 || max_width == 0 {
        return Vec::new();
    }
    let n = lines.len();
    let take = max_rows.min(n);
    let idx: Vec<usize> = if take >= n {
        (0..n).collect()
    } else if take == 1 {
        vec![0]
    } else {
        // (n-1)/(take-1) >= 1, so indices are strictly increasing.
        (0..take).map(|i| i * (n - 1) / (take - 1)).collect()
    };
    idx.into_iter()
        .map(|i| shrink_line(&lines[i], max_width))
        .collect()
}

/// Halve a row's width by merging adjacent cell pairs until it fits.
/// A trailing odd cell survives as-is; exact trim keeps the image start.
fn shrink_line(line: &Line<'static>, max_width: usize) -> Line<'static> {
    let mut spans = line.spans.clone();
    while spans.len() > max_width && spans.len() > 1 {
        spans = spans.chunks(2).map(merge_pair).collect();
    }
    spans.truncate(max_width);
    Line::from(spans)
}

fn merge_pair(pair: &[Span<'static>]) -> Span<'static> {
    let [a, b] = pair else {
        return pair.first().cloned().unwrap_or_else(|| Span::raw(""));
    };
    if a.content == "▀" && b.content == "▀" {
        Span::styled(
            "▀",
            Style::default()
                .fg(avg_color(a.style.fg, b.style.fg))
                .bg(avg_color(a.style.bg, b.style.bg)),
        )
    } else {
        // Density runes (monochrome mode): the denser cell wins.
        if rune_density(&b.content) > rune_density(&a.content) {
            b.clone()
        } else {
            a.clone()
        }
    }
}

fn rune_density(s: &str) -> u8 {
    match s {
        "█" => 4,
        "▓" => 3,
        "▒" => 2,
        "░" => 1,
        "▀" => 2,
        _ => 0,
    }
}

/// Average two cell colors in RGB space. Stays in the 256-palette when both
/// sides are indexed (256-color terminals must never receive truecolor);
/// a missing side takes the present color, both missing stay `Reset`.
fn avg_color(a: Option<Color>, b: Option<Color>) -> Color {
    use crate::art::{ansi256_to_rgb, rgb_to_ansi256};
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(i) => Some(ansi256_to_rgb(i)),
        _ => None,
    };
    let mid = |(x, y): ((u8, u8, u8), (u8, u8, u8))| {
        let m = |p: u8, q: u8| ((u16::from(p) + u16::from(q)) / 2) as u8;
        (m(x.0, y.0), m(x.1, y.1), m(x.2, y.2))
    };
    match (a.and_then(rgb), b.and_then(rgb)) {
        (Some(x), Some(y)) => {
            let (r, g, bl) = mid((x, y));
            match (a, b) {
                (Some(Color::Indexed(_)), Some(Color::Indexed(_))) => {
                    Color::Indexed(rgb_to_ansi256(r, g, bl))
                }
                _ => Color::Rgb(r, g, bl),
            }
        }
        (Some(_), None) => a.unwrap_or(Color::Reset),
        (None, Some(_)) => b.unwrap_or(Color::Reset),
        (None, None) => Color::Reset,
    }
}

fn render_now_playing(frame: &mut Frame, area: Rect, state: &UiState) {
    // Centered column per ui-design/Main page.png: cover → status → title →
    // artist/album → EQ → timestamp → key hints.
    let mut rows: Vec<Line> = Vec::new();
    // Cover mosaic: scales the whole picture to the space left by the
    // chrome (collapses entirely when too little is left).
    if state.art_enabled && !state.art.is_empty() {
        let budget = area.height.saturating_sub(NP_CHROME_ROWS) as usize;
        if budget >= ART_MIN_ROWS {
            let fitted = fit_art(&state.art, budget, area.width as usize);
            if !fitted.is_empty() {
                rows.extend(fitted.into_iter().map(|l| l.centered()));
                rows.push(Line::from(""));
            }
        }
    }
    rows.push(Line::from(Span::styled(state.playback.label(), accent())).centered());
    rows.push(Line::from(""));
    match &state.track {
        Some(track) => {
            rows.push(
                Line::from(Span::styled(
                    track.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ))
                .centered(),
            );
            rows.push(
                Line::from(Span::styled(
                    format!("{} — {}", track.artist, track.album),
                    dim(),
                ))
                .centered(),
            );
        }
        None => rows.push(
            Line::from(Span::styled(
                "nothing playing — enqueue with qobi <file>",
                dim(),
            ))
            .centered(),
        ),
    }
    rows.push(Line::from(""));
    if state.eq_enabled {
        rows.push(eq_line(&state.eq_bars).centered());
    } else {
        rows.push(Line::from(Span::styled("(eq off)", dim())).centered());
    }
    rows.push(Line::from(""));
    rows.push(time_line(state.elapsed_secs, state.total_secs));
    rows.push(Line::from(""));
    rows.push(controls_hint(area.width as usize));
    frame.render_widget(Paragraph::new(rows).centered(), area);
}

/// Pad or truncate a cell to exactly `width` chars (char-based; CJK may
/// misalign by a cell — accepted, the previous renderer had the same limit).
fn fit(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        if width == 0 {
            return String::new();
        }
        let mut t: String = chars[..width - 1].iter().collect();
        t.push('…');
        t
    } else {
        let mut t = s.to_string();
        while t.chars().count() < width {
            t.push(' ');
        }
        t
    }
}

/// One line with a left and a right group (`QUEUE 03 tracks … repeat off`).
/// The middle is space-filled; on overflow the left group wins and ratatui
/// clips the rest — never a background fill, never a panic.
fn sides(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let lw: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let rw: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let mut spans = left;
    if lw + rw < width {
        spans.push(Span::raw(" ".repeat(width - lw - rw)));
    }
    spans.extend(right);
    Line::from(spans)
}

/// Fixed column widths for the playlist / search tables. Gaps are single
/// spaces; the title column takes whatever is left (`width - FIXED_USED`).
/// Below `WIDE_MIN` columns the table falls back to plain title rows.
const WIDE_MIN: usize = 72;
const FIXED_USED: usize = 64; // marker+num+artist+album+year+dur+fmt+qid+gaps

fn track_line(row: &TrackRow, index: usize, current: bool, width: usize) -> Line<'static> {
    let title_w = width.saturating_sub(FIXED_USED).max(8);
    let num = format!("{:02}", index + 1);
    let (marker, num_style) = if current {
        (Span::styled("▸ ", accent()), accent())
    } else {
        (Span::styled("  ", dim()), dim())
    };
    let fmt_style = if row.format == "FLAC" {
        accent()
    } else {
        dim()
    };
    Line::from(vec![
        marker,
        Span::styled(format!("{} ", fit(&num, 2)), num_style),
        Span::styled(
            format!("{} ", fit(&row.title, title_w)),
            if current {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
        Span::styled(format!("{} ", fit(&row.artist, 14)), Style::default()),
        Span::styled(format!("{} ", fit(&row.album, 18)), dim()),
        Span::styled(format!("{} ", fit(&row.year, 4)), dim()),
        Span::styled(format!("{} ", fit(&row.duration, 5)), dim()),
        Span::styled(format!("{} ", fit(&row.format, 4)), fmt_style),
        Span::styled(row.qid.clone(), amber()),
    ])
}

/// Narrow-screen fallback: marker + title only, same as the pre-design rows.
fn simple_line(title: &str, current: bool) -> Line<'static> {
    if current {
        Line::from(vec![
            Span::styled("▸ ", accent()),
            Span::styled(title.to_string(), accent()),
        ])
    } else {
        Line::from(format!("  {title}"))
    }
}

fn render_queue(frame: &mut Frame, area: Rect, state: &UiState) {
    let width = area.width as usize;
    let mut top: Vec<Line> = Vec::new();
    if let Some(q) = &state.search_query {
        // Dedicated SEARCH section per ui-design/Search section.png.
        top.push(Line::from(Span::styled("SEARCH", dim())));
        top.push(sides(
            vec![
                Span::styled("/ ", accent()),
                Span::styled(q.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("▏ in title, artist, album, path", dim()),
            ],
            vec![Span::styled(
                format!("{} matches · esc clear", state.queue.len()),
                dim(),
            )],
            width,
        ));
        top.push(Line::from(Span::styled("─".repeat(width.max(1)), accent())));
        top.push(sides(
            vec![Span::raw(format!(
                "{} / {}",
                state.queue.len(),
                state.total_tracks
            ))],
            vec![Span::styled("sort: queue order", dim())],
            width,
        ));
    } else {
        top.push(sides(
            vec![
                Span::styled("QUEUE  ", dim()),
                Span::raw(format!("{:02} tracks", state.total_tracks)),
            ],
            vec![Span::styled(
                format!("repeat {}  shuffle off", state.repeat_label),
                dim(),
            )],
            width,
        ));
    }
    if state.queue.is_empty() {
        top.push(Line::from(Span::styled(
            if state.search_query.is_some() {
                "(no matches — esc clears)"
            } else {
                "(queue empty — enqueue with qobi <file>)"
            },
            dim(),
        )));
        frame.render_widget(Paragraph::new(top), area);
        return;
    }
    let wide = width >= WIDE_MIN;
    let items: Vec<ListItem> = state
        .queue
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let line = if wide {
                track_line(row, i, i == state.cursor, width)
            } else {
                simple_line(&row.title, i == state.cursor)
            };
            ListItem::new(line)
        })
        .collect();
    // Highlight by marker + fg only: no reversed/composite style, so transparency holds.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(top.len() as u16), Constraint::Min(1)])
        .split(area);
    if !top.is_empty() {
        frame.render_widget(Paragraph::new(top), chunks[0]);
    }
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::NONE)),
        chunks[1],
    );
}

/// Grouped shortcut reference. Lists the real input map only — the designs
/// sketch keys with no binding (`Ctrl+X`, `j/k`, …), which stay out until a
/// keymap change lands.
const HELP_SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "NAVIGATION",
        &[
            ("tab", "now playing / queue"),
            ("/", "search queue (enter plays, esc exits)"),
            ("↑ / ↓", "navigate"),
            ("enter", "play selected"),
        ],
    ),
    (
        "PLAYBACK",
        &[
            ("space", "play / pause"),
            ("→ / ←", "seek ±5s"),
            ("+ / -", "volume"),
            ("e", "toggle equalizer"),
            ("a", "toggle cover art"),
        ],
    ),
    (
        "QUEUE",
        &[
            ("c", "clear queue"),
            ("s", "shuffle queue"),
            ("r", "repeat off → all → one"),
        ],
    ),
    ("SESSION", &[("?", "this help"), ("q / Ctrl+C", "quit")]),
];

fn render_help(frame: &mut Frame, area: Rect) {
    let mut rows: Vec<Line> = vec![
        Line::from(Span::styled("HELP", dim())),
        Line::from(Span::raw(
            "Qobi is a keyboard-first local music player. The main page stays \
             focused on the current track, album cover, playing status, and \
             equalizer. Search, queue, and help are separated into dedicated \
             sections so the terminal stays readable and uncluttered.",
        )),
        Line::from(""),
    ];
    for (section, keys) in HELP_SECTIONS {
        rows.push(Line::from(Span::styled(*section, dim())));
        for (key, desc) in *keys {
            rows.push(Line::from(vec![
                Span::styled(format!("{key:<12}"), accent()),
                Span::styled((*desc).to_string(), dim()),
            ]));
        }
        rows.push(Line::from(""));
    }
    rows.push(Line::from(Span::styled(
        "Session and volume are saved locally. No telemetry is sent, and the player stays offline-first.",
        dim(),
    )));
    frame.render_widget(Paragraph::new(rows), area);
}

/// Render one full frame. Never sets a background outside half-block art
/// cells: transparency is structural everywhere else.
pub fn render(frame: &mut Frame, state: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());
    frame.render_widget(Paragraph::new(tabs_line(state.view)), chunks[0]);
    match state.view {
        View::NowPlaying => render_now_playing(frame, chunks[1], state),
        View::Queue => render_queue(frame, chunks[1], state),
        View::Help => render_help(frame, chunks[1]),
    }
    let status = match &state.toast {
        Some(t) => Line::from(Span::styled(t.clone(), warn())),
        // Now Playing carries its own centered hints in-content; repeating
        // the generic hint here doubled it (spotted on a 60x15 capture).
        None if state.view == View::NowPlaying => Line::from(""),
        None => Line::from(Span::styled(
            "space play/pause · tab queue · / search · ? help · q quit",
            dim(),
        )),
    };
    frame.render_widget(Paragraph::new(status), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn row(title: &str, format: &str, qid: &str) -> TrackRow {
        TrackRow {
            title: title.to_string(),
            artist: "Someone".to_string(),
            album: "Singles".to_string(),
            year: "2024".to_string(),
            duration: "3:12".to_string(),
            format: format.to_string(),
            qid: qid.to_string(),
        }
    }

    fn demo_state() -> UiState {
        UiState {
            track: Some(TrackInfo {
                title: "Atish Soozi".to_string(),
                artist: "Someone".to_string(),
                album: "Singles".to_string(),
            }),
            playback: Playback::Playing,
            progress: 0.25,
            elapsed_secs: 62,
            total_secs: Some(248),
            queue: vec![row("a", "MP3", "q01"), row("b", "FLAC", "q02")],
            total_tracks: 2,
            repeat_label: "off".to_string(),
            cursor: 0,
            toast: Some("Queued: c.mp3".to_string()),
            eq_enabled: true,
            ..UiState::default()
        }
    }

    fn draw(state: &UiState, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| render(f, state)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn assert_transparent(buf: &ratatui::buffer::Buffer) {
        for cell in buf.content.iter() {
            // Half-block art cells are the one sanctioned bg exception (see
            // art::ArtImage::to_lines); everything else must stay Reset.
            assert!(
                cell.bg == Color::Reset || cell.symbol() == "▀",
                "background fill breaks transparency outside art cells"
            );
        }
    }

    fn text_of(buf: &ratatui::buffer::Buffer) -> String {
        buf.content
            .chunks(buf.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn all_views_transparent_at_80x24() {
        for view in [View::NowPlaying, View::Queue, View::Help] {
            let mut s = demo_state();
            s.view = view;
            let buf = draw(&s, 80, 24);
            assert_transparent(&buf);
        }
    }

    #[test]
    fn transparent_at_all_sizes_including_tiny() {
        let s = demo_state();
        for (w, h) in [(120, 40), (200, 60), (60, 15), (80, 24)] {
            assert_transparent(&draw(&s, w, h));
        }
    }

    #[test]
    fn now_playing_shows_track_status_and_time() {
        let mut s = demo_state();
        s.view = View::NowPlaying;
        s.toast = None;
        let text = text_of(&draw(&s, 100, 30));
        assert!(text.contains("Atish Soozi"), "title missing:\n{text}");
        assert!(text.contains("PLAYING"), "status missing:\n{text}");
        assert!(text.contains("1:02 / 4:08"), "time missing:\n{text}");
        assert!(text.contains("▁"), "eq row missing:\n{text}");
        assert!(text.contains("play/pause"), "hints missing:\n{text}");
    }

    #[test]
    fn eq_line_maps_levels_to_glyphs() {
        let flat = eq_line(&[0.0; EQ_BANDS]);
        let text: String = flat.spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(text, "▁".repeat(EQ_BANDS));
        let hot = eq_line(&[1.0; EQ_BANDS]);
        let text: String = hot.spans.iter().map(|s| s.content.clone()).collect();
        assert_eq!(text, "█".repeat(EQ_BANDS));
        assert_eq!(hot.spans[0].style, accent());
        assert_eq!(flat.spans[0].style, dim());
    }

    #[test]
    fn toast_and_empty_states() {
        let mut s = demo_state();
        s.view = View::Queue;
        let text = text_of(&draw(&s, 100, 24));
        assert!(text.contains("Queued: c.mp3"), "toast missing:\n{text}");
        assert!(text.contains("QUEUE"), "queue header missing:\n{text}");
        assert!(text.contains("repeat off"), "repeat label missing:\n{text}");
        assert!(
            text.contains("q01") && text.contains("q02"),
            "qids missing:\n{text}"
        );
        assert!(text.contains("FLAC"), "format column missing:\n{text}");

        let empty = UiState {
            view: View::Queue,
            ..UiState::default()
        };
        let text = text_of(&draw(&empty, 80, 24));
        assert!(text.contains("queue empty"), "empty state missing:\n{text}");

        let no_track = UiState {
            view: View::NowPlaying,
            ..UiState::default()
        };
        let text = text_of(&draw(&no_track, 100, 30));
        assert!(
            text.contains("nothing playing"),
            "idle state missing:\n{text}"
        );
        assert!(text.contains("STOPPED"), "idle status missing:\n{text}");
    }

    #[test]
    fn help_lists_shortcuts_and_eq_toggle() {
        let mut s = demo_state();
        s.view = View::Help;
        let text = text_of(&draw(&s, 100, 40));
        for key in [
            "space",
            "seek",
            "search",
            "volume",
            "quit",
            "clear",
            "shuffle",
            "repeat",
            "tab",
            "NAVIGATION",
            "PLAYBACK",
            "QUEUE",
            "SESSION",
            "offline-first",
        ] {
            assert!(text.contains(key), "{key} missing from help:\n{text}");
        }
        s.view = View::NowPlaying;
        s.eq_enabled = false;
        let text = text_of(&draw(&s, 100, 30));
        assert!(text.contains("eq off"), "eq toggle missing:\n{text}");
    }

    #[test]
    fn search_query_renders_search_section() {
        let mut s = demo_state();
        s.view = View::Queue;
        s.search_query = Some("al".to_string());
        let buf = draw(&s, 100, 24);
        assert_transparent(&buf);
        let text = text_of(&buf);
        assert!(text.contains("SEARCH"), "search header missing:\n{text}");
        assert!(text.contains("al"), "query missing:\n{text}");
        assert!(
            text.contains("in title, artist, album, path"),
            "scope hint missing:\n{text}"
        );
        assert!(text.contains("matches"), "match count missing:\n{text}");
        assert!(
            text.contains("sort: queue order"),
            "sort label missing:\n{text}"
        );
    }

    #[test]
    fn art_scales_to_fit_instead_of_cropping() {
        use crate::art::{MOSAIC_H, MOSAIC_PIXEL_H, MOSAIC_W, placeholder};
        let mut s = demo_state();
        s.view = View::NowPlaying;
        s.art = placeholder(42, MOSAIC_W, MOSAIC_PIXEL_H).to_lines(true);
        assert_eq!(s.art.len(), MOSAIC_H as usize, "40x40 px → 20 cell rows");
        let cells = |state: &UiState, w: u16, h: u16| {
            text_of(&draw(state, w, h))
                .chars()
                .filter(|&c| c == '▀')
                .count()
        };
        // Tall screen: full 40x20 mosaic, untouched (content 38 − chrome 11
        // leaves room for all 20 rows).
        assert_eq!(cells(&s, 120, 40), (MOSAIC_W * MOSAIC_H) as usize);
        assert_eq!(cells(&s, 80, 30), MOSAIC_W as usize * 17);
        // 80x24 (content 22, chrome 11): scaled 20 → 11 rows, 440 cells —
        // and the full picture survives: first scaled row is the original
        // first row, last scaled row the original last.
        assert_eq!(cells(&s, 80, 24), (MOSAIC_W * 11) as usize);
        let fitted = fit_art(&s.art, 11, 80);
        assert_eq!(fitted.len(), 11);
        assert_eq!(fitted[0].spans, s.art[0].spans, "top must survive");
        assert_eq!(
            fitted[10].spans,
            s.art[MOSAIC_H as usize - 1].spans,
            "bottom must survive"
        );
        // Tiny screen: art hides instead of showing a 2-row smear.
        assert_eq!(cells(&s, 80, 16), 0);
        assert!(fit_art(&s.art, 3, 80).len() <= 3);
        s.art_enabled = false;
        assert_eq!(cells(&s, 120, 40), 0, "toggle must hide art entirely");
    }

    #[test]
    fn fit_art_merges_overwide_rows_by_averaging_colors() {
        use ratatui::style::Color;
        let wide = Line::from(vec![
            Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(200, 0, 0))
                    .bg(Color::Rgb(0, 0, 0)),
            ),
            Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(100, 0, 0))
                    .bg(Color::Rgb(0, 0, 0)),
            ),
            Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(0, 0, 200))
                    .bg(Color::Rgb(0, 0, 0)),
            ),
            Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(0, 0, 100))
                    .bg(Color::Rgb(0, 0, 0)),
            ),
        ]);
        let out = fit_art(&[wide], 4, 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spans.len(), 2, "4 cells → 2 merged cells");
        assert_eq!(out[0].spans[0].content, "▀");
        assert_eq!(
            out[0].spans[0].style.fg,
            Some(Color::Rgb(150, 0, 0)),
            "pair average, not a crop"
        );
        // Transparency invariant holds through merges: bg only on ▀ cells.
        let buf = draw(
            &UiState {
                view: View::NowPlaying,
                art: out,
                ..UiState::default()
            },
            80,
            30,
        );
        assert_transparent(&buf);
    }

    #[test]
    fn fit_art_handles_degenerate_inputs() {
        assert!(fit_art(&[], 10, 80).is_empty());
        assert!(fit_art(&[Line::from("x")], 0, 80).is_empty());
        assert!(fit_art(&[Line::from("x")], 10, 0).is_empty());
        let one = fit_art(&[Line::from("x")], 1, 80);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn long_titles_do_not_break_layout() {
        let mut s = demo_state();
        s.view = View::NowPlaying;
        s.track = Some(TrackInfo {
            title: "x".repeat(300),
            artist: "y".repeat(300),
            album: "z".repeat(300),
        });
        let buf = draw(&s, 80, 24);
        assert_transparent(&buf);
        assert_eq!(buf.area.width, 80);
    }
}
