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

/// Display-only track snapshot. The TUI never owns audio state (Chunk A owns it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// Full UI state for one frame. Plain data — trivially snapshot-testable.
#[derive(Debug, Clone)]
pub struct UiState {
    pub view: View,
    pub track: Option<TrackInfo>,
    pub progress: f32,
    pub elapsed_secs: u64,
    pub total_secs: Option<u64>,
    pub queue: Vec<String>,
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
            progress: 0.0,
            elapsed_secs: 0,
            total_secs: None,
            queue: Vec::new(),
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

/// Single accent, theme-aware by omission: every style below sets fg only,
/// so the terminal background (transparent) always shows through.
fn accent() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn warn() -> Style {
    Style::default().fg(Color::Yellow)
}

fn fmt_time(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Thin progress bar from line-drawing cells (no widget background fill).
fn progress_line(progress: f32, elapsed: u64, total: Option<u64>, width: usize) -> Line<'static> {
    let ratio = progress.clamp(0.0, 1.0);
    let label = format!(
        "{} / {} ",
        fmt_time(elapsed),
        total.map(fmt_time).unwrap_or_else(|| "--:--".to_string())
    );
    let bar_width = width.saturating_sub(label.len()).max(4);
    let filled = (ratio * bar_width as f32).round() as usize;
    let bar: String = (0..bar_width)
        .map(|i| if i < filled { '━' } else { '─' })
        .collect();
    Line::from(vec![
        Span::styled(bar, accent()),
        Span::styled(format!(" {label}"), Style::default()),
    ])
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

/// How many cover rows fit: full 20-row mosaic on tall screens, a
/// 12-row crop on medium ones, collapsed on short screens (art yields
/// first per the responsive rule).
fn art_row_budget(area_height: u16, art_len: usize) -> usize {
    let budget = if area_height >= 26 {
        art_len
    } else if area_height >= 18 {
        12.min(art_len)
    } else {
        0
    };
    budget.min(art_len)
}

fn render_now_playing(frame: &mut Frame, area: Rect, state: &UiState) {
    let mut rows: Vec<Line> = Vec::new();
    // Cover mosaic: collapses on short screens (responsive rule).
    if state.art_enabled && !state.art.is_empty() {
        let take = art_row_budget(area.height, state.art.len());
        rows.extend(state.art.iter().take(take).cloned().map(|l| l.centered()));
        if take > 0 {
            rows.push(Line::from(""));
        }
    }
    match &state.track {
        Some(track) => {
            rows.push(Line::from(Span::styled(
                track.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            rows.push(Line::from(Span::styled(
                format!("{} — {}", track.artist, track.album),
                accent(),
            )));
        }
        None => rows.push(Line::from(Span::styled("nothing playing", dim()))),
    }
    rows.push(Line::from(""));
    rows.push(progress_line(
        state.progress,
        state.elapsed_secs,
        state.total_secs,
        area.width as usize,
    ));
    rows.push(Line::from(""));
    rows.push(if state.eq_enabled {
        eq_line(&state.eq_bars)
    } else {
        Line::from(Span::styled("(eq off)", dim()))
    });
    frame.render_widget(Paragraph::new(rows), area);
}

fn render_queue(frame: &mut Frame, area: Rect, state: &UiState) {
    let mut top: Vec<Line> = Vec::new();
    if let Some(q) = &state.search_query {
        top.push(Line::from(vec![
            Span::styled("/", dim()),
            Span::styled(q.clone(), Style::default().add_modifier(Modifier::BOLD)),
        ]));
    }
    if state.queue.is_empty() {
        top.push(Line::from(Span::styled(
            "(queue empty — enqueue with qobi <file>)",
            dim(),
        )));
        frame.render_widget(Paragraph::new(top), area);
        return;
    }
    let items: Vec<ListItem> = state
        .queue
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let line = if i == state.cursor {
                Line::from(vec![
                    Span::styled("▸ ", accent()),
                    Span::styled(name.clone(), accent()),
                ])
            } else {
                Line::from(format!("  {name}"))
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

const HELP_ROWS: &[(&str, &str)] = &[
    ("space", "play / pause"),
    ("→ / ←", "seek ±5s"),
    ("↑ / ↓", "navigate"),
    ("Enter", "play selected"),
    ("Tab", "now playing / queue"),
    ("/", "search"),
    ("+", "- volume"),
    ("e", "toggle equalizer"),
    ("a", "toggle art"),
    ("c", "clear queue"),
    ("s", "shuffle queue"),
    ("r", "repeat off/all/one"),
    ("?", "this help"),
    ("q / Ctrl+C", "quit"),
];

fn render_help(frame: &mut Frame, area: Rect) {
    let rows: Vec<Line> = HELP_ROWS
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(format!("{key:<12}"), accent()),
                Span::raw(*desc),
            ])
        })
        .collect();
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

    fn demo_state() -> UiState {
        UiState {
            track: Some(TrackInfo {
                title: "Atish Soozi".to_string(),
                artist: "Someone".to_string(),
                album: "Singles".to_string(),
            }),
            progress: 0.25,
            elapsed_secs: 62,
            total_secs: Some(248),
            queue: vec!["a.mp3".to_string(), "b.flac".to_string()],
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
    fn now_playing_shows_track_and_progress() {
        let mut s = demo_state();
        s.view = View::NowPlaying;
        s.toast = None;
        let text = text_of(&draw(&s, 80, 24));
        assert!(text.contains("Atish Soozi"), "title missing:\n{text}");
        assert!(text.contains("1:02 / 4:08"), "time missing:\n{text}");
        assert!(text.contains("▁"), "eq row missing:\n{text}");
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
        let text = text_of(&draw(&s, 80, 24));
        assert!(text.contains("Queued: c.mp3"), "toast missing:\n{text}");
        assert!(
            text.contains("a.mp3") && text.contains("b.flac"),
            "queue missing:\n{text}"
        );

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
        let text = text_of(&draw(&no_track, 80, 24));
        assert!(
            text.contains("nothing playing"),
            "idle state missing:\n{text}"
        );
    }

    #[test]
    fn help_lists_shortcuts_and_eq_toggle() {
        let mut s = demo_state();
        s.view = View::Help;
        let text = text_of(&draw(&s, 80, 24));
        for key in [
            "space", "seek", "search", "volume", "quit", "clear", "shuffle", "repeat", "Tab",
        ] {
            assert!(text.contains(key), "{key} missing from help:\n{text}");
        }
        s.view = View::NowPlaying;
        s.eq_enabled = false;
        let text = text_of(&draw(&s, 80, 24));
        assert!(text.contains("eq off"), "eq toggle missing:\n{text}");
    }

    #[test]
    fn search_query_renders_in_queue() {
        let mut s = demo_state();
        s.view = View::Queue;
        s.search_query = Some("al".to_string());
        let buf = draw(&s, 80, 24);
        assert_transparent(&buf);
        let text = text_of(&buf);
        assert!(text.contains("/al"), "search row missing:\n{text}");
    }

    #[test]
    fn art_renders_centered_and_transparent() {
        use crate::art::{MOSAIC_H, MOSAIC_PIXEL_H, MOSAIC_W, placeholder};
        let mut s = demo_state();
        s.view = View::NowPlaying;
        s.art = placeholder(42, MOSAIC_W, MOSAIC_PIXEL_H).to_lines(true);
        assert_eq!(s.art.len(), MOSAIC_H as usize, "40x40 px → 20 cell rows");
        // Tall screen: full 40x20 mosaic.
        let buf = draw(&s, 80, 30);
        assert_transparent(&buf);
        let on = text_of(&buf).chars().filter(|&c| c == '▀').count();
        assert_eq!(on, (MOSAIC_W * MOSAIC_H) as usize, "mosaic cells missing");
        // Medium screen: 12-row crop.
        let mid = text_of(&draw(&s, 80, 24))
            .chars()
            .filter(|&c| c == '▀')
            .count();
        assert_eq!(mid, (MOSAIC_W * 12) as usize, "medium crop missing");
        // Short screen: art collapses entirely.
        let tiny = text_of(&draw(&s, 80, 16))
            .chars()
            .filter(|&c| c == '▀')
            .count();
        assert_eq!(tiny, 0, "short screen must collapse art");
        s.art_enabled = false;
        let off = text_of(&draw(&s, 80, 30))
            .chars()
            .filter(|&c| c == '▀')
            .count();
        assert_eq!(off, 0, "toggle must hide art entirely");
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
