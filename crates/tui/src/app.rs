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
    AccessLevel, AccountView, ActionTicket, AgreementValue, Capability, CommentDraft, Event,
    ForumSummary, ForumView, KarmaValue, ModActionIntent, ModActionValue, ModLog, ModQueue,
    ModScope, NotificationList, OpinionIntent, PinScope, PostDraft, PostList, PostSort,
    ReactTally, Stance, SyncReport, ThreadRoot, ThreadView, Tier, Topic, UnreadCounts, ViewerCaps,
    WriteState, WriteTicket,
};
use lessdugin_core::engine::EngineError;

use crate::body::Body;
use crate::editor;
use crate::keys::{self, Cmd, Op};
use crate::text::{TermText, actor, cat, count, lit, post_title, tt};
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
    /// `opinion(subject, intent)`: one opinion key (`synthesis-v1.md` §6.11).
    Opinion {
        /// The post, comment, wiki revision or sequence.
        subject: String,
        /// The opinion change.
        intent: OpinionIntent,
        /// The forum the subject is in (for the caps gate message and a reload).
        forum: String,
        /// The screen the key was pressed on (reloaded after the write).
        screen: ScreenId,
        /// What the status line calls the write (`"Vote"`, `"React"`).
        label: &'static str,
    },
    /// `undo(id)`: undo a pending opinion write (`Backspace`, the 4 s undo window).
    Undo {
        /// The write id.
        id: String,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `grant(forum, actors)`: approves applicants from the mod queue (`social.md` §4.7).
    Grant {
        /// The forum.
        forum: String,
        /// The applicants' DIDs.
        actors: Vec<String>,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `mod_action(intent)`: one entry of the mod menu (`synthesis-v1.md` §6.11 "Other").
    ModAction {
        /// The assertion or removal.
        intent: ModActionIntent,
        /// The forum it is in (reloaded after the write).
        forum: String,
        /// The screen the menu was opened from.
        screen: ScreenId,
    },
    /// `notifications(q)` (the Notifications screen).
    LoadNotifications {
        /// The screen.
        screen: ScreenId,
        /// Read ones too.
        all: bool,
        /// Who asked.
        origin: Origin,
    },
    /// `mark_notifications_read(ids)`.
    MarkRead {
        /// The notification ids.
        ids: Vec<String>,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `unread_counts()`: the notification badge and the forums' unread state.
    LoadUnread,
    /// `mod_queue(forum)`.
    LoadQueue {
        /// The screen.
        screen: ScreenId,
        /// The forum uri.
        forum: String,
        /// Who asked.
        origin: Origin,
    },
    /// `mod_log(forum, None, MOD_LOG_LIMIT)`.
    LoadLog {
        /// The screen.
        screen: ScreenId,
        /// The forum uri.
        forum: String,
        /// Who asked.
        origin: Origin,
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
    /// [`Effect::Opinion`] finished.
    OpinionDone {
        /// The forum the subject is in.
        forum: String,
        /// The screen that was reloaded.
        screen: ScreenId,
        /// What the status line calls the write (as [`Effect::Opinion::label`]).
        label: &'static str,
        /// The action ticket (with its notices), or why it was refused.
        result: Result<ActionTicket, EngineError>,
    },
    /// [`Effect::Undo`] finished.
    Undone {
        /// The screen that was reloaded.
        screen: ScreenId,
        /// The write's new state.
        result: Result<WriteTicket, EngineError>,
    },
    /// [`Effect::Grant`] finished.
    Granted {
        /// The screen to reload.
        screen: ScreenId,
        /// The write tickets, or why the grant was refused.
        result: Result<Vec<WriteTicket>, EngineError>,
    },
    /// [`Effect::ModAction`] finished.
    ModActionDone {
        /// The forum it was in.
        forum: String,
        /// The screen that was reloaded.
        screen: ScreenId,
        /// The action ticket (with its notices), or why it was refused.
        result: Result<ActionTicket, EngineError>,
    },
    /// [`Effect::LoadNotifications`] finished.
    NotificationsLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The list.
        result: Result<NotificationList, EngineError>,
    },
    /// [`Effect::MarkRead`] finished.
    MarkedRead {
        /// The screen to reload.
        screen: ScreenId,
        /// Why marking failed, if it did.
        result: Result<(), EngineError>,
    },
    /// [`Effect::LoadUnread`] finished.
    UnreadLoaded(Result<UnreadCounts, EngineError>),
    /// [`Effect::LoadQueue`] finished.
    QueueLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The queue (or `blocked` for a viewer who is not an authority).
        result: Result<ModQueue, EngineError>,
    },
    /// [`Effect::LoadLog`] finished.
    LogLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The log.
        result: Result<ModLog, EngineError>,
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

/// The most consecutive live reloads of one screen with no user activity in between
/// ([`Screen::live_burst`]): the loop guard against a load that always re-announces a change.
pub const LIVE_RELOAD_CAP: u32 = 3;

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

/// Which half of the Mod screen shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModTab {
    /// The mod queue (authorities only).
    #[default]
    Queue,
    /// The mod log.
    Log,
}

/// The Mod screen: the forum's mod queue and log (`synthesis-v1.md` §6.11).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModScreen {
    /// The forum uri.
    pub forum: String,
    /// What shows.
    pub tab: ModTab,
    /// The queue (or why it is blocked: it is for the forum's authorities).
    pub queue: Option<Result<ModQueue, EngineError>>,
    /// The log.
    pub log: Option<Result<ModLog, EngineError>>,
    /// The cursor.
    pub pos: ListPos,
}

impl ModScreen {
    /// The number of rows the current tab has.
    pub fn row_count(&self) -> usize {
        match self.tab {
            ModTab::Queue => self
                .queue
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map_or(0, |q| q.items.len()),
            ModTab::Log => self
                .log
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map_or(0, |l| l.entries.len()),
        }
    }
}

/// The Notifications screen (`synthesis-v1.md` §6.11, `social.md` §16): the account's
/// notifications, newest first, with their reason kinds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NotificationsScreen {
    /// Read ones too (`a` toggles).
    pub all: bool,
    /// The list, once loaded.
    pub data: Option<NotificationList>,
    /// A live update waiting for `R`.
    pub pending: Option<NotificationList>,
    /// The cursor.
    pub pos: ListPos,
}

impl NotificationsScreen {
    fn uris(d: &NotificationList) -> Vec<String> {
        d.items.iter().map(|n| n.id.clone()).collect()
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
    /// The mod queue and log of a forum.
    Mod(Box<ModScreen>),
    /// The account's notifications.
    Notifications(Box<NotificationsScreen>),
}

/// One entry of the mod menu (`m`): a fixed label and the intent it issues (the reason is
/// prompted for after).
#[derive(Clone, Debug, PartialEq)]
pub struct ModEntry {
    /// What the menu shows.
    pub label: &'static str,
    /// The action, or `None` for the two screens the menu also opens.
    pub intent: Option<ModActionIntent>,
    /// The entry opens the queue screen.
    pub queue: bool,
    /// The entry opens the log screen.
    pub log: bool,
}

/// The mod menu overlay (`synthesis-v1.md` §6.11 "Other": `m` mod menu). The entries are the
/// forum-tier actions of `ViewerCaps.moderate`/`rank` (plus `curate` for the curated tier),
/// built for the selected item and its author.
#[derive(Clone, Debug, PartialEq)]
pub struct ModMenu {
    /// The forum.
    pub forum: String,
    /// The entries.
    pub entries: Vec<ModEntry>,
    /// The selected entry.
    pub cursor: usize,
    /// The first entry shown.
    pub scroll: usize,
    /// The screen to reload after a write.
    pub screen: ScreenId,
}

/// A one-line text prompt (the mod action's optional reason, `synthesis-v1.md` §6.11).
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// What is asked for.
    pub label: &'static str,
    /// The text so far.
    pub text: String,
    /// The action the text is the reason of.
    pub intent: ModActionIntent,
    /// Its forum.
    pub forum: String,
    /// The screen to reload after the write.
    pub screen: ScreenId,
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
    /// Consecutive live (engine-event) reloads with no user activity in between. A forum
    /// whose authority repo stays unavailable re-emits `Changed` on every load
    /// (`sync_authorities` always reports a change then), so the cap breaks what would be an
    /// endless reload loop; any key or user reload starts over.
    pub live_burst: u32,
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
            ScreenKind::Mod(m) => Effect::LoadQueue {
                screen,
                forum: m.forum.clone(),
                origin,
            },
            ScreenKind::Notifications(n) => Effect::LoadNotifications {
                screen,
                all: n.all,
                origin,
            },
        }
    }

    /// The loads of this screen (the Mod screen loads its queue and its log together).
    fn loads(&self, origin: Origin) -> Vec<Effect> {
        match &self.kind {
            ScreenKind::Mod(m) => {
                let screen = self.id;
                vec![
                    Effect::LoadQueue {
                        screen,
                        forum: m.forum.clone(),
                        origin,
                    },
                    Effect::LoadLog {
                        screen,
                        forum: m.forum.clone(),
                        origin,
                    },
                ]
            }
            _ => vec![self.load(origin)],
        }
    }

    /// True if a `Changed` topic concerns this screen.
    fn concerns(&self, topic: &Topic) -> bool {
        match (&self.kind, topic) {
            (ScreenKind::Home(_), Topic::Forum { .. } | Topic::Accounts) => true,
            (ScreenKind::Notifications(_), Topic::Notifications) => true,
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
            ScreenKind::Mod(_) => None,
            ScreenKind::Notifications(s) => {
                let p = s.pending.as_ref()?;
                Some(new_of(
                    s.data.as_ref().map(NotificationsScreen::uris).unwrap_or_default(),
                    NotificationsScreen::uris(p),
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

/// The react palette overlay (`x`, `synthesis-v1.md` §6.11, `social.md` §9). The palette is
/// the forum's (`policy.reactPalette`, `default/1` in the MVP); the overlay reacts without a
/// quote (the TUI has no quote-selection mechanism yet).
#[derive(Clone, Debug, PartialEq)]
pub struct ReactOverlay {
    /// The item reacted to.
    pub subject: String,
    /// Its forum.
    pub forum: String,
    /// `Tab` switches it.
    pub stance: Stance,
    /// The selected palette entry.
    pub cursor: usize,
    /// The first entry shown.
    pub scroll: usize,
    /// The item's whole-item tallies (`quote: None`), by react id: the counts the picker shows
    /// and the viewer's own stance (`Enter` on a react the viewer already holds removes it).
    pub tallies: BTreeMap<String, ReactTally>,
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
    /// The `x` react palette overlay.
    pub react: Option<ReactOverlay>,
    /// The `m` mod menu overlay.
    pub mod_menu: Option<ModMenu>,
    /// A one-line prompt (the mod action's reason).
    pub prompt: Option<Prompt>,
    /// The unread counts (`unread_counts()`): the header's notification badge and the Home
    /// screen's per-forum new counts.
    pub unread: Option<UnreadCounts>,
    /// The help overlay's first shown row (it scrolls with the navigation keys).
    pub help_scroll: usize,
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
    /// Opinion writes queued from this TUI, in order (a vote, an agreement vote or a react):
    /// `Backspace` undoes the last one still open (`engine.undo`, the 4 s undo window).
    pub op_writes: Vec<String>,
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
            react: None,
            mod_menu: None,
            prompt: None,
            unread: None,
            help_scroll: 0,
            status: None,
            account,
            offline,
            auto_sync: !offline,
            sync: BTreeMap::new(),
            writes: BTreeMap::new(),
            failed: BTreeSet::new(),
            own: BTreeMap::new(),
            op_writes: Vec::new(),
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
            fx.push(Effect::LoadUnread);
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
            Action::OpinionDone {
                forum: _,
                screen,
                label,
                result,
            } => self.opinion_done(screen, label, result, &mut fx),
            Action::Undone { screen, result } => self.undone(screen, result, &mut fx),
            Action::Granted { screen, result } => self.granted(screen, result, &mut fx),
            Action::ModActionDone {
                forum: _,
                screen,
                result,
            } => self.mod_action_done(screen, result, &mut fx),
            Action::QueueLoaded { screen, result, .. } => {
                self.mod_loaded(screen, Some(result), None)
            }
            Action::LogLoaded { screen, result, .. } => self.mod_loaded(screen, None, Some(result)),
            Action::NotificationsLoaded {
                screen,
                origin,
                result,
            } => self.notifications_loaded(screen, origin, result),
            Action::MarkedRead { screen, result } => self.marked_read(screen, result, &mut fx),
            Action::UnreadLoaded(result) => {
                if let Ok(u) = result {
                    self.unread = Some(u);
                }
            }
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
            ScreenKind::Mod(m) => {
                let len = m.row_count();
                m.pos.reveal(len, h);
            }
            ScreenKind::Notifications(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.items.len());
                s.pos.reveal(len, h);
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
            live_burst: 0,
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
        match origin {
            Origin::Live => {
                if s.live_burst >= LIVE_RELOAD_CAP {
                    // the loads change nothing the user can see (an authority repo that stays
                    // unavailable keeps re-announcing itself): stop until the user acts or
                    // presses R; the screen keeps what it shows
                    return;
                }
                s.live_burst += 1;
            }
            Origin::User => s.live_burst = 0,
        }
        let effects = s.loads(origin);
        s.loading += u32::try_from(effects.len()).unwrap_or(1);
        fx.extend(effects);
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
            ScreenKind::Mod(_) => {}
            ScreenKind::Notifications(s) => {
                if let Some(p) = s.pending.take() {
                    receive_list(
                        &mut s.data,
                        &mut s.pending,
                        &mut s.pos,
                        p,
                        Origin::User,
                        NotificationsScreen::uris,
                    );
                }
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
        // a prompt captures the raw keys (it edits a text line)
        if self.prompt.is_some() {
            self.prompt_key(k, fx);
            return;
        }
        let cmd = keys::command(k);
        if cmd == Some(Cmd::Quit) {
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        // while the help overlay is open, the navigation keys scroll it and any other key
        // closes it
        if self.help {
            match cmd {
                Some(Cmd::Down) => self.help_scroll = self.help_scroll.saturating_add(1),
                Some(Cmd::Up) => self.help_scroll = self.help_scroll.saturating_sub(1),
                Some(Cmd::HalfDown) => self.help_scroll = self.help_scroll.saturating_add(5),
                Some(Cmd::HalfUp) => self.help_scroll = self.help_scroll.saturating_sub(5),
                Some(Cmd::Top) => self.help_scroll = 0,
                Some(Cmd::Bottom) => self.help_scroll = usize::MAX,
                _ => {
                    self.help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }
        let Some(cmd) = cmd else {
            return;
        };
        // while the react palette is open, it captures the keys (navigation, Enter to react,
        // Tab for the stance, Back/Esc to close)
        if self.react.is_some() {
            self.react_key(cmd, fx);
            return;
        }
        // the mod menu captures the keys too
        if self.mod_menu.is_some() {
            self.mod_menu_key(cmd, fx);
            return;
        }
        match cmd {
            Cmd::Help => {
                self.help = true;
                self.help_scroll = 0;
            }
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
            Cmd::Opinion(op) => self.opinion(op, fx),
            Cmd::Undo => self.undo_last(fx),
            Cmd::ReactPalette => self.open_react(),
            Cmd::ModMenu => self.open_mod_menu(),
            Cmd::Notifications => self.open_notifications(fx),
            Cmd::MarkAllRead => self.mark_all_read(fx),
            Cmd::Stance => {
                // on the Mod screen, `Tab` switches the queue/log tab; elsewhere it is the
                // react palette's stance key, which only makes sense inside the palette
                if let Some(ScreenKind::Mod(m)) = self.stack.last().map(|s| &s.kind) {
                    let tab = match m.tab {
                        ModTab::Queue => ModTab::Log,
                        ModTab::Log => ModTab::Queue,
                    };
                    self.switch_mod_tab(tab);
                } else {
                    self.say(
                        Tone::Info,
                        lit("Open the react palette (x) to set a stance."),
                    );
                }
            }
            nav => self.navigate(nav),
        }
    }

    fn navigate(&mut self, cmd: Cmd) {
        let (w, h) = (self.width, self.content_height());
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        top.settling = false;
        top.live_burst = 0;
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
            ScreenKind::Mod(m) => {
                let len = m.row_count();
                m.pos.apply(cmd, len, h);
            }
            ScreenKind::Notifications(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.items.len());
                s.pos.apply(cmd, len, h);
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
            // on the Mod screen, Enter approves the selected queue row (an applicant)
            ScreenKind::Mod(_) => self.approve_selected(fx),
            // on the Notifications screen, Enter opens the notification's thread
            ScreenKind::Notifications(s) => {
                if let Some(n) = s
                    .data
                    .as_ref()
                    .and_then(|d| d.items.get(s.pos.cursor))
                {
                    let uri = n.subject.clone();
                    self.push_thread(uri, fx);
                }
            }
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
            ScreenKind::Mod(m) => SyncTarget::Forum(m.forum.clone()),
            ScreenKind::Notifications(_) => {
                self.say(Tone::Info, lit("Notifications are local; sync a forum with R on it."));
                return;
            }
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
            ScreenKind::Mod(m) => Some((m.forum.clone(), m.forum.clone())),
            ScreenKind::Notifications(_) => None,
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
    // opinions (M3, synthesis-v1.md §6.11)

    /// The item an opinion key applies to: its uri and its forum.
    fn opinion_subject(&self) -> Option<(String, String)> {
        match &self.stack.last()?.kind {
            ScreenKind::Forum(s) => s.selected().map(|p| (p.uri.clone(), p.forum.clone())),
            ScreenKind::Thread(t) => {
                let uri = t.layout.items.get(t.pos.selected)?.uri.clone();
                let d = t.data.as_ref()?;
                let forum = match &d.view.post {
                    ThreadRoot::Post(p) => p.post.forum.clone(),
                    ThreadRoot::Missing { .. } => return None,
                };
                Some((uri, forum))
            }
            ScreenKind::Home(_) | ScreenKind::Mod(_) | ScreenKind::Notifications(_) => None,
        }
    }

    /// The viewer caps of `forum` from the screens below (a Forum screen has its own).
    fn caps_of(&self, forum: &str) -> Option<&ViewerCaps> {
        self.stack.iter().rev().find_map(|s| match &s.kind {
            ScreenKind::Forum(f) if f.uri == forum => f.data.as_ref()?.view.viewer.as_ref(),
            _ => None,
        })
    }

    /// Why an opinion key is refused before the engine is asked, if it is (a disabled cap
    /// shows why; the engine re-checks and is authoritative).
    fn opinion_gate(&self, op: Op, caps: Option<&ViewerCaps>) -> Option<TermText> {
        let Some(c) = caps else {
            // the forum's header is not on the stack: the engine's own checks decide
            return None;
        };
        let agreed = matches!(
            op,
            Op::Agree | Op::StrongAgree | Op::Disagree | Op::StrongDisagree
        );
        let strong = matches!(op, Op::StrongUp | Op::StrongDown);
        if op != Op::Clear && !c.vote {
            return Some(lit(
                "You cannot vote here (banned, nullified or restricted from voting).",
            ));
        }
        if agreed && !c.agree {
            return Some(lit(
                "This forum does not use agreement voting (its karma policy is `karmaOnly`).",
            ));
        }
        if strong && !c.strong_vote {
            return Some(lit("Strong votes are not available to you here."));
        }
        // A non-established voter (vote but not weighted) is NOT refused: the engine records
        // the vote and returns a NotEstablished notice ("will count once you're established"),
        // which opinion_done surfaces. Nothing to gate here.
        None
    }

    /// One opinion key (`u`/`U`/`d`/`D`/`a`/`A`/`z`/`Z`/`0`).
    fn opinion(&mut self, op: Op, fx: &mut Vec<Effect>) {
        let Some((subject, forum)) = self.opinion_subject() else {
            self.say(Tone::Info, lit("Nothing selected to vote on."));
            return;
        };
        if !self.can_write() {
            return;
        }
        if let Some(why) = self.opinion_gate(op, self.caps_of(&forum)) {
            self.say(Tone::Warn, why);
            return;
        }
        let Some(screen) = self.stack.last().map(|s| s.id) else {
            return;
        };
        // `0` clears both axes: the engine coalesces the two intents into one held write.
        let intents: Vec<OpinionIntent> = match op {
            Op::Up => vec![OpinionIntent::Karma {
                value: Some(KarmaValue::Up),
            }],
            Op::StrongUp => vec![OpinionIntent::Karma {
                value: Some(KarmaValue::StrongUp),
            }],
            Op::Down => vec![OpinionIntent::Karma {
                value: Some(KarmaValue::Down),
            }],
            Op::StrongDown => vec![OpinionIntent::Karma {
                value: Some(KarmaValue::StrongDown),
            }],
            Op::Agree => vec![OpinionIntent::Agreement {
                value: Some(AgreementValue::Agree),
            }],
            Op::StrongAgree => vec![OpinionIntent::Agreement {
                value: Some(AgreementValue::StrongAgree),
            }],
            Op::Disagree => vec![OpinionIntent::Agreement {
                value: Some(AgreementValue::Disagree),
            }],
            Op::StrongDisagree => vec![OpinionIntent::Agreement {
                value: Some(AgreementValue::StrongDisagree),
            }],
            Op::Clear => vec![
                OpinionIntent::Karma { value: None },
                OpinionIntent::Agreement { value: None },
            ],
        };
        for intent in intents {
            fx.push(Effect::Opinion {
                subject: subject.clone(),
                intent,
                forum: forum.clone(),
                screen,
                label: "Vote",
            });
        }
    }

    /// `Backspace`: undoes the most recent opinion write this TUI queued that is still open
    /// (the 4 s undo window; `engine.undo`).
    fn undo_last(&mut self, fx: &mut Vec<Effect>) {
        let Some(id) = self
            .op_writes
            .iter()
            .rev()
            .find(|id| self.writes.contains_key(*id))
            .cloned()
        else {
            self.say(
                Tone::Info,
                lit("Nothing to undo: no opinion write is pending."),
            );
            return;
        };
        self.op_writes.retain(|x| *x != id);
        let screen = self.top().map_or(0, |s| s.id);
        self.say(Tone::Info, lit("Undoing..."));
        fx.push(Effect::Undo { id, screen });
    }

    // -----------------------------------------------------------------------------------------
    // reacts (M3, synthesis-v1.md §6.11, social.md §9)

    /// The forum header of `forum` from the screens below (a Forum screen has its own).
    fn forum_view_of(&self, forum: &str) -> Option<&ForumView> {
        self.stack.iter().rev().find_map(|s| match &s.kind {
            ScreenKind::Forum(f) if f.uri == forum => f.data.as_ref().map(|d| &d.view),
            _ => None,
        })
    }

    /// The whole-item react tallies (`quote: None`) of `subject`, when the screen on top holds
    /// them (a thread; a list's `PostSummary` carries no tallies).
    fn react_tallies(&self, subject: &str) -> BTreeMap<String, ReactTally> {
        let reacts: &[ReactTally] = match self.top_thread().and_then(|t| t.data.as_ref()) {
            Some(d) => match &d.view.post {
                ThreadRoot::Post(p) if p.post.uri == subject => &p.reacts,
                _ => d
                    .view
                    .comments
                    .iter()
                    .find(|c| c.uri == subject)
                    .map(|c| c.reacts.as_slice())
                    .unwrap_or(&[]),
            },
            None => &[],
        };
        reacts
            .iter()
            .filter(|t| t.quote.is_none())
            .map(|t| (t.react.clone(), t.clone()))
            .collect()
    }

    /// `x`: opens the react palette for the selected item (`social.md` §9; the TUI has no
    /// quote-selection mechanism, so the palette reacts on the whole item).
    fn open_react(&mut self) {
        let Some((subject, forum)) = self.opinion_subject() else {
            self.say(Tone::Info, lit("Nothing selected to react to."));
            return;
        };
        if !self.can_write() {
            return;
        }
        // reacts exist only under `policy.voting = reacts` (the engine is authoritative; this
        // is UX)
        if let Some(v) = self.forum_view_of(&forum) {
            let voting = v
                .policy
                .params
                .iter()
                .find(|(k, _)| k == "voting")
                .map_or("", |(_, x)| x.as_str());
            if voting != "reacts" {
                self.say(
                    Tone::Warn,
                    tt(&format!(
                        "This forum does not use reacts (its voting policy is `{voting}`)."
                    )),
                );
                return;
            }
        }
        if let Some(c) = self.caps_of(&forum) {
            if !c.vote {
                self.say(
                    Tone::Warn,
                    lit("You cannot react here (banned, nullified or restricted)."),
                );
                return;
            }
            // an eligible voter who may not react is restricted from it (a voter outside E is
            // let through: the engine records the react and says it will not count)
            if !c.react && c.weighted {
                self.say(
                    Tone::Warn,
                    lit("You are restricted from reacting in this forum."),
                );
                return;
            }
        }
        let tallies = self.react_tallies(&subject);
        self.react = Some(ReactOverlay {
            subject,
            forum,
            stance: Stance::For,
            cursor: 0,
            scroll: 0,
            tallies,
        });
    }

    /// A key while the react palette is open.
    fn react_key(&mut self, cmd: Cmd, fx: &mut Vec<Effect>) {
        let palette = lessdugin_core::model::palette::default_1();
        let n = palette.len();
        match cmd {
            Cmd::Down => self.react_move(1, n),
            Cmd::Up => self.react_move(-1, n),
            Cmd::HalfDown => self.react_move(5, n),
            Cmd::HalfUp => self.react_move(-5, n),
            Cmd::Top => {
                if let Some(o) = &mut self.react {
                    o.cursor = 0;
                    o.scroll = 0;
                }
            }
            Cmd::Bottom => {
                if let Some(o) = &mut self.react {
                    o.cursor = n.saturating_sub(1);
                }
            }
            Cmd::Stance => {
                if let Some(o) = &mut self.react {
                    o.stance = match o.stance {
                        Stance::For => Stance::Against,
                        Stance::Against => Stance::For,
                    };
                }
            }
            Cmd::Open => self.react_choose(fx),
            _ => self.react = None,
        }
    }

    /// Moves the palette's cursor by `d` (negative: up), revealing it in the scroll window.
    fn react_move(&mut self, d: i64, n: usize) {
        let rows = self.react_rows().max(1);
        let Some(o) = &mut self.react else {
            return;
        };
        let cur = o.cursor as i64 + d;
        o.cursor = cur.clamp(0, n.saturating_sub(1) as i64) as usize;
        if o.cursor < o.scroll {
            o.scroll = o.cursor;
        } else if o.cursor >= o.scroll + rows {
            o.scroll = o.cursor + 1 - rows;
        }
    }

    /// The palette rows that fit on the screen.
    fn react_rows(&self) -> usize {
        let h = usize::from(self.height.saturating_sub(4));
        let n = lessdugin_core::model::palette::default_1().len();
        h.min(n)
    }

    /// `Enter`: reacts with the selected palette entry (the viewer's own react of the same
    /// stance toggles off; a stance switch removes and re-adds, as the engine coalesces the
    /// two into one opinion record).
    fn react_choose(&mut self, fx: &mut Vec<Effect>) {
        let Some(o) = self.react.clone() else {
            return;
        };
        let palette = lessdugin_core::model::palette::default_1();
        let Some(info) = palette.get(o.cursor) else {
            return;
        };
        let id = info.id.clone();
        let existing = o.tallies.get(&id).and_then(|t| t.mine);
        if o.stance == Stance::Against && !o.tallies.contains_key(&id) {
            self.say(
                Tone::Warn,
                tt(&format!(
                    "Nobody has reacted {id} here yet, so there is nothing to react against."
                )),
            );
            return;
        }
        let Some(screen) = self.stack.last().map(|s| s.id) else {
            return;
        };
        let react = |stance, remove| Effect::Opinion {
            subject: o.subject.clone(),
            intent: OpinionIntent::React {
                id: id.clone(),
                stance,
                quote: None,
                remove,
            },
            forum: o.forum.clone(),
            screen,
            label: "React",
        };
        match existing {
            // the viewer already holds this stance: take it back
            Some(s) if s == o.stance => fx.push(react(o.stance, true)),
            // the opposite stance: remove it, then add the new one (one opinion record)
            Some(s) => {
                fx.push(react(s, true));
                fx.push(react(o.stance, false));
            }
            None => fx.push(react(o.stance, false)),
        }
        self.react = None;
        self.say(Tone::Info, lit("Recording the react..."));
    }

    // -----------------------------------------------------------------------------------------
    // moderation (M3, synthesis-v1.md §6.11, social.md §4.4)

    /// The item and author the mod menu acts on: `(item uri, forum, author did)`. The author
    /// is the selected comment's author, or the thread's post author, or (on a list) the
    /// selected post's author.
    fn mod_subject(&self) -> Option<(String, String, String)> {
        match &self.stack.last()?.kind {
            ScreenKind::Forum(s) => s
                .selected()
                .map(|p| (p.uri.clone(), p.forum.clone(), p.author.did.clone())),
            ScreenKind::Thread(t) => {
                let d = t.data.as_ref()?;
                let forum = match &d.view.post {
                    ThreadRoot::Post(p) => p.post.forum.clone(),
                    ThreadRoot::Missing { .. } => return None,
                };
                let uri = t.layout.items.get(t.pos.selected)?.uri.clone();
                let author = match d.view.comments.iter().find(|c| c.uri == uri) {
                    Some(c) => c.author.did.clone(),
                    None => match &d.view.post {
                        ThreadRoot::Post(p) => p.post.author.did.clone(),
                        ThreadRoot::Missing { .. } => return None,
                    },
                };
                Some((uri, forum, author))
            }
            _ => None,
        }
    }

    /// `m`: opens the mod menu for the selected item (and its author) when the viewer may
    /// moderate the forum (`ViewerCaps.moderate`, §4.4), else says why not.
    fn open_mod_menu(&mut self) {
        let Some((item, forum, author)) = self.mod_subject() else {
            self.say(Tone::Info, lit("Nothing selected to moderate."));
            return;
        };
        let caps = self.caps_of(&forum).cloned();
        let Some(c) = caps.as_ref() else {
            // no forum header on the stack: the engine's own checks decide
            self.mod_menu = Some(self.mod_menu_for(&forum, &item, &author, None));
            return;
        };
        if !c.moderate && !c.author_mod_personal && !c.author_mod_frontpage {
            self.say(
                Tone::Warn,
                lit("You cannot moderate this forum (not an authority)."),
            );
            return;
        }
        self.mod_menu = Some(self.mod_menu_for(&forum, &item, &author, Some(c)));
    }

    /// The menu's entries for the selected item and author, from the viewer's caps
    /// (`ViewerCaps.moderate`/`rank`/`curate`; `social.md` §4.4).
    fn mod_menu_for(
        &self,
        forum: &str,
        item: &str,
        author: &str,
        caps: Option<&ViewerCaps>,
    ) -> ModMenu {
        let moderate = caps.is_none_or(|c| c.moderate);
        let curate = caps.is_none_or(|c| c.curate);
        let me = self.account.as_ref().map(|a| a.did.as_str()).unwrap_or("");
        let own_thread = me == author;
        let assert = |action: ModActionValue| ModActionIntent::Assert {
            forum: forum.to_owned(),
            scope: ModScope::Forum,
            subject: item.to_owned(),
            action,
            reason: None,
            until_ms: None,
        };
        let remove = |key: &'static str| ModActionIntent::Remove {
            forum: forum.to_owned(),
            scope: ModScope::Forum,
            subject: item.to_owned(),
            action_key: key.to_owned(),
        };
        let user_assert = |action: ModActionValue| ModActionIntent::Assert {
            forum: forum.to_owned(),
            scope: ModScope::Forum,
            subject: author.to_owned(),
            action,
            reason: None,
            until_ms: None,
        };
        let user_remove = |key: &'static str| ModActionIntent::Remove {
            forum: forum.to_owned(),
            scope: ModScope::Forum,
            subject: author.to_owned(),
            action_key: key.to_owned(),
        };
        let mut e = Vec::new();
        let entry = |label: &'static str, intent| ModEntry {
            label,
            intent: Some(intent),
            queue: false,
            log: false,
        };
        if moderate {
            e.push(entry(
                "hide this item",
                assert(ModActionValue::Hide {
                    hidden: true,
                    spam: false,
                    show_reason: true,
                }),
            ));
            e.push(entry(
                "hide as spam",
                assert(ModActionValue::Hide {
                    hidden: true,
                    spam: true,
                    show_reason: true,
                }),
            ));
            e.push(entry("unhide (keep)", remove("hide")));
            e.push(entry(
                "ban the author",
                user_assert(ModActionValue::Access {
                    level: AccessLevel::Banned,
                }),
            ));
            e.push(entry("unban the author", user_remove("access")));
            e.push(entry(
                "pin in the forum",
                assert(ModActionValue::Pin {
                    scope: PinScope::Forum,
                    pinned: true,
                }),
            ));
            e.push(entry("unpin", remove("pin:forum")));
            e.push(entry("lock", assert(ModActionValue::Lock { locked: true })));
            e.push(entry("unlock", remove("lock")));
            e.push(entry(
                "tier: personal",
                assert(ModActionValue::Tier {
                    tier: Tier::Personal,
                    notice: None,
                }),
            ));
            e.push(entry(
                "tier: frontpage",
                assert(ModActionValue::Tier {
                    tier: Tier::Frontpage,
                    notice: None,
                }),
            ));
            if curate {
                e.push(entry(
                    "tier: curated",
                    assert(ModActionValue::Tier {
                        tier: Tier::Curated,
                        notice: None,
                    }),
                ));
            }
            e.push(entry(
                "restrict the author (posting)",
                user_assert(ModActionValue::Restrict {
                    capabilities: vec![Capability::Post, Capability::Comment],
                    nullify_votes: false,
                }),
            ));
            e.push(entry(
                "nullify the author's votes",
                user_assert(ModActionValue::Restrict {
                    capabilities: vec![],
                    nullify_votes: true,
                }),
            ));
            e.push(entry("unrestrict", user_remove("restrict")));
        }
        if !moderate && own_thread {
            // author scope on one's own thread (social.md §4.9): hide a comment, ban a user
            let hide = ModActionIntent::Assert {
                forum: forum.to_owned(),
                scope: ModScope::Author,
                subject: item.to_owned(),
                action: ModActionValue::Hide {
                    hidden: true,
                    spam: false,
                    show_reason: true,
                },
                reason: None,
                until_ms: None,
            };
            e.push(entry("hide this comment (your thread)", hide));
            let ban = ModActionIntent::Assert {
                forum: forum.to_owned(),
                scope: ModScope::Author,
                subject: author.to_owned(),
                action: ModActionValue::Access {
                    level: AccessLevel::Banned,
                },
                reason: None,
                until_ms: None,
            };
            e.push(entry("ban the author (your threads)", ban));
        }
        e.push(ModEntry {
            label: "mod queue",
            intent: None,
            queue: true,
            log: false,
        });
        e.push(ModEntry {
            label: "mod log",
            intent: None,
            queue: false,
            log: true,
        });
        ModMenu {
            forum: forum.to_owned(),
            entries: e,
            cursor: 0,
            scroll: 0,
            screen: self.stack.last().map_or(0, |s| s.id),
        }
    }

    /// A key while the mod menu is open.
    fn mod_menu_key(&mut self, cmd: Cmd, fx: &mut Vec<Effect>) {
        let n = self.mod_menu.as_ref().map_or(0, |m| m.entries.len());
        match cmd {
            Cmd::Down => self.mod_move(1, n),
            Cmd::Up => self.mod_move(-1, n),
            Cmd::Top => {
                if let Some(m) = &mut self.mod_menu {
                    m.cursor = 0;
                    m.scroll = 0;
                }
            }
            Cmd::Bottom => {
                if let Some(m) = &mut self.mod_menu {
                    m.cursor = n.saturating_sub(1);
                }
            }
            Cmd::Open => self.mod_choose(fx),
            // anything else closes the menu
            _ => self.mod_menu = None,
        }
    }

    /// Moves the menu's cursor by `d`, revealing it.
    fn mod_move(&mut self, d: i64, n: usize) {
        let rows = self.mod_rows().max(1);
        let Some(m) = &mut self.mod_menu else {
            return;
        };
        let cur = m.cursor as i64 + d;
        m.cursor = cur.clamp(0, n.saturating_sub(1) as i64) as usize;
        if m.cursor < m.scroll {
            m.scroll = m.cursor;
        } else if m.cursor >= m.scroll + rows {
            m.scroll = m.cursor + 1 - rows;
        }
    }

    /// The menu rows that fit on the screen.
    fn mod_rows(&self) -> usize {
        let h = usize::from(self.height.saturating_sub(6));
        let n = self.mod_menu.as_ref().map_or(0, |m| m.entries.len());
        h.clamp(1, n.max(1))
    }

    /// `Enter`: runs the selected entry (an action goes through the reason prompt; the queue
    /// and log entries open their screens).
    fn mod_choose(&mut self, fx: &mut Vec<Effect>) {
        let Some(m) = self.mod_menu.clone() else {
            return;
        };
        let Some(e) = m.entries.get(m.cursor) else {
            return;
        };
        if e.queue || e.log {
            self.mod_menu = None;
            self.open_mod_screen(e.queue, fx);
            return;
        }
        let Some(intent) = e.intent.clone() else {
            return;
        };
        self.mod_menu = None;
        // ask for the optional reason before the write
        self.prompt = Some(Prompt {
            label: e.label,
            text: String::new(),
            intent,
            forum: m.forum.clone(),
            screen: m.screen,
        });
    }

    /// `Enter` on the Mod screen: approves the selected queue row when it is an applicant
    /// (`engine.grant`, `social.md` §4.7). Other queue rows and log rows have no action.
    fn approve_selected(&mut self, fx: &mut Vec<Effect>) {
        let Some(m) = self.stack.last().and_then(|s| match &s.kind {
            ScreenKind::Mod(m) => Some(m.clone()),
            _ => None,
        }) else {
            return;
        };
        if m.tab != ModTab::Queue {
            return;
        }
        let Some(q) = m.queue.as_ref().and_then(|r| r.as_ref().ok()) else {
            return;
        };
        let Some(item) = q.items.get(m.pos.cursor) else {
            return;
        };
        if item.item_kind != "applicant" {
            self.say(
                Tone::Info,
                lit("Only an applicant can be approved from the queue."),
            );
            return;
        }
        let Some(screen) = self.stack.last().map(|s| s.id) else {
            return;
        };
        self.say(Tone::Info, lit("Approving..."));
        fx.push(Effect::Grant {
            forum: m.forum.clone(),
            actors: vec![item.subject.clone()],
            screen,
        });
    }

    /// [`Action::Granted`]: reloads the queue after an approval.
    fn granted(
        &mut self,
        screen: ScreenId,
        result: Result<Vec<WriteTicket>, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(tickets) => {
                let n = u32::try_from(tickets.len()).unwrap_or(u32::MAX);
                for t in tickets {
                    if !matches!(
                        t.state,
                        WriteState::Done { .. } | WriteState::Failed { .. } | WriteState::Undone
                    ) {
                        self.writes.insert(t.id.clone(), t.state);
                    }
                }
                self.say(
                    Tone::Good,
                    cat([count(n, "applicant", "applicants"), lit(" approved.")]),
                );
                self.follow_own = Some(screen);
                if let Some(i) = self.index_of(screen) {
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => self.say(
                Tone::Error,
                cat([lit("Approval refused: "), tt(&e.message)]),
            ),
        }
    }

    /// Switches the Mod screen's tab (queue/log), resetting its cursor.
    fn switch_mod_tab(&mut self, tab: ModTab) {
        if let Some(ScreenKind::Mod(m)) = self.stack.last_mut().map(|s| &mut s.kind) {
            m.tab = tab;
            m.pos = ListPos::default();
        }
    }

    /// `N`: opens the Notifications screen on top of Home.
    fn open_notifications(&mut self, fx: &mut Vec<Effect>) {
        if self.account.is_none() {
            self.say(
                Tone::Info,
                lit("Notifications need an account: run `lessdugin login`."),
            );
            return;
        }
        self.push(
            ScreenKind::Notifications(Box::new(NotificationsScreen::default())),
            fx,
        );
    }

    /// `M` (or the notifications' opening): marks every listed notification read
    /// (`engine.mark_notifications_read`).
    fn mark_all_read(&mut self, fx: &mut Vec<Effect>) {
        let Some(s) = self.stack.last().and_then(|s| match &s.kind {
            ScreenKind::Notifications(s) => Some(s.clone()),
            _ => None,
        }) else {
            self.say(
                Tone::Info,
                lit("Mark notifications read on the notifications screen (N)."),
            );
            return;
        };
        let ids: Vec<String> = s
            .data
            .as_ref()
            .map(|d| {
                d.items
                    .iter()
                    .filter(|n| !n.read)
                    .map(|n| n.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            self.say(Tone::Info, lit("Nothing to mark read."));
            return;
        }
        let screen = self.stack.last().map_or(0, |x| x.id);
        fx.push(Effect::MarkRead { ids, screen });
    }

    /// [`Action::NotificationsLoaded`].
    fn notifications_loaded(
        &mut self,
        screen: ScreenId,
        origin: Origin,
        result: Result<NotificationList, EngineError>,
    ) {
        let Some(i) = self.index_of(screen) else {
            return;
        };
        let s = &mut self.stack[i];
        s.loading = s.loading.saturating_sub(1);
        let ScreenKind::Notifications(n) = &mut s.kind else {
            return;
        };
        match result {
            Ok(list) => {
                receive_list(
                    &mut n.data,
                    &mut n.pending,
                    &mut n.pos,
                    list,
                    origin,
                    NotificationsScreen::uris,
                );
            }
            Err(e) => s.error = Some(e),
        }
    }

    /// [`Action::MarkedRead`]: refreshes the list and the badge.
    fn marked_read(
        &mut self,
        screen: ScreenId,
        result: Result<(), EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(()) => {
                self.unread = None;
                fx.push(Effect::LoadUnread);
                if let Some(i) = self.index_of(screen) {
                    self.reload(i, Origin::Live, fx);
                }
            }
            Err(e) => self.say(Tone::Error, cat([lit("Marking read failed: "), tt(&e.message)])),
        }
    }

    /// Opens the Mod screen (queue and log) for the forum under the top screen.
    fn open_mod_screen(&mut self, queue: bool, fx: &mut Vec<Effect>) {
        let Some(forum) = self.top_forum() else {
            self.say(Tone::Info, lit("Open a forum first."));
            return;
        };
        self.push(
            ScreenKind::Mod(Box::new(ModScreen {
                forum,
                tab: if queue { ModTab::Queue } else { ModTab::Log },
                ..ModScreen::default()
            })),
            fx,
        );
    }

    /// The forum uri of the screen on top (or under it, for a thread).
    fn top_forum(&self) -> Option<String> {
        for s in self.stack.iter().rev() {
            match &s.kind {
                ScreenKind::Forum(f) => return Some(f.uri.clone()),
                ScreenKind::Mod(m) => return Some(m.forum.clone()),
                ScreenKind::Thread(t) => {
                    if let Some(p) = t.post() {
                        return Some(p.post.forum.clone());
                    }
                }
                ScreenKind::Home(_) | ScreenKind::Notifications(_) => {}
            }
        }
        None
    }

    /// A key while a prompt is open: printable characters edit the text, Enter runs the
    /// action, Esc closes it.
    fn prompt_key(&mut self, k: &KeyEvent, fx: &mut Vec<Effect>) {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c')) {
            self.prompt = None;
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        match k.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let Some(p) = self.prompt.clone() else {
                    return;
                };
                self.prompt = None;
                let intent = match &p.intent {
                    ModActionIntent::Assert {
                        forum,
                        scope,
                        subject,
                        action,
                        until_ms,
                        ..
                    } => ModActionIntent::Assert {
                        forum: forum.clone(),
                        scope: *scope,
                        subject: subject.clone(),
                        action: action.clone(),
                        reason: (!p.text.trim().is_empty()).then(|| p.text.trim().to_owned()),
                        until_ms: *until_ms,
                    },
                    r => r.clone(),
                };
                fx.push(Effect::ModAction {
                    intent,
                    forum: p.forum,
                    screen: p.screen,
                });
            }
            KeyCode::Backspace => {
                if let Some(p) = &mut self.prompt {
                    p.text.pop();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(p) = &mut self.prompt {
                    // a reason is prose: keep printable text, drop control characters
                    if !c.is_control() {
                        p.text.push(c);
                    }
                }
            }
            _ => {}
        }
    }

    /// [`Action::ModActionDone`]: the notices, or the refusal.
    fn mod_action_done(
        &mut self,
        screen: ScreenId,
        result: Result<ActionTicket, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(t) => {
                if !matches!(
                    t.state,
                    WriteState::Done { .. } | WriteState::Failed { .. } | WriteState::Undone
                ) {
                    self.writes.insert(t.id.clone(), t.state);
                }
                let mut msg = lit("Mod action recorded.");
                if let Some(n) = t.notices.first() {
                    msg = cat([lit("Mod action recorded: "), tt(&n.message), lit(".")]);
                }
                self.say(Tone::Good, msg);
                self.follow_own = Some(screen);
                if let Some(i) = self.index_of(screen) {
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => self.say(
                Tone::Error,
                cat([lit("Mod action refused: "), tt(&e.message)]),
            ),
        }
    }

    /// [`Action::QueueLoaded`] / [`Action::LogLoaded`].
    fn mod_loaded(
        &mut self,
        screen: ScreenId,
        queue: Option<Result<ModQueue, EngineError>>,
        log: Option<Result<ModLog, EngineError>>,
    ) {
        let Some(i) = self.index_of(screen) else {
            return;
        };
        let s = &mut self.stack[i];
        s.loading = s.loading.saturating_sub(1);
        let ScreenKind::Mod(m) = &mut s.kind else {
            return;
        };
        if let Some(q) = queue {
            // `blocked` for a viewer who is not an authority is not an error: the screen says so
            m.queue = Some(q);
        }
        if let Some(l) = log {
            m.log = Some(l);
        }
        let len = m.row_count();
        m.pos.reveal(len, usize::from(self.height));
    }

    /// [`Action::OpinionDone`]: notices and the undo hint, or the refusal.
    fn opinion_done(
        &mut self,
        screen: ScreenId,
        label: &'static str,
        result: Result<ActionTicket, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(t) => {
                if !matches!(
                    t.state,
                    WriteState::Done { .. } | WriteState::Failed { .. } | WriteState::Undone
                ) {
                    if !self.op_writes.contains(&t.id) {
                        self.op_writes.push(t.id.clone());
                    }
                    self.writes.insert(t.id.clone(), t.state);
                }
                let mut msg = cat([lit(label), lit(" recorded (Backspace undoes).")]);
                if let Some(n) = t.notices.first() {
                    msg = cat([
                        lit(label),
                        lit(" recorded: "),
                        tt(&n.message),
                        lit(" (Backspace undoes)."),
                    ]);
                }
                self.say(Tone::Good, msg);
                self.follow_own = Some(screen);
                if let Some(i) = self.index_of(screen) {
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => {
                self.say(Tone::Error, cat([lit("Not recorded: "), tt(&e.message)]));
            }
        }
    }

    /// [`Action::Undone`].
    fn undone(
        &mut self,
        screen: ScreenId,
        result: Result<WriteTicket, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(t) if matches!(t.state, WriteState::Undone) => {
                self.writes.remove(&t.id);
                self.failed.remove(&t.id);
                self.say(Tone::Good, lit("Undone: the vote was not sent."));
                if let Some(i) = self.index_of(screen) {
                    self.reload(i, Origin::User, fx);
                }
            }
            Ok(t) => {
                // it left the undo window before the key arrived
                self.writes.insert(t.id, t.state);
                self.say(
                    Tone::Warn,
                    lit("Too late to undo: the vote was already sent. Press 0 to clear it."),
                );
            }
            Err(e) => self.say(Tone::Error, cat([lit("Undo failed: "), tt(&e.message)])),
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
                // the header's badge follows the notifications
                if topics
                    .iter()
                    .any(|t| matches!(t, Topic::Notifications | Topic::Forum { .. }))
                {
                    fx.push(Effect::LoadUnread);
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
                    self.op_writes.retain(|x| *x != id);
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
                    self.op_writes.retain(|x| *x != id);
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
