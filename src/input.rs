use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// UI intents. Pure data: [`map_key`] converts terminal keys, the controller
/// (`app`) executes. `Ctrl+P` / `Ctrl+X` from the early spec are deliberately
/// unmapped — they conflict with shell line discipline (Architecture §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    TogglePlay,
    SeekForward,
    SeekBackward,
    CursorUp,
    CursorDown,
    PlaySelected,
    VolumeUp,
    VolumeDown,
    ToggleEq,
    ToggleArt,
    ToggleHelp,
    StartSearch,
    SearchChar(char),
    SearchBackspace,
    ExitSearch,
    ClearQueue,
    ShuffleQueue,
    CycleRepeat,
    Quit,
    Ignored,
}

/// Map a key in normal mode.
pub fn map_key(key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            _ => Action::Ignored,
        };
    }
    match key.code {
        KeyCode::Char(' ') => Action::TogglePlay,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::VolumeUp,
        KeyCode::Char('-') => Action::VolumeDown,
        KeyCode::Char('/') => Action::StartSearch,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('e') => Action::ToggleEq,
        KeyCode::Char('a') => Action::ToggleArt,
        KeyCode::Char('c') => Action::ClearQueue,
        KeyCode::Char('s') => Action::ShuffleQueue,
        KeyCode::Char('r') => Action::CycleRepeat,
        KeyCode::Up => Action::CursorUp,
        KeyCode::Down => Action::CursorDown,
        KeyCode::Left => Action::SeekBackward,
        KeyCode::Right => Action::SeekForward,
        KeyCode::Enter => Action::PlaySelected,
        KeyCode::Esc => Action::Ignored,
        _ => Action::Ignored,
    }
}

/// Map a key while search mode is active: typing edits the query,
/// `Enter`/`Esc` leave search mode, arrows still navigate results.
pub fn map_search_key(key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            _ => Action::Ignored,
        };
    }
    match key.code {
        KeyCode::Esc | KeyCode::Enter => Action::ExitSearch,
        KeyCode::Backspace => Action::SearchBackspace,
        KeyCode::Char(c) => Action::SearchChar(c),
        KeyCode::Up => Action::CursorUp,
        KeyCode::Down => Action::CursorDown,
        _ => Action::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn full_key_table() {
        let cases = [
            (key(KeyCode::Char(' ')), Action::TogglePlay),
            (key(KeyCode::Right), Action::SeekForward),
            (key(KeyCode::Left), Action::SeekBackward),
            (key(KeyCode::Up), Action::CursorUp),
            (key(KeyCode::Down), Action::CursorDown),
            (key(KeyCode::Enter), Action::PlaySelected),
            (key(KeyCode::Char('/')), Action::StartSearch),
            (key(KeyCode::Char('+')), Action::VolumeUp),
            (key(KeyCode::Char('=')), Action::VolumeUp),
            (key(KeyCode::Char('-')), Action::VolumeDown),
            (key(KeyCode::Char('e')), Action::ToggleEq),
            (key(KeyCode::Char('a')), Action::ToggleArt),
            (key(KeyCode::Char('c')), Action::ClearQueue),
            (key(KeyCode::Char('s')), Action::ShuffleQueue),
            (key(KeyCode::Char('r')), Action::CycleRepeat),
            (key(KeyCode::Char('?')), Action::ToggleHelp),
            (key(KeyCode::Char('q')), Action::Quit),
            (ctrl(KeyCode::Char('c')), Action::Quit),
        ];
        for (k, expected) in cases {
            assert_eq!(map_key(k), expected, "key {k:?}");
        }
    }

    #[test]
    fn rejected_and_unknown_keys_are_ignored() {
        // Ctrl+P / Ctrl+X must never trigger playback actions.
        assert_eq!(map_key(ctrl(KeyCode::Char('p'))), Action::Ignored);
        assert_eq!(map_key(ctrl(KeyCode::Char('x'))), Action::Ignored);
        assert_eq!(map_key(key(KeyCode::F(1))), Action::Ignored);
        assert_eq!(map_key(key(KeyCode::Esc)), Action::Ignored);
    }

    #[test]
    fn search_mode_edits_query() {
        assert_eq!(
            map_search_key(key(KeyCode::Char('a'))),
            Action::SearchChar('a')
        );
        assert_eq!(
            map_search_key(key(KeyCode::Char(' '))),
            Action::SearchChar(' ')
        );
        assert_eq!(
            map_search_key(key(KeyCode::Backspace)),
            Action::SearchBackspace
        );
        assert_eq!(map_search_key(key(KeyCode::Enter)), Action::ExitSearch);
        assert_eq!(map_search_key(key(KeyCode::Esc)), Action::ExitSearch);
        assert_eq!(map_search_key(key(KeyCode::Up)), Action::CursorUp);
        assert_eq!(map_search_key(ctrl(KeyCode::Char('c'))), Action::Quit);
    }
}
