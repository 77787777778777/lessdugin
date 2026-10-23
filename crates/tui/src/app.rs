// SPDX-License-Identifier: AGPL-3.0-or-later
//! Application state, the update function and the screen stack (`synthesis-v1.md` §6.11).
//!
//! The TUI is an Elm-style loop: [`State::update`] takes an [`Action`] (a key, a resize, an
//! engine event, the result of an engine call or of the editor) and returns the [`Effect`]s to
//! run; it never does I/O itself. The view ([`crate::view`]) is a pure function of the state.
//! A runtime ([`crate::runtime`] on a real terminal, [`crate::headless`] in tests) runs the
//! effects (engine calls as tasks, `$EDITOR` with the terminal suspended) and feeds their
//! results back as actions.
//!
//! **Live updates never move the cursor.** A reload that the user did not ask for (an engine
//! `Changed` event, the end of a background sync) replaces the screen's data only when its
//! items are the same, in the same order (a badge or a body changed); otherwise it is kept as
//! pending and the screen shows a banner, "3 new — R". `R` shows it (and syncs again). Loads
//! the user asked for (opening a screen, `R`, a write) apply at once, keeping the selection on
//! the same item.

use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::KeyEvent;
use lessdugin_core::dto::{
    AccountView, CommentDraft, Event, ForumSummary, ForumView, PostDraft, PostList, PostSort,
    SyncReport, ThreadRoot, ThreadView, Topic, WriteState, WriteTicket,
};
use lessdugin_core::engine::EngineError;

use crate::body::Body;
use crate::editor;
use crate::keys::{self, Cmd};
use crate::text::{TermText, actor, cat, lit, post_title, tt};
use crate::thread::{self, Layout, Pos};

/// Identifies a screen instance (results for a screen that was closed are dropped).
pub type ScreenId = u64;

/// Who asked for a load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The user (opening a screen, `R`, a write): applied at once.
    User,
    /// A live update: applied only if it moves nothing, else kept as pending.
    Live,
}

/// The Home screen's data.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeData {
    /// The followed forums.
    pub forums: Vec<ForumSummary>,
}

/// The Forum screen's data.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumData {
    /// The forum header.
    pub view: ForumView,
    /// Its posts.
    pub list: PostList,
}

/// The Thread screen's data: the engine's `ThreadView` exactly as `get_thread` returned it,
/// and the bodies to show.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadData {
    /// The view (dto/1).
    pub view: ThreadView,
    /// The bodies by item uri.
    pub bodies: BTreeMap<String, Body>,
}

/// What a sync covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncTarget {
    /// These forums, one after the other.
    Forums(Vec<String>),
    /// One forum.
    Forum(String),
    /// A thread (and, when `forum` is given, its whole forum after it).
    Thread {
        /// The post or comment.
        uri: String,
        /// The forum to sync after the thread.
        forum: Option<String>,
    },
}

/// What is being written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compose {
    /// A comment on `root`, replying to `parent`.
    Comment {
        /// The thread's post.
        root: String,
        /// The comment replied to.
        parent: Option<String>,
    },
    /// A new post in `forum`.
    Post {
        /// The forum.
        forum: String,
    },
}

impl Compose {
    fn noun(&self) -> &'static str {
        match self {
            Compose::Comment {
                parent: Some(_), ..
            } => "reply",
            Compose::Comment { parent: None, .. } => "comment",
            Compose::Post { .. } => "post",
        }
    }

    /// The key that opens the editor for it again.
    fn again(&self) -> &'static str {
        match self {
            Compose::Comment {
                parent: Some(_), ..
            } => "r",
            Compose::Comment { parent: None, .. } => "c",
            Compose::Post { .. } => "P",
        }
    }
}

/// A write queued from this TUI, until it is sent (or fails).
#[derive(Clone, Debug)]
struct OwnWrite {
    /// The screen on top when it was queued: its next change is the write, shown at once.
    screen: ScreenId,
    /// What was written, and the text (offered again if the write fails).
    compose: Compose,
    text: String,
}

/// An `$EDITOR` session to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorRequest {
    /// What is written.
    pub compose: Compose,
    /// The template (`editor::comment_template`, `editor::post_template`).
    pub template: String,
}

/// Something for the runtime to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `followed()`.
    LoadHome {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
    },
    /// `forum_open` and `list_posts` (filter frontpage, the M3 default).
    LoadForum {
        /// The screen.
        screen: ScreenId,
        /// The forum uri.
        forum: String,
        /// The sort.
        sort: PostSort,
        /// Who asked.
        origin: Origin,
    },
    /// `get_thread` and the bodies.
    LoadThread {
        /// The screen.
        screen: ScreenId,
        /// The post or comment uri.
        uri: String,
        /// Who asked.
        origin: Origin,
    },
    /// `sync_forum` / `sync_thread`.
    Sync {
        /// What to sync.
        target: SyncTarget,
        /// True when the user asked (`R`).
        user: bool,
    },
    /// Run `$EDITOR` with the terminal suspended.
    Editor(EditorRequest),
    /// `comment(draft)`.
    Comment {
        /// The draft.
        draft: CommentDraft,
    },
    /// `publish(draft)`.
    Publish {
        /// The draft.
        draft: PostDraft,
    },
    /// `writes()`: the outbox as it is at start (writes queued or failed before this
    /// session).
    LoadWrites,
    /// Leave the TUI.
    Quit,
}

/// Something that happened.
#[derive(Clone, Debug)]
pub enum Action {
    /// A key press.
    Key(KeyEvent),
    /// The terminal's new size.
    Resize(u16, u16),
    /// An engine event.
    Engine(Event),
    /// [`Effect::LoadHome`] finished.
    HomeLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The data.
        result: Result<HomeData, EngineError>,
    },
    /// [`Effect::LoadForum`] finished.
    ForumLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The data.
        result: Result<Box<ForumData>, EngineError>,
    },
    /// [`Effect::LoadThread`] finished.
    ThreadLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The data.
        result: Result<Box<ThreadData>, EngineError>,
    },
    /// [`Effect::Sync`] finished.
    Synced {
        /// What was synced.
        target: SyncTarget,
        /// True when the user asked.
        user: bool,
        /// When it finished (host clock).
        at_ms: i64,
        /// The outcome per forum (or per item when its forum is unknown).
        results: Vec<(String, Result<SyncReport, EngineError>)>,
    },
    /// The editor exited: the file's text, or why nothing came back.
    Edited {
        /// What was written.
        compose: Compose,
        /// The file's whole text.
        result: Result<String, editor::EditError>,
    },
    /// [`Effect::LoadWrites`] finished.
    WritesLoaded(Result<Vec<WriteTicket>, EngineError>),
    /// [`Effect::Comment`] or [`Effect::Publish`] finished.
    Queued {
        /// What was written.
        compose: Compose,
        /// The text (kept for another attempt when queuing failed).
        text: String,
        /// The ticket.
        result: Result<WriteTicket, EngineError>,
    },
}

/// The tone of a status message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Neutral.
    Info,
    /// Success.
    Good,
    /// A warning.
    Warn,
    /// An error.
    Error,
}

/// The status line's message.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// The text.
    pub text: TermText,
    /// Its tone.
    pub tone: Tone,
}

/// A forum's sync state as the TUI saw it.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncStatus {
    /// Running: repos synced so far of those planned so far.
    Running {
        /// Done.
        done: u32,
        /// Planned.
        total: u32,
    },
    /// Finished.
    Done {
        /// When.
        at_ms: i64,
        /// Not every repo was synced.
        partial: bool,
        /// Repos complete.
        complete: u32,
        /// Repos in the forum.
        total: u32,
    },
    /// Finished, seen from the engine's events only (a sync this TUI did not start).
    Finished,
    /// Failed.
    Failed {
        /// When.
        at_ms: i64,
        /// Why.
        message: TermText,
    },
}

/// A list's cursor and first shown item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListPos {
    /// The selected item.
    pub cursor: usize,
    /// The first item shown.
    pub top: usize,
}

impl ListPos {
    /// Applies a navigation command to a list of `len` items, `visible` of which show.
    pub fn apply(&mut self, cmd: Cmd, len: usize, visible: usize) {
        let visible = visible.max(1);
        let last = len.saturating_sub(1);
        self.cursor = match cmd {
            Cmd::Down => (self.cursor + 1).min(last),
            Cmd::Up => self.cursor.saturating_sub(1),
            Cmd::Top => 0,
            Cmd::Bottom => last,
            Cmd::HalfDown => (self.cursor + (visible / 2).max(1)).min(last),
            Cmd::HalfUp => self.cursor.saturating_sub((visible / 2).max(1)),
            _ => return,
        };
        self.reveal(len, visible);
    }

    /// Clamps the cursor and scrolls the least to show it.
    pub fn reveal(&mut self, len: usize, visible: usize) {
        let visible = visible.max(1);
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.cursor = self.cursor.min(len - 1);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + visible {
            self.top = self.cursor + 1 - visible;
        }
        self.top = self.top.min(len.saturating_sub(visible));
    }
}

/// Rows per item in the Home and Forum lists.
pub const LIST_ROWS: usize = 2;

/// The Home screen: followed forums with their sync status.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeScreen {
    /// The data, once loaded.
    pub data: Option<HomeData>,
    /// A live update waiting for `R`.
    pub pending: Option<HomeData>,
    /// The cursor.
    pub pos: ListPos,
}

impl HomeScreen {
    fn uris(d: &HomeData) -> Vec<String> {
        d.forums.iter().map(|f| f.uri.clone()).collect()
    }

    /// The selected forum.
    pub fn selected(&self) -> Option<&ForumSummary> {
        self.data.as_ref()?.forums.get(self.pos.cursor)
    }
}

/// The Forum screen: the post list.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumScreen {
    /// The forum uri.
    pub uri: String,
    /// The sort (`s`: new, old, top).
    pub sort: PostSort,
    /// The data, once loaded.
    pub data: Option<ForumData>,
    /// A live update waiting for `R`.
    pub pending: Option<ForumData>,
    /// The cursor.
    pub pos: ListPos,
}

impl ForumScreen {
    fn uris(d: &ForumData) -> Vec<String> {
        d.list.items.iter().map(|p| p.uri.clone()).collect()
    }

    /// The selected post.
    pub fn selected(&self) -> Option<&lessdugin_core::dto::PostSummary> {
        self.data.as_ref()?.list.items.get(self.pos.cursor)
    }

    /// The forum's name, once known.
    pub fn name(&self) -> Option<&str> {
        self.data.as_ref().map(|d| d.view.forum.name.as_str())
    }
}

/// The Thread screen: the post and its comments.
#[derive(Clone, Debug, Default)]
pub struct ThreadScreen {
    /// The uri opened (a post, or a comment of the thread).
    pub uri: String,
    /// The data, once loaded.
    pub data: Option<ThreadData>,
    /// A live update waiting for `R`.
    pub pending: Option<ThreadData>,
    /// Comments whose replies are folded.
    pub collapsed: BTreeSet<String>,
    /// The selection and scroll.
    pub pos: Pos,
    /// The layout for the current width.
    pub layout: Layout,
}

impl ThreadScreen {
    /// Lays the thread out again (new data, width or folds), keeping the selected item and
    /// its row on screen.
    pub fn relayout(&mut self, width: u16, h: usize) {
        let Some(data) = &self.data else {
            self.layout = Layout {
                width,
                ..Layout::default()
            };
            self.pos = Pos::default();
            return;
        };
        let before = self
            .layout
            .items
            .get(self.pos.selected)
            .map(|i| (i.uri.clone(), i.start as isize - self.pos.scroll as isize));
        self.layout = thread::layout(data, &self.collapsed, width);
        if let Some((uri, row)) = before {
            if let Some(i) = self.layout.index_of(&uri) {
                self.pos.selected = i;
            }
            self.pos.clamp(&self.layout, h);
            if let Some(it) = self.layout.items.get(self.pos.selected) {
                self.pos.scroll = usize::try_from(it.start as isize - row).unwrap_or(0);
            }
        }
        self.pos.clamp(&self.layout, h);
    }

    /// The root post's uri and its forum, when the post is available.
    pub fn post(&self) -> Option<&lessdugin_core::dto::PostView> {
        match &self.data.as_ref()?.view.post {
            ThreadRoot::Post(p) => Some(p),
            ThreadRoot::Missing { .. } => None,
        }
    }

    fn receive(&mut self, new: ThreadData, origin: Origin, width: u16, h: usize) {
        let first = self.data.is_none();
        if !first
            && origin == Origin::Live
            && self
                .data
                .as_ref()
                .is_some_and(|cur| thread::item_uris(cur) != thread::item_uris(&new))
        {
            self.pending = Some(new);
            return;
        }
        self.data = Some(new);
        self.pending = None;
        self.relayout(width, h);
        if first {
            // a comment link opens its thread on that comment
            if let Some(i) = self.layout.index_of(&self.uri) {
                self.pos.selected = i;
                self.pos.reveal(&self.layout, h, false);
            }
        }
    }

    fn apply_pending(&mut self, width: u16, h: usize) -> bool {
        match self.pending.take() {
            Some(p) => {
                self.data = Some(p);
                self.relayout(width, h);
                self.pos.reveal(&self.layout, h, false);
                true
            }
            None => false,
        }
    }
}

/// A screen kind.
#[derive(Clone, Debug)]
pub enum ScreenKind {
    /// Home.
    Home(HomeScreen),
    /// A forum.
    Forum(Box<ForumScreen>),
    /// A thread.
    Thread(Box<ThreadScreen>),
}

/// A screen on the stack.
#[derive(Clone, Debug)]
pub struct Screen {
    /// Its id.
    pub id: ScreenId,
    /// What it shows.
    pub kind: ScreenKind,
    /// Loads in flight.
    pub loading: u32,
    /// A live reload was asked for while loading: reload when it finishes.
    pub dirty: bool,
    /// Its data changed while another screen was on top: reload when it shows again.
    pub stale: bool,
    /// The sync started when it opened is running and the user has not moved yet: live
    /// reloads apply at once (nothing to keep in place yet).
    pub settling: bool,
    /// The last load's error.
    pub error: Option<EngineError>,
}

impl Screen {
    fn load(&self, origin: Origin) -> Effect {
        let screen = self.id;
        match &self.kind {
            ScreenKind::Home(_) => Effect::LoadHome { screen, origin },
            ScreenKind::Forum(f) => Effect::LoadForum {
                screen,
                forum: f.uri.clone(),
                sort: f.sort,
                origin,
            },
            ScreenKind::Thread(t) => Effect::LoadThread {
                screen,
                uri: t.uri.clone(),
                origin,
            },
        }
    }

    /// True if a `Changed` topic concerns this screen.
    fn concerns(&self, topic: &Topic) -> bool {
        match (&self.kind, topic) {
            (ScreenKind::Home(_), Topic::Forum { .. } | Topic::Accounts) => true,
            (ScreenKind::Forum(f), Topic::Forum { uri }) => *uri == f.uri,
            (ScreenKind::Thread(t), topic) => {
                let Some(d) = &t.data else {
                    return matches!(topic, Topic::Thread { uri } if *uri == t.uri);
                };
                match topic {
                    Topic::Thread { uri } => *uri == t.uri || *uri == thread::root_uri(d),
                    Topic::Forum { uri } => t.post().is_some_and(|p| p.post.forum == *uri),
                    Topic::User { did } => {
                        d.view.comments.iter().any(|c| c.author.did == *did)
                            || t.post().is_some_and(|p| p.post.author.did == *did)
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// How many items a pending update adds (`None`: no pending update).
    pub fn pending_new(&self) -> Option<usize> {
        fn new_of(cur: Vec<String>, pending: Vec<String>) -> usize {
            let cur: BTreeSet<String> = cur.into_iter().collect();
            pending.iter().filter(|u| !cur.contains(*u)).count()
        }
        match &self.kind {
            ScreenKind::Home(s) => {
                let p = s.pending.as_ref()?;
                Some(new_of(
                    s.data.as_ref().map(HomeScreen::uris).unwrap_or_default(),
                    HomeScreen::uris(p),
                ))
            }
            ScreenKind::Forum(s) => {
                let p = s.pending.as_ref()?;
                Some(new_of(
                    s.data.as_ref().map(ForumScreen::uris).unwrap_or_default(),
                    ForumScreen::uris(p),
                ))
            }
            ScreenKind::Thread(s) => {
                let p = s.pending.as_ref()?;
                Some(new_of(
                    s.data.as_ref().map(thread::item_uris).unwrap_or_default(),
                    thread::item_uris(p),
                ))
            }
        }
    }
}

/// How a list screen takes new data: at once (the user asked, or nothing is shown yet), in
/// place (the same items), or as pending.
fn receive_list<T>(
    data: &mut Option<T>,
    pending: &mut Option<T>,
    pos: &mut ListPos,
    new: T,
    origin: Origin,
    uris: fn(&T) -> Vec<String>,
) {
    match data {
        Some(cur) if origin == Origin::Live && uris(cur) != uris(&new) => {
            *pending = Some(new);
        }
        Some(cur) => {
            let selected = uris(cur).get(pos.cursor).cloned();
            let fresh = uris(&new);
            if let Some(i) = selected.and_then(|u| fresh.iter().position(|x| *x == u)) {
                pos.cursor = i;
            }
            *data = Some(new);
            *pending = None;
        }
        None => {
            *data = Some(new);
            *pending = None;
        }
    }
}

/// The application state.
#[derive(Clone, Debug)]
pub struct State {
    /// Terminal width.
    pub width: u16,
    /// Terminal height.
    pub height: u16,
    /// The screens, Home at the bottom.
    pub stack: Vec<Screen>,
    next_id: ScreenId,
    /// The `?` overlay is open.
    pub help: bool,
    /// The status line's message.
    pub status: Option<Status>,
    /// The account that writes (`None`: read-only).
    pub account: Option<AccountView>,
    /// No network: nothing is synced, writes stay queued.
    pub offline: bool,
    /// Sync a forum or thread when its screen opens (on unless offline).
    pub auto_sync: bool,
    /// Sync state by forum uri.
    pub sync: BTreeMap<String, SyncStatus>,
    /// Outbox writes not finished yet, by id.
    pub writes: BTreeMap<String, WriteState>,
    /// Outbox writes that failed (sent again only by `lessdugin outbox retry`), by id.
    pub failed: BTreeSet<String>,
    /// The writes queued from this TUI.
    own: BTreeMap<String, OwnWrite>,
    /// Writes an event finished before the result of queuing them arrived (the runtime runs
    /// the call as a task, and the pump may send the write first).
    finished: BTreeSet<String>,
    /// One of them is done, queued from this screen: its next change is shown at once (the
    /// user's own write is what they expect to see, not a banner). Cleared when the stack
    /// changes, so it never applies to another screen.
    follow_own: Option<ScreenId>,
    /// Text kept after a failed attempt, offered again by the next editor session for it.
    kept: Option<(Compose, String)>,
    /// The user asked to quit.
    pub quit: bool,
}

/// The rows the frame takes outside the content: the header, the info line and the status line
/// (one row; a long message takes up to [`MAX_STATUS_ROWS`]).
pub const CHROME_ROWS: u16 = 3;

/// The most rows a status message wraps to.
pub const MAX_STATUS_ROWS: u16 = 3;

/// A status message as the status line draws it (after a one-column margin).
pub fn status_text(s: &Status) -> TermText {
    cat([lit(" "), s.text.clone()])
}

impl State {
    /// A new state for a terminal of `width` × `height`.
    pub fn new(width: u16, height: u16, account: Option<AccountView>, offline: bool) -> Self {
        Self {
            width,
            height,
            stack: Vec::new(),
            next_id: 1,
            help: false,
            status: None,
            account,
            offline,
            auto_sync: !offline,
            sync: BTreeMap::new(),
            writes: BTreeMap::new(),
            failed: BTreeSet::new(),
            own: BTreeMap::new(),
            finished: BTreeSet::new(),
            follow_own: None,
            kept: None,
            quit: false,
        }
    }

    /// Opens Home, and the forum `forum` on top of it when given; loads the outbox of earlier
    /// sessions when an account writes.
    pub fn start(&mut self, forum: Option<String>) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.account.is_some() {
            fx.push(Effect::LoadWrites);
        }
        self.push(ScreenKind::Home(HomeScreen::default()), &mut fx);
        if let Some(f) = forum {
            self.push_forum(f, &mut fx);
        }
        fx
    }

    /// The content rows (the frame minus header, info and status lines).
    pub fn content_height(&self) -> usize {
        let chrome = CHROME_ROWS - 1 + self.status_rows();
        usize::from(self.height.saturating_sub(chrome).max(1))
    }

    /// The rows of the status line: a long message wraps, up to [`MAX_STATUS_ROWS`] (and
    /// never taking the last content row).
    pub fn status_rows(&self) -> u16 {
        let Some(s) = &self.status else {
            return 1;
        };
        let need = crate::text::wrapped(
            &status_text(s),
            ratatui::style::Style::default(),
            usize::from(self.width),
        )
        .len();
        let room = self.height.saturating_sub(CHROME_ROWS).max(1);
        u16::try_from(need)
            .unwrap_or(u16::MAX)
            .clamp(1, MAX_STATUS_ROWS.min(room))
    }

    /// The top screen.
    pub fn top(&self) -> Option<&Screen> {
        self.stack.last()
    }

    /// The thread on top, if a thread is on top.
    pub fn top_thread(&self) -> Option<&ThreadScreen> {
        match &self.top()?.kind {
            ScreenKind::Thread(t) => Some(t),
            _ => None,
        }
    }

    fn index_of(&self, id: ScreenId) -> Option<usize> {
        self.stack.iter().position(|s| s.id == id)
    }

    fn say(&mut self, tone: Tone, text: TermText) {
        self.status = Some(Status { text, tone });
    }

    /// Runs `action`; returns the effects to run.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        let mut fx = Vec::new();
        match action {
            Action::Key(k) => self.key(&k, &mut fx),
            Action::Resize(w, h) => {
                self.width = w;
                self.height = h;
            }
            Action::Engine(e) => self.event(e, &mut fx),
            Action::HomeLoaded {
                screen,
                origin,
                result,
            } => self.loaded(screen, origin, result.map(Loaded::Home), &mut fx),
            Action::ForumLoaded {
                screen,
                origin,
                result,
            } => self.loaded(screen, origin, result.map(Loaded::Forum), &mut fx),
            Action::ThreadLoaded {
                screen,
                origin,
                result,
            } => self.loaded(screen, origin, result.map(Loaded::Thread), &mut fx),
            Action::Synced {
                target,
                user,
                at_ms,
                results,
            } => self.synced(&target, user, at_ms, results, &mut fx),
            Action::Edited { compose, result } => self.edited(compose, result, &mut fx),
            Action::WritesLoaded(result) => self.writes_loaded(result),
            Action::Queued {
                compose,
                text,
                result,
            } => self.queued(compose, text, result, &mut fx),
        }
        self.fit();
        fx
    }

    /// Keeps the top screen consistent with the terminal size: the thread laid out for the
    /// width, lists scrolled to show their cursor.
    fn fit(&mut self) {
        let (w, h) = (self.width, self.content_height());
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        match &mut top.kind {
            ScreenKind::Home(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.forums.len());
                s.pos.reveal(len, h / LIST_ROWS);
            }
            ScreenKind::Forum(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.list.items.len());
                s.pos.reveal(len, h / LIST_ROWS);
            }
            ScreenKind::Thread(t) => {
                if t.layout.width != w {
                    t.relayout(w, h);
                    t.pos.reveal(&t.layout, h, false);
                }
                t.pos.clamp(&t.layout, h);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // screens

    fn push(&mut self, kind: ScreenKind, fx: &mut Vec<Effect>) -> usize {
        self.follow_own = None;
        let id = self.next_id;
        self.next_id += 1;
        self.stack.push(Screen {
            id,
            kind,
            loading: 0,
            dirty: false,
            stale: false,
            settling: false,
            error: None,
        });
        let i = self.stack.len() - 1;
        self.reload(i, Origin::User, fx);
        i
    }

    fn push_forum(&mut self, uri: String, fx: &mut Vec<Effect>) {
        let i = self.push(
            ScreenKind::Forum(Box::new(ForumScreen {
                uri: uri.clone(),
                sort: PostSort::Magic,
                data: None,
                pending: None,
                pos: ListPos::default(),
            })),
            fx,
        );
        if self.auto_sync && !self.offline {
            self.stack[i].settling = true;
            fx.push(Effect::Sync {
                target: SyncTarget::Forum(uri),
                user: false,
            });
        }
    }

    /// Opens the thread of `uri` (a post or a comment) on top.
    pub fn open_thread(&mut self, uri: String) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.push_thread(uri, &mut fx);
        self.fit();
        fx
    }

    /// Opens the forum `uri` on top.
    pub fn open_forum(&mut self, uri: String) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.push_forum(uri, &mut fx);
        self.fit();
        fx
    }

    fn push_thread(&mut self, uri: String, fx: &mut Vec<Effect>) {
        let i = self.push(
            ScreenKind::Thread(Box::new(ThreadScreen {
                uri: uri.clone(),
                ..ThreadScreen::default()
            })),
            fx,
        );
        if self.auto_sync && !self.offline {
            self.stack[i].settling = true;
            fx.push(Effect::Sync {
                target: SyncTarget::Thread { uri, forum: None },
                user: false,
            });
        }
    }

    fn pop(&mut self, fx: &mut Vec<Effect>) {
        if self.stack.len() <= 1 {
            return;
        }
        self.follow_own = None;
        self.stack.pop();
        let i = self.stack.len() - 1;
        if self.stack[i].stale {
            self.stack[i].stale = false;
            self.reload(i, Origin::Live, fx);
        }
    }

    fn reload(&mut self, i: usize, origin: Origin, fx: &mut Vec<Effect>) {
        let s = &mut self.stack[i];
        if origin == Origin::Live && s.loading > 0 {
            s.dirty = true;
            return;
        }
        s.loading += 1;
        fx.push(s.load(origin));
    }

    fn apply_pending(&mut self) {
        let (w, h) = (self.width, self.content_height());
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        match &mut top.kind {
            ScreenKind::Home(s) => {
                if let Some(p) = s.pending.take() {
                    receive_list(
                        &mut s.data,
                        &mut s.pending,
                        &mut s.pos,
                        p,
                        Origin::User,
                        HomeScreen::uris,
                    );
                }
            }
            ScreenKind::Forum(s) => {
                if let Some(p) = s.pending.take() {
                    receive_list(
                        &mut s.data,
                        &mut s.pending,
                        &mut s.pos,
                        p,
                        Origin::User,
                        ForumScreen::uris,
                    );
                }
            }
            ScreenKind::Thread(t) => {
                t.apply_pending(w, h);
            }
        }
    }

    /// Shows the top screen's pending live update, if any (what `R` does before syncing; also
    /// for headless captures).
    pub fn show_pending(&mut self) {
        self.apply_pending();
        self.fit();
    }

    // -----------------------------------------------------------------------------------------
    // keys

    fn key(&mut self, k: &KeyEvent, fx: &mut Vec<Effect>) {
        if k.kind == crossterm::event::KeyEventKind::Release {
            return;
        }
        let cmd = keys::command(k);
        if cmd == Some(Cmd::Quit) {
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        // any key closes the help overlay
        if self.help {
            self.help = false;
            return;
        }
        let Some(cmd) = cmd else {
            return;
        };
        match cmd {
            Cmd::Help => self.help = true,
            Cmd::Back => self.pop(fx),
            Cmd::BackOrQuit => {
                if self.stack.len() > 1 {
                    self.pop(fx);
                } else {
                    self.quit = true;
                    fx.push(Effect::Quit);
                }
            }
            Cmd::Open => self.open(fx),
            Cmd::Sync => self.sync_top(fx),
            Cmd::Sort => self.sort(fx),
            Cmd::Reply => self.compose_comment(true, fx),
            Cmd::CommentOnPost => self.compose_comment(false, fx),
            Cmd::NewPost => self.compose_post(fx),
            nav => self.navigate(nav),
        }
    }

    fn navigate(&mut self, cmd: Cmd) {
        let (w, h) = (self.width, self.content_height());
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        top.settling = false;
        match &mut top.kind {
            ScreenKind::Home(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.forums.len());
                s.pos.apply(cmd, len, h / LIST_ROWS);
            }
            ScreenKind::Forum(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.list.items.len());
                s.pos.apply(cmd, len, h / LIST_ROWS);
            }
            ScreenKind::Thread(t) => {
                let l = &t.layout;
                match cmd {
                    Cmd::Down => t.pos.down(l, h),
                    Cmd::Up => t.pos.up(l, h),
                    Cmd::Top => t.pos.top(),
                    Cmd::Bottom => t.pos.bottom(l, h),
                    Cmd::HalfDown => t.pos.half_down(l, h),
                    Cmd::HalfUp => t.pos.half_up(l, h),
                    Cmd::Parent => t.pos.parent(l, h),
                    Cmd::Collapse => {
                        let Some(it) = l.items.get(t.pos.selected) else {
                            return;
                        };
                        if !it.comment || it.replies == 0 {
                            return;
                        }
                        let uri = it.uri.clone();
                        if !t.collapsed.remove(&uri) {
                            t.collapsed.insert(uri);
                        }
                        t.relayout(w, h);
                        t.pos.reveal(&t.layout, h, false);
                    }
                    _ => {}
                }
            }
        }
    }

    fn open(&mut self, fx: &mut Vec<Effect>) {
        let Some(top) = self.stack.last() else {
            return;
        };
        match &top.kind {
            ScreenKind::Home(s) => {
                if let Some(f) = s.selected() {
                    let uri = f.uri.clone();
                    self.push_forum(uri, fx);
                }
            }
            ScreenKind::Forum(s) => {
                if let Some(p) = s.selected() {
                    let uri = p.uri.clone();
                    self.push_thread(uri, fx);
                }
            }
            ScreenKind::Thread(_) => self.navigate(Cmd::Collapse),
        }
    }

    fn sort(&mut self, fx: &mut Vec<Effect>) {
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        let ScreenKind::Forum(s) = &mut self.stack[i].kind else {
            return;
        };
        s.sort = match s.sort {
            PostSort::Magic => PostSort::Top,
            PostSort::Top => PostSort::New,
            PostSort::New => PostSort::Old,
            _ => PostSort::Magic,
        };
        s.pos = ListPos::default();
        s.pending = None;
        self.reload(i, Origin::User, fx);
    }

    fn sync_top(&mut self, fx: &mut Vec<Effect>) {
        self.apply_pending();
        if self.offline {
            self.say(Tone::Warn, lit("Offline: nothing is synced."));
            return;
        }
        let Some(top) = self.stack.last() else {
            return;
        };
        let target = match &top.kind {
            ScreenKind::Home(s) => {
                let forums: Vec<String> = s
                    .data
                    .as_ref()
                    .map(|d| d.forums.iter().map(|f| f.uri.clone()).collect())
                    .unwrap_or_default();
                if forums.is_empty() {
                    self.say(
                        Tone::Info,
                        lit("You follow no forum yet: run `lessdugin join <forum>`."),
                    );
                    return;
                }
                SyncTarget::Forums(forums)
            }
            ScreenKind::Forum(s) => SyncTarget::Forum(s.uri.clone()),
            ScreenKind::Thread(t) => SyncTarget::Thread {
                uri: t.uri.clone(),
                forum: t.post().map(|p| p.post.forum.clone()),
            },
        };
        let forums = match &target {
            SyncTarget::Forums(f) => f.clone(),
            SyncTarget::Forum(f) => vec![f.clone()],
            SyncTarget::Thread { forum, .. } => forum.iter().cloned().collect(),
        };
        for f in forums {
            self.sync
                .insert(f, SyncStatus::Running { done: 0, total: 0 });
        }
        self.say(Tone::Info, lit("Syncing..."));
        fx.push(Effect::Sync { target, user: true });
    }

    // -----------------------------------------------------------------------------------------
    // writing

    fn can_write(&mut self) -> bool {
        if self.account.is_none() {
            self.say(
                Tone::Error,
                lit("Log in to write: run `lessdugin login`, then start the TUI again."),
            );
            return false;
        }
        true
    }

    fn prefill(&self, compose: &Compose) -> String {
        match &self.kept {
            Some((c, text)) if c == compose => text.clone(),
            _ => String::new(),
        }
    }

    fn compose_comment(&mut self, reply: bool, fx: &mut Vec<Effect>) {
        let Some(t) = self.top_thread() else {
            self.say(Tone::Info, lit("Open a post to comment on it."));
            return;
        };
        let Some(p) = t.post() else {
            if t.data.is_some() {
                self.say(
                    Tone::Error,
                    lit("The post is not available, so it cannot be commented on."),
                );
            }
            return;
        };
        let root = p.post.uri.clone();
        let title = post_title(&p.post).to_plain_string();
        let selected = t
            .layout
            .items
            .get(t.pos.selected)
            .filter(|i| i.comment && reply)
            .and_then(|i| {
                t.data
                    .as_ref()?
                    .view
                    .comments
                    .iter()
                    .find(|c| c.uri == i.uri && !c.missing)
            });
        let (parent, context, quoted) = match selected {
            Some(c) => (
                Some(c.uri.clone()),
                vec![
                    format!("Replying to {}", actor(&c.author).to_plain_string()),
                    format!("in: {title}"),
                ],
                Some(c.body.text.clone()),
            ),
            None => (
                None,
                vec![
                    format!("Commenting on: {title}"),
                    format!("by {}", actor(&p.post.author).to_plain_string()),
                ],
                None,
            ),
        };
        if !self.can_write() {
            return;
        }
        let compose = Compose::Comment { root, parent };
        let template =
            editor::comment_template(&self.prefill(&compose), &context, quoted.as_deref());
        fx.push(Effect::Editor(EditorRequest { compose, template }));
    }

    fn compose_post(&mut self, fx: &mut Vec<Effect>) {
        let Some(top) = self.stack.last() else {
            return;
        };
        let forum = match &top.kind {
            ScreenKind::Home(s) => s.selected().map(|f| (f.uri.clone(), f.name.clone())),
            ScreenKind::Forum(s) => Some((
                s.uri.clone(),
                s.name().map_or_else(|| s.uri.clone(), str::to_owned),
            )),
            ScreenKind::Thread(t) => t.post().map(|p| {
                let uri = p.post.forum.clone();
                let name = self
                    .stack
                    .iter()
                    .find_map(|s| match &s.kind {
                        ScreenKind::Forum(f) if f.uri == uri => f.name().map(str::to_owned),
                        _ => None,
                    })
                    .unwrap_or_else(|| uri.clone());
                (uri, name)
            }),
        };
        let Some((uri, name)) = forum else {
            self.say(Tone::Info, lit("Open a forum to post in it."));
            return;
        };
        if !self.can_write() {
            return;
        }
        let compose = Compose::Post { forum: uri };
        let template = editor::post_template(&self.prefill(&compose), &name);
        fx.push(Effect::Editor(EditorRequest { compose, template }));
    }

    fn edited(
        &mut self,
        compose: Compose,
        result: Result<String, editor::EditError>,
        fx: &mut Vec<Effect>,
    ) {
        let raw = match result {
            Ok(raw) => raw,
            Err(e) => {
                // an editor that failed after the user wrote something: keep the text
                match e.text.as_deref().and_then(editor::strip_template) {
                    Some(text) => {
                        let again = compose.again();
                        self.kept = Some((compose, text));
                        self.say(
                            Tone::Error,
                            tt(&format!(
                                "{}. Your text is kept; press {again} to edit it again.",
                                e.message
                            )),
                        );
                    }
                    None => self.say(Tone::Error, tt(&e.message)),
                }
                return;
            }
        };
        let Some(text) = editor::strip_template(&raw) else {
            self.say(Tone::Info, lit("Nothing to send: the text was empty."));
            return;
        };
        self.kept = Some((compose.clone(), text.clone()));
        match &compose {
            Compose::Comment { root, parent } => {
                self.say(Tone::Info, lit("Queuing..."));
                fx.push(Effect::Comment {
                    draft: CommentDraft::new(root.clone(), parent.clone(), text),
                });
            }
            Compose::Post { forum } => match editor::split_post(&text) {
                Ok((title, body)) => {
                    self.say(Tone::Info, lit("Queuing..."));
                    fx.push(Effect::Publish {
                        draft: PostDraft::article(forum.clone(), title, body),
                    });
                }
                Err(why) => self.say(
                    Tone::Error,
                    cat([
                        lit("Not published: "),
                        tt(why),
                        lit(". Press P to edit it again."),
                    ]),
                ),
            },
        }
    }

    fn queued(
        &mut self,
        compose: Compose,
        text: String,
        result: Result<WriteTicket, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        let noun = compose.noun();
        match result {
            Ok(t) if self.failed.contains(&t.id) => {
                // it failed before this result arrived: the text comes back to the editor
                let again = compose.again();
                self.kept = Some((compose, text));
                self.say(
                    Tone::Error,
                    tt(&format!(
                        "Your {noun} could not be sent. Press {again} to edit the text and send \
                         it again, or run `lessdugin outbox retry {}`.",
                        t.id
                    )),
                );
            }
            Ok(t) => {
                self.kept = None;
                if !matches!(
                    t.state,
                    WriteState::Done { .. } | WriteState::Failed { .. } | WriteState::Undone
                ) && !self.failed.contains(&t.id)
                    && !self.finished.remove(&t.id)
                {
                    // (an event may already have finished it: then it is not tracked again)
                    let screen = self.top().map_or(0, |s| s.id);
                    self.own.insert(
                        t.id.clone(),
                        OwnWrite {
                            screen,
                            compose: compose.clone(),
                            text: text.clone(),
                        },
                    );
                    self.writes.insert(t.id.clone(), t.state.clone());
                }
                let msg = if self.offline {
                    format!("Your {noun} is queued; it is sent when you are online.")
                } else {
                    format!("Your {noun} is queued; sending...")
                };
                self.say(Tone::Good, tt(&msg));
                if let Some(i) = self.stack.len().checked_sub(1) {
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => {
                let again = compose.again();
                self.kept = Some((compose, text));
                self.say(
                    Tone::Error,
                    tt(&format!(
                        "Your {noun} was not queued: {}. Your text is kept; press {again} to edit it again.",
                        e.message
                    )),
                );
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // engine results and events

    fn loaded(
        &mut self,
        screen: ScreenId,
        origin: Origin,
        result: Result<Loaded, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        let (w, h) = (self.width, self.content_height());
        let Some(i) = self.index_of(screen) else {
            return;
        };
        let is_top = i + 1 == self.stack.len();
        let s = &mut self.stack[i];
        s.loading = s.loading.saturating_sub(1);
        let origin = if s.settling { Origin::User } else { origin };
        let mut failed = None;
        match result {
            Err(e) => {
                failed = Some(tt(&e.message));
                s.error = Some(e);
            }
            Ok(data) => {
                s.error = None;
                match (&mut s.kind, data) {
                    (ScreenKind::Home(s), Loaded::Home(d)) => receive_list(
                        &mut s.data,
                        &mut s.pending,
                        &mut s.pos,
                        d,
                        origin,
                        HomeScreen::uris,
                    ),
                    (ScreenKind::Forum(s), Loaded::Forum(d)) => {
                        // a load for another sort is superseded by the one in flight
                        if d.list.sort == s.sort {
                            receive_list(
                                &mut s.data,
                                &mut s.pending,
                                &mut s.pos,
                                *d,
                                origin,
                                ForumScreen::uris,
                            );
                        }
                    }
                    (ScreenKind::Thread(t), Loaded::Thread(d)) => t.receive(*d, origin, w, h),
                    _ => {}
                }
            }
        }
        if s.dirty && s.loading == 0 {
            s.dirty = false;
            self.reload(i, Origin::Live, fx);
        }
        if let (Some(msg), true, Origin::User) = (failed, is_top, origin) {
            self.say(Tone::Error, msg);
        }
    }

    fn synced(
        &mut self,
        target: &SyncTarget,
        user: bool,
        at_ms: i64,
        results: Vec<(String, Result<SyncReport, EngineError>)>,
        fx: &mut Vec<Effect>,
    ) {
        let mut errors = Vec::new();
        let mut partial = false;
        for (forum, r) in results {
            let st = match r {
                Ok(rep) => {
                    partial |= rep.partial;
                    SyncStatus::Done {
                        at_ms,
                        partial: rep.partial,
                        complete: rep.repos_complete,
                        total: rep.repos_total,
                    }
                }
                Err(e) => {
                    errors.push(e.message.clone());
                    SyncStatus::Failed {
                        at_ms,
                        message: tt(&e.message),
                    }
                }
            };
            // a thread sync reports under its forum; keep the forum's own status otherwise
            if matches!(target, SyncTarget::Thread { .. })
                && st_is_ok(&st)
                && self.sync.contains_key(&forum)
                && !matches!(self.sync.get(&forum), Some(SyncStatus::Running { .. }))
            {
                continue;
            }
            self.sync.insert(forum, st);
        }
        if !user {
            // the sync a screen started when it opened: show its result at once
            for i in 0..self.stack.len() {
                let s = &self.stack[i];
                let ours = match (&s.kind, target) {
                    (ScreenKind::Forum(f), SyncTarget::Forum(u)) => f.uri == *u,
                    (ScreenKind::Thread(t), SyncTarget::Thread { uri, .. }) => t.uri == *uri,
                    _ => false,
                };
                if ours && s.settling {
                    self.stack[i].settling = false;
                    self.reload(i, Origin::User, fx);
                }
            }
            return;
        }
        match errors.first() {
            Some(e) => self.say(Tone::Error, cat([lit("Sync failed: "), tt(e)])),
            None if partial => {
                self.say(Tone::Warn, lit("Synced in part; press R to sync the rest."))
            }
            None => self.say(Tone::Good, lit("Synced.")),
        }
        if let Some(i) = self.stack.len().checked_sub(1) {
            self.reload(i, Origin::User, fx);
        }
    }

    /// The outbox at start: writes still queued or failed from earlier sessions (states that
    /// events already reported are kept).
    fn writes_loaded(&mut self, result: Result<Vec<WriteTicket>, EngineError>) {
        let Ok(tickets) = result else {
            return;
        };
        for t in tickets {
            if self.writes.contains_key(&t.id) || self.own.contains_key(&t.id) {
                continue;
            }
            match t.state {
                WriteState::Pending { .. } | WriteState::Sending => {
                    self.writes.insert(t.id, t.state);
                }
                WriteState::Failed { .. } => {
                    self.failed.insert(t.id);
                }
                WriteState::Done { .. } | WriteState::Undone => {}
            }
        }
    }

    fn event(&mut self, e: Event, fx: &mut Vec<Effect>) {
        match e {
            Event::Changed { topics } => {
                // a sync's last progress event (done == total) is followed by `Changed`
                for t in &topics {
                    if let Topic::Forum { uri } = t
                        && let Some(SyncStatus::Running { done, total }) = self.sync.get(uri)
                        && done == total
                    {
                        self.sync.insert(uri.clone(), SyncStatus::Finished);
                    }
                }
                let top = self.stack.len().saturating_sub(1);
                for i in 0..self.stack.len() {
                    if !topics.iter().any(|t| self.stack[i].concerns(t)) {
                        continue;
                    }
                    if i == top {
                        let origin = if self.follow_own == Some(self.stack[i].id) {
                            self.follow_own = None;
                            Origin::User
                        } else {
                            Origin::Live
                        };
                        self.reload(i, origin, fx);
                    } else {
                        self.stack[i].stale = true;
                    }
                }
            }
            Event::SyncProgress { forum, done, total } => {
                self.sync.insert(forum, SyncStatus::Running { done, total });
            }
            Event::Write { id, state } => match state {
                WriteState::Pending { .. } | WriteState::Sending => {
                    self.failed.remove(&id);
                    self.writes.insert(id, state);
                }
                WriteState::Done { .. } => {
                    self.writes.remove(&id);
                    self.failed.remove(&id);
                    match self.own.remove(&id) {
                        Some(w) => {
                            if self.top().is_some_and(|s| s.id == w.screen) {
                                self.follow_own = Some(w.screen);
                            }
                            self.say(Tone::Good, lit("Sent."));
                        }
                        None => {
                            self.finished.insert(id);
                        }
                    }
                }
                WriteState::Failed { error } => {
                    self.writes.remove(&id);
                    self.failed.insert(id.clone());
                    if let Some(w) = self.own.remove(&id) {
                        // the text comes back to the editor; the write stays in the outbox
                        let noun = w.compose.noun();
                        let again = w.compose.again();
                        self.kept = Some((w.compose, w.text));
                        self.say(
                            Tone::Error,
                            tt(&format!(
                                "Could not send your {noun}: {}. Press {again} to edit the text \
                                 and send it again, or run `lessdugin outbox retry {id}`.",
                                error.message
                            )),
                        );
                    }
                }
                WriteState::Undone => {
                    self.writes.remove(&id);
                    self.failed.remove(&id);
                    if self.own.remove(&id).is_none() {
                        self.finished.insert(id);
                    }
                }
            },
            Event::Auth { state, .. } => {
                if matches!(state, lessdugin_core::dto::AuthState::Expired) {
                    self.say(
                        Tone::Error,
                        lit("Your session expired: run `lessdugin login` to keep writing."),
                    );
                }
            }
            Event::Warning { message, .. } => {
                self.say(Tone::Warn, tt(&message));
            }
        }
    }
}

fn st_is_ok(s: &SyncStatus) -> bool {
    matches!(s, SyncStatus::Done { .. })
}

/// A load result.
enum Loaded {
    Home(HomeData),
    Forum(Box<ForumData>),
    Thread(Box<ThreadData>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(st: &mut State, id: &str, root: &str) -> Vec<Effect> {
        st.update(Action::Queued {
            compose: Compose::Comment {
                root: root.into(),
                parent: None,
            },
            text: "my reply".into(),
            result: Ok(WriteTicket {
                id: id.into(),
                state: WriteState::Pending {
                    not_before_ms: None,
                    held: false,
                    throttled: false,
                    needs_login: false,
                    error: None,
                },
            }),
        })
    }

    fn done(st: &mut State, id: &str) -> Vec<Effect> {
        st.update(Action::Engine(Event::Write {
            id: id.into(),
            state: WriteState::Done {
                uri: "at://did:plc:x/c/1".into(),
                cid: None,
            },
        }))
    }

    fn changed(st: &mut State, topic: Topic) -> Vec<Effect> {
        st.update(Action::Engine(Event::Changed {
            topics: vec![topic],
        }))
    }

    fn user_loads(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| {
                matches!(
                    e,
                    Effect::LoadForum {
                        origin: Origin::User,
                        ..
                    } | Effect::LoadThread {
                        origin: Origin::User,
                        ..
                    }
                )
            })
            .count()
    }

    /// An own write shows at once on the screen it was written from, and only there: when it
    /// finishes under another screen, that screen's next live update is not applied at once.
    #[test]
    fn own_writes_are_followed_only_on_their_screen() {
        let mut st = State::new(80, 24, None, true);
        st.start(None);
        st.open_forum("at://f".into());
        st.open_thread("at://t".into());
        queued(&mut st, "w1", "at://t");
        done(&mut st, "w1");
        let fx = changed(
            &mut st,
            Topic::Thread {
                uri: "at://t".into(),
            },
        );
        assert_eq!(user_loads(&fx), 1, "the own write is shown at once: {fx:?}");
        // the flag is spent
        let fx = changed(
            &mut st,
            Topic::Thread {
                uri: "at://t".into(),
            },
        );
        assert_eq!(user_loads(&fx), 0, "{fx:?}");

        // queued on the thread, finished after going back to the forum
        queued(&mut st, "w2", "at://t");
        st.update(Action::Key(keys::parse_key("h").unwrap()));
        done(&mut st, "w2");
        let fx = changed(
            &mut st,
            Topic::Forum {
                uri: "at://f".into(),
            },
        );
        assert_eq!(
            user_loads(&fx),
            0,
            "the forum's live update is not forced: {fx:?}"
        );
        assert_eq!(
            st.status
                .as_ref()
                .map(|s| s.text.to_plain_string())
                .as_deref(),
            Some("Sent.")
        );
    }

    /// A failed own write gives its text back to the editor and names the write; a long status
    /// message wraps (up to three rows), taking rows from the content.
    #[test]
    fn a_failed_write_keeps_its_text_and_the_status_wraps() {
        let mut st = State::new(40, 12, None, true);
        st.start(None);
        st.open_thread("at://t".into());
        queued(&mut st, "did:plc:x/42", "at://t");
        st.update(Action::Engine(Event::Write {
            id: "did:plc:x/42".into(),
            state: WriteState::Failed {
                error: EngineError::invalid_input("InvalidRequest (HTTP 400)"),
            },
        }));
        let compose = Compose::Comment {
            root: "at://t".into(),
            parent: None,
        };
        assert_eq!(st.prefill(&compose), "my reply");
        let msg = st.status.as_ref().unwrap().text.to_plain_string();
        assert!(
            msg.contains("`lessdugin outbox retry did:plc:x/42`"),
            "{msg}"
        );
        assert!(msg.contains("Press c"), "{msg}");
        assert!(st.failed.contains("did:plc:x/42") && st.writes.is_empty());
        assert_eq!(st.status_rows(), MAX_STATUS_ROWS);
        assert_eq!(st.content_height(), 7);
        // a short message takes one row; a tiny terminal keeps a content row
        st.say(Tone::Info, lit("ok"));
        assert_eq!(st.status_rows(), 1);
        st.say(Tone::Error, tt(&msg));
        st.update(Action::Resize(10, 4));
        assert_eq!(st.status_rows(), 1);
        assert_eq!(st.content_height(), 1);
    }

    /// An editor that failed after the user wrote: the text is offered again.
    #[test]
    fn a_failed_editor_keeps_the_text() {
        let mut st = State::new(80, 24, None, true);
        st.start(None);
        let compose = Compose::Post {
            forum: "at://f".into(),
        };
        st.update(Action::Edited {
            compose: compose.clone(),
            result: Err(editor::EditError {
                message: "the editor (vi) exited with exit status: 1; nothing was sent".into(),
                text: Some(editor::post_template("Title\n\nWritten", "Meta")),
            }),
        });
        assert_eq!(st.prefill(&compose), "Title\n\nWritten");
        let msg = st.status.as_ref().unwrap().text.to_plain_string();
        assert!(
            msg.ends_with("Your text is kept; press P to edit it again."),
            "{msg}"
        );
        // nothing written: nothing kept
        let other = Compose::Post {
            forum: "at://g".into(),
        };
        st.update(Action::Edited {
            compose: other.clone(),
            result: Err(editor::EditError {
                message: "aborted".into(),
                text: Some(editor::post_template("", "Meta")),
            }),
        });
        assert_eq!(st.prefill(&other), "");
    }

    /// Writes from earlier sessions are counted at start (queued and failed).
    #[test]
    fn the_outbox_of_earlier_sessions_is_loaded() {
        let mut st = State::new(80, 24, None, true);
        st.start(None);
        let pending = WriteState::Pending {
            not_before_ms: None,
            held: false,
            throttled: false,
            needs_login: false,
            error: None,
        };
        st.update(Action::WritesLoaded(Ok(vec![
            WriteTicket {
                id: "a/1".into(),
                state: pending,
            },
            WriteTicket {
                id: "a/2".into(),
                state: WriteState::Failed {
                    error: EngineError::invalid_input("x"),
                },
            },
            WriteTicket {
                id: "a/3".into(),
                state: WriteState::Done {
                    uri: "at://x".into(),
                    cid: None,
                },
            },
        ])));
        assert_eq!(st.writes.keys().collect::<Vec<_>>(), ["a/1"]);
        assert_eq!(st.failed.iter().collect::<Vec<_>>(), ["a/2"]);
    }

    #[test]
    fn list_cursor_scrolls_the_least() {
        let mut p = ListPos::default();
        for _ in 0..5 {
            p.apply(Cmd::Down, 10, 3);
        }
        assert_eq!(p, ListPos { cursor: 5, top: 3 });
        p.apply(Cmd::Top, 10, 3);
        assert_eq!(p, ListPos::default());
        p.apply(Cmd::Bottom, 10, 3);
        assert_eq!(p, ListPos { cursor: 9, top: 7 });
        p.apply(Cmd::HalfUp, 10, 4);
        assert_eq!(p, ListPos { cursor: 7, top: 6 });
        p.reveal(2, 3);
        assert_eq!(p, ListPos { cursor: 1, top: 0 });
        p.reveal(0, 3);
        assert_eq!(p, ListPos::default());
    }

    #[test]
    fn live_lists_never_move_the_cursor() {
        let mut data = Some(vec!["a".to_owned(), "b".to_owned()]);
        let mut pending = None;
        let mut pos = ListPos { cursor: 1, top: 0 };
        let uris = |v: &Vec<String>| v.clone();
        // the same items: applied in place
        receive_list(
            &mut data,
            &mut pending,
            &mut pos,
            vec!["a".into(), "b".into()],
            Origin::Live,
            uris,
        );
        assert!(pending.is_none());
        // a new item: pending
        receive_list(
            &mut data,
            &mut pending,
            &mut pos,
            vec!["c".into(), "a".into(), "b".into()],
            Origin::Live,
            uris,
        );
        assert_eq!(data.as_ref().unwrap().len(), 2);
        assert!(pending.is_some());
        assert_eq!(pos.cursor, 1);
        // the user asks: applied, the cursor stays on "b"
        let p = pending.take().unwrap();
        receive_list(&mut data, &mut pending, &mut pos, p, Origin::User, uris);
        assert_eq!(pos.cursor, 2);
    }
}
