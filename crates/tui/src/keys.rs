// SPDX-License-Identifier: AGPL-3.0-or-later
//! Keys (`docs/design/synthesis-v1.md` §6.11): terminal key events become [`Cmd`]s, and the
//! help overlay lists them. Remapping (`[tui.keys]`) arrives later; the default map is here.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// One opinion key's meaning (`synthesis-v1.md` §6.11 "Opinions").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `u`.
    Up,
    /// `U`.
    StrongUp,
    /// `d`.
    Down,
    /// `D`.
    StrongDown,
    /// `a`.
    Agree,
    /// `A`.
    StrongAgree,
    /// `z`.
    Disagree,
    /// `Z`.
    StrongDisagree,
    /// `0`.
    Clear,
}

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
    /// `u`, `U`, `d`, `D`, `a`, `A`, `z`, `Z`, `0`: an opinion on the selected item.
    Opinion(Op),
    /// `Backspace`: undo the pending write (the 4 s undo window).
    Undo,
    /// `x`: open the react palette on the selected item.
    ReactPalette,
    /// `E`: show the version history of the selected item (M4).
    History,
    /// `Tab` (inside the react palette): switch the stance.
    Stance,
    /// `m`: open the mod menu on the selected item (or its author).
    ModMenu,
    /// `N`: open the Notifications screen (`synthesis-v1.md` §6.11 lists it; `n`/`N` are
    /// reserved for search matches there, and search is not built yet).
    Notifications,
    /// `M` (on the Notifications screen): mark every listed notification read.
    MarkAllRead,
    /// `a` (on the Notifications screen): toggle unread-only / all.
    ToggleAll,
    /// `?`: help.
    Help,
    /// `:`: the command line overlay (`synthesis-v1.md` §6.11 Overlays).
    Command,
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
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => Cmd::Back,
        KeyCode::Char('q') => Cmd::BackOrQuit,
        KeyCode::Char(' ') => Cmd::Collapse,
        KeyCode::Char('p') => Cmd::Parent,
        KeyCode::Char('r') => Cmd::Reply,
        KeyCode::Char('c') => Cmd::CommentOnPost,
        KeyCode::Char('P') => Cmd::NewPost,
        KeyCode::Char('R') => Cmd::Sync,
        KeyCode::Char('s') => Cmd::Sort,
        KeyCode::Char('u') => Cmd::Opinion(Op::Up),
        KeyCode::Char('U') => Cmd::Opinion(Op::StrongUp),
        KeyCode::Char('d') => Cmd::Opinion(Op::Down),
        KeyCode::Char('D') => Cmd::Opinion(Op::StrongDown),
        KeyCode::Char('a') => Cmd::Opinion(Op::Agree),
        KeyCode::Char('A') => Cmd::Opinion(Op::StrongAgree),
        KeyCode::Char('z') => Cmd::Opinion(Op::Disagree),
        KeyCode::Char('Z') => Cmd::Opinion(Op::StrongDisagree),
        KeyCode::Char('0') => Cmd::Opinion(Op::Clear),
        KeyCode::Backspace => Cmd::Undo,
        KeyCode::Char('x') => Cmd::ReactPalette,
        KeyCode::Char('E') => Cmd::History,
        KeyCode::Tab => Cmd::Stance,
        KeyCode::Char('m') => Cmd::ModMenu,
        KeyCode::Char('N') => Cmd::Notifications,
        KeyCode::Char('M') => Cmd::MarkAllRead,
        KeyCode::Char('?') => Cmd::Help,
        KeyCode::Char(':') => Cmd::Command,
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
            ("s", "sort: magic, top, new, old"),
            ("Tab", "posts / activity"),
            ("P", "new post in $EDITOR"),
            ("R", "sync and show new items"),
        ],
    ),
    (
        "Opinions",
        &[
            ("u U", "upvote / strong upvote"),
            ("d D", "downvote / strong downvote"),
            ("a A", "agree / strongly agree"),
            ("z Z", "disagree / strongly disagree"),
            ("0", "clear your votes on the item"),
            ("x", "react palette (Tab: for/against)"),
            ("Backspace", "undo the pending write (4 s window)"),
        ],
    ),
    (
        "Other",
        &[
            ("m", "mod menu (queue, log, actions)"),
            (":", "command line (any M3 verb; : help)"),
            ("E", "versions of the selected item"),
            ("N", "notifications"),
            ("M", "mark all notifications read"),
            ("?", "this help"),
            ("Ctrl-c", "quit"),
        ],
    ),
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
        // the M3 opinion keys (synthesis §6.11)
        assert_eq!(c("u"), Some(Cmd::Opinion(Op::Up)));
        assert_eq!(c("U"), Some(Cmd::Opinion(Op::StrongUp)));
        assert_eq!(c("d"), Some(Cmd::Opinion(Op::Down)));
        assert_eq!(c("D"), Some(Cmd::Opinion(Op::StrongDown)));
        assert_eq!(c("a"), Some(Cmd::Opinion(Op::Agree)));
        assert_eq!(c("A"), Some(Cmd::Opinion(Op::StrongAgree)));
        assert_eq!(c("z"), Some(Cmd::Opinion(Op::Disagree)));
        assert_eq!(c("Z"), Some(Cmd::Opinion(Op::StrongDisagree)));
        assert_eq!(c("0"), Some(Cmd::Opinion(Op::Clear)));
        assert_eq!(c("Backspace"), Some(Cmd::Undo));
        assert_eq!(c("x"), Some(Cmd::ReactPalette));
        assert_eq!(c("Tab"), Some(Cmd::Stance));
        assert_eq!(c("m"), Some(Cmd::ModMenu));
        assert_eq!(c("N"), Some(Cmd::Notifications));
        assert_eq!(c("M"), Some(Cmd::MarkAllRead));
        // the `:` command line (synthesis §6.11 Overlays)
        assert_eq!(c(":"), Some(Cmd::Command));
        assert_eq!(c("Ctrl-x"), None);
        assert!(parse_key("Ctrl-").is_none());
    }
}
