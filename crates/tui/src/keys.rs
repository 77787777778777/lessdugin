// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keys (`docs/design/synthesis-v1.md` §6.11): terminal key events become [`Cmd`]s, and the
//! help overlay lists them. Remapping (`[tui.keys]`) arrives later; the default map is here.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// A command, independent of the key that gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// `j`, `↓`.
    Down,
    /// `k`, `↑`.
    Up,
    /// `g`, `Home`.
    Top,
    /// `G`, `End`.
    Bottom,
    /// `Ctrl-d`, `PageDown`.
    HalfDown,
    /// `Ctrl-u`, `PageUp`.
    HalfUp,
    /// `Enter`, `l`, `→`.
    Open,
    /// `h`, `←`, `Esc`.
    Back,
    /// `q`: back, or quit on the home screen.
    BackOrQuit,
    /// `Ctrl-c`.
    Quit,
    /// `Space`: collapse or expand a comment's replies.
    Collapse,
    /// `p`: go to the parent comment.
    Parent,
    /// `r`: reply to the selected comment (on the post: comment on the post).
    Reply,
    /// `c`: comment on the post.
    CommentOnPost,
    /// `P`: new post.
    NewPost,
    /// `R`: sync, and show what a live update brought.
    Sync,
    /// `s`: next sort.
    Sort,
    /// `?`: help.
    Help,
}

/// The command of a key press, if any (key releases and repeats on some platforms are ignored).
pub fn command(k: &KeyEvent) -> Option<Cmd> {
    if k.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    Some(match k.code {
        KeyCode::Char('c') if ctrl => Cmd::Quit,
        KeyCode::Char('d') if ctrl => Cmd::HalfDown,
        KeyCode::Char('u') if ctrl => Cmd::HalfUp,
        _ if ctrl => return None,
        KeyCode::Char('j') | KeyCode::Down => Cmd::Down,
        KeyCode::Char('k') | KeyCode::Up => Cmd::Up,
        KeyCode::Char('g') | KeyCode::Home => Cmd::Top,
        KeyCode::Char('G') | KeyCode::End => Cmd::Bottom,
        KeyCode::PageDown => Cmd::HalfDown,
        KeyCode::PageUp => Cmd::HalfUp,
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Cmd::Open,
        // Backspace is kept free: synthesis §6.11 gives it to undoing the pending write (M3).
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => Cmd::Back,
        KeyCode::Char('q') => Cmd::BackOrQuit,
        KeyCode::Char(' ') => Cmd::Collapse,
        KeyCode::Char('p') => Cmd::Parent,
        KeyCode::Char('r') => Cmd::Reply,
        KeyCode::Char('c') => Cmd::CommentOnPost,
        KeyCode::Char('P') => Cmd::NewPost,
        KeyCode::Char('R') => Cmd::Sync,
        KeyCode::Char('s') => Cmd::Sort,
        KeyCode::Char('?') => Cmd::Help,
        _ => return None,
    })
}

/// Parses a key name as tests and the headless driver write them: a character (`j`, `G`, `?`),
/// `Enter`, `Esc`, `Space`, `Tab`, `Backspace`, `Up`/`Down`/`Left`/`Right`, `Home`, `End`,
/// `PageUp`, `PageDown`, or `Ctrl-<char>`.
pub fn parse_key(s: &str) -> Option<KeyEvent> {
    let (code, mods) = match s {
        "Enter" => (KeyCode::Enter, KeyModifiers::NONE),
        "Esc" => (KeyCode::Esc, KeyModifiers::NONE),
        "Space" => (KeyCode::Char(' '), KeyModifiers::NONE),
        "Tab" => (KeyCode::Tab, KeyModifiers::NONE),
        "Backspace" => (KeyCode::Backspace, KeyModifiers::NONE),
        "Up" => (KeyCode::Up, KeyModifiers::NONE),
        "Down" => (KeyCode::Down, KeyModifiers::NONE),
        "Left" => (KeyCode::Left, KeyModifiers::NONE),
        "Right" => (KeyCode::Right, KeyModifiers::NONE),
        "Home" => (KeyCode::Home, KeyModifiers::NONE),
        "End" => (KeyCode::End, KeyModifiers::NONE),
        "PageUp" => (KeyCode::PageUp, KeyModifiers::NONE),
        "PageDown" => (KeyCode::PageDown, KeyModifiers::NONE),
        _ => {
            if let Some(c) = s.strip_prefix("Ctrl-") {
                let mut it = c.chars();
                let (Some(c), None) = (it.next(), it.next()) else {
                    return None;
                };
                (KeyCode::Char(c), KeyModifiers::CONTROL)
            } else {
                let mut it = s.chars();
                let (Some(c), None) = (it.next(), it.next()) else {
                    return None;
                };
                let mods = if c.is_ascii_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                };
                (KeyCode::Char(c), mods)
            }
        }
    };
    Some(KeyEvent::new(code, mods))
}

/// The help overlay: `(keys, what)` rows by group.
pub const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation",
        &[
            ("j k  Down Up", "next / previous"),
            ("g G", "first / last"),
            ("Ctrl-d Ctrl-u", "half a page down / up"),
            ("Enter l", "open"),
            ("h Esc q", "back (q on the home screen quits)"),
        ],
    ),
    (
        "Threads",
        &[
            ("Space", "collapse / expand replies"),
            ("p", "parent comment"),
            ("r", "reply in $EDITOR"),
            ("c", "comment on the post in $EDITOR"),
        ],
    ),
    (
        "Forums",
        &[
            ("s", "sort: new, old, top"),
            ("P", "new post in $EDITOR"),
            ("R", "sync and show new items"),
        ],
    ),
    ("Other", &[("?", "this help"), ("Ctrl-c", "quit")]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map() {
        let c = |s: &str| command(&parse_key(s).unwrap());
        assert_eq!(c("j"), Some(Cmd::Down));
        assert_eq!(c("Ctrl-d"), Some(Cmd::HalfDown));
        assert_eq!(c("Ctrl-c"), Some(Cmd::Quit));
        assert_eq!(c("G"), Some(Cmd::Bottom));
        assert_eq!(c("P"), Some(Cmd::NewPost));
        assert_eq!(c("R"), Some(Cmd::Sync));
        assert_eq!(c("Space"), Some(Cmd::Collapse));
        assert_eq!(c("Esc"), Some(Cmd::Back));
        // reserved for undoing the pending write (synthesis §6.11, M3)
        assert_eq!(c("Backspace"), None);
        assert_eq!(c("Ctrl-x"), None);
        assert_eq!(c("x"), None);
        assert!(parse_key("Ctrl-").is_none());
    }
}
