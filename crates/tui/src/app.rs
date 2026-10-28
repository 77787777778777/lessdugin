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
    AccessLevel, AccountView, ActionNotice, ActionTicket, AgreementValue, Capability, CommentDraft, CommentSort,
    DiscussionList, Event, ForumSummary, ForumView, HistoryView, KarmaValue, ModActionIntent,
    ModActionValue, ModLog, ModQueue, ModScope, ModsChange, NotificationList, OpinionIntent,
    PinScope, PostDraft, PostFilter, PostList, PostSort, PublicActionKind, ReactTally, Stance,
    SyncReport, ThreadOpts, ThreadRoot, ThreadView, Tier, Topic, UnreadCounts, ViewerCaps,
    WriteState, WriteTicket,
};
use lessdugin_core::dto::{
    Draft, ForumDraft, ForumRole, KarmaCadence, MemberView, MembershipMode, PostKind, RawView, Ref,
    Settings, Subscription, TextQuote,
};
use lessdugin_core::engine::EngineError;

use crate::body::Body;
use crate::command::{AccessOp, ForumChanges, RestrictDim};
use crate::editor;
use crate::image::{MediaBytes, Placeholder};
use crate::keys::{self, Cmd, Op};
use crate::text::{TermText, actor, cat, count, lit, post_title, tt};
use crate::thread::{self, Layout, MediaState, Pos};

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
#[derive(Clone, Debug, PartialEq)]
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
    /// An edit of your post or comment `uri` (`:edit`): `base` is its current draft, whose
    /// text (and a post's title) the editor's text replaces.
    Edit {
        /// The item.
        uri: String,
        /// Its draft as cached.
        base: Box<Draft>,
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
            Compose::Edit { .. } => "edit",
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
            Compose::Edit { .. } => ":edit",
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
#[derive(Clone, Debug, PartialEq)]
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
    /// `forum_open` and `list_posts` (`magic` over `frontpage` when it opens, the M3 default).
    LoadForum {
        /// The screen.
        screen: ScreenId,
        /// The forum uri.
        forum: String,
        /// The sort.
        sort: PostSort,
        /// The filter (`f`).
        filter: PostFilter,
        /// Who asked.
        origin: Origin,
    },
    /// `get_thread` and the bodies.
    LoadThread {
        /// The screen.
        screen: ScreenId,
        /// The post or comment uri.
        uri: String,
        /// The comment sort (`s` on the thread).
        opts: ThreadOpts,
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
    /// `mods_set(forum, change)`: adds or removes a moderator (`data-model.md` §6.1, `mods`).
    ModsSet {
        /// The forum.
        forum: String,
        /// The change.
        change: ModsChange,
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
        /// How a `#restrict` assertion is completed (read when the effect runs).
        merge: Option<RestrictDim>,
    },
    /// `recent_discussion(forum, None, limit)`: the Forum screen's Activity tab
    /// (`synthesis-v1.md` §6.11, `social.md` §12.5).
    LoadActivity {
        /// The screen.
        screen: ScreenId,
        /// The forum uri.
        forum: String,
        /// Who asked.
        origin: Origin,
    },
    /// `notifications(q)` (the Notifications screen).
    /// `history` (`engine-api.md` §3.6): the version panel of the opened item.
    LoadHistory {
        /// The screen.
        screen: ScreenId,
        /// The item.
        uri: String,
        /// Who asked.
        origin: Origin,
    },
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
    /// `mark_all_notifications_read()`: every notification, listed or not (`M`, `:read-all`).
    MarkAllRead {
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `unread_counts()`: the notification badge and the forums' unread state.
    LoadUnread,
    /// `mark_read(post)`: the thread was viewed (`social.md` §16: "viewing a thread sets
    /// lastVisited"); its unread marks and notifications clear.
    MarkThreadRead {
        /// The thread's post.
        post: String,
    },
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
    /// `acknowledged_public_actions()`: the "this is public" interstitials the account has
    /// seen (`engine-api.md` §3.6).
    LoadAcknowledged,
    /// `acknowledge_public_action(kind)`: the user went past the interstitial of `kind`.
    Acknowledge {
        /// The kind.
        kind: PublicActionKind,
    },
    /// `parse_ref(target)`, then the screen of the forum, post or comment it names (`:open`).
    Open {
        /// The reference.
        target: String,
    },
    /// `forum_follow(forum)` or `forum_unfollow(forum)`.
    Follow {
        /// The forum.
        forum: String,
        /// Follow, or stop.
        on: bool,
    },
    /// `join(forum, note)`: a public action (`PublicActionKind::Join`).
    Join {
        /// The forum.
        forum: String,
        /// The membership note.
        note: Option<String>,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `leave(forum)`.
    Leave {
        /// The forum.
        forum: String,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `members(forum, query)`, shown in a panel.
    LoadMembers {
        /// The forum.
        forum: String,
        /// The role kept.
        status: Option<ForumRole>,
        /// The applicants only.
        pending: bool,
    },
    /// `grant`, `revoke`, `approve` or `reject` of `actors`.
    Access {
        /// Which.
        op: AccessOp,
        /// The forum.
        forum: String,
        /// The DIDs or handles.
        actors: Vec<String>,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `mod_adopt(forum, former)`.
    Adopt {
        /// The forum.
        forum: String,
        /// The former moderator.
        former: String,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `forum_create(draft)`.
    ForumCreate {
        /// The new forum.
        draft: ForumDraft,
    },
    /// `forum_update(forum, draft)`, the draft being the cached declaration with `changes`.
    ForumEdit {
        /// The forum.
        forum: String,
        /// What changes.
        changes: ForumChanges,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// Reads your post or comment `uri` from the cache and opens `$EDITOR` on its text.
    StartEdit {
        /// The item (or a reference to it).
        uri: String,
        /// Its forum, for a reference.
        forum: String,
    },
    /// `edit(uri, draft, EditOpts::default())`: the old version is kept as a revision.
    Edit {
        /// The item.
        uri: String,
        /// The new draft.
        draft: Draft,
        /// The editor's text (offered again if the edit fails).
        text: String,
    },
    /// `delete(uri)`, `retract(uri)` or `redact_revision(uri)` of your post or comment.
    ItemWrite {
        /// Which.
        op: ItemOp,
        /// The item (or a reference to it).
        uri: String,
        /// Its forum, for a reference.
        forum: String,
        /// The screen to reload afterwards.
        screen: ScreenId,
    },
    /// `raw(uri)`, shown in a panel.
    LoadRaw {
        /// The item (or a reference to it).
        uri: String,
        /// Its forum, for a reference.
        forum: String,
    },
    /// `bookmark(uri, on)`.
    Bookmark {
        /// The post (or a reference to it).
        uri: String,
        /// Its forum, for a reference.
        forum: String,
        /// Bookmark, or remove it.
        on: bool,
    },
    /// `subscribe(subscription, on)` (`setSubscription` in TypeScript).
    Subscribe {
        /// What.
        subscription: Subscription,
        /// Subscribe, or stop.
        on: bool,
    },
    /// `mute(actor, on)`.
    Mute {
        /// The DID or handle.
        actor: String,
        /// Mute, or unmute.
        on: bool,
    },
    /// `karma_notifications()`, or `set_karma_notifications(cadence)`.
    KarmaNotifications {
        /// The cadence to set; `None` shows it.
        cadence: Option<KarmaCadence>,
    },
    /// `settings()`, after `set_setting(key, value)` when a key is given.
    Settings {
        /// The key and its value (`None`: the default).
        set: Option<(String, Option<String>)>,
    },
    /// `retry_write(id)`.
    Retry {
        /// The write id.
        id: String,
    },
    /// `load_media(cid, media)` of an image from its author's PDS (streamed and CID-checked by
    /// the host), then its bytes for the decoder.
    FetchMedia {
        /// The image as the item's body lists it (its CID, author, declared type and size).
        media: Box<lessdugin_core::dto::MediaView>,
    },
    /// Decodes a fetched image off the UI thread: the runtime's [`crate::image::Images`].
    DecodeMedia {
        /// The blob's CID.
        cid: String,
        /// Its bytes.
        bytes: MediaBytes,
    },
    /// Leave the TUI.
    Quit,
}

/// The writes of `:delete`, `:retract` and `:redact` on your own item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemOp {
    /// `delete(uri)`.
    Delete,
    /// `retract(uri)`.
    Retract,
    /// `redact_revision(uri)`.
    Redact,
}

impl Effect {
    /// The public action this effect writes (`social.md` §17), whose first-use interstitial
    /// must have been seen before it runs: an opinion by its intent; a mod action, an
    /// `#access` grant or a moderator change is a mod action.
    pub fn public_kind(&self) -> Option<PublicActionKind> {
        match self {
            Effect::Opinion { intent, .. } => Some(intent.public_kind()),
            Effect::ModAction { .. }
            | Effect::Grant { .. }
            | Effect::ModsSet { .. }
            | Effect::Adopt { .. }
            | Effect::Access {
                op: AccessOp::Grant | AccessOp::Revoke | AccessOp::Approve,
                ..
            } => Some(PublicActionKind::ModAction),
            Effect::Join { .. } => Some(PublicActionKind::Join),
            _ => None,
        }
    }

    /// The engine operation (`engine-api.md` §3) the effect calls, by its name there (for an
    /// editor session, the write it leads to; for `Open`, the screen's `forum_open` or
    /// `get_thread`); `None` for what the runtime does itself
    /// (decoding an image, quitting).
    pub fn operation(&self) -> Option<&'static str> {
        Some(match self {
            Effect::Editor(req) => match req.compose {
                Compose::Comment { .. } => "comment",
                Compose::Post { .. } => "publish",
                Compose::Edit { .. } => "edit",
            },
            Effect::LoadHome { .. } => "followed",
            Effect::LoadForum { .. } => "list_posts",
            Effect::LoadThread { .. } => "get_thread",
            Effect::Sync { .. } => "sync_forum",
            Effect::Comment { .. } => "comment",
            Effect::Publish { .. } => "publish",
            Effect::Opinion { .. } => "opinion",
            Effect::Undo { .. } => "undo",
            Effect::Grant { .. } => "grant",
            Effect::ModsSet { .. } => "mods_set",
            Effect::ModAction { .. } => "mod_action",
            Effect::LoadActivity { .. } => "recent_discussion",
            Effect::LoadHistory { .. } => "history",
            Effect::LoadNotifications { .. } => "notifications",
            Effect::MarkRead { .. } => "mark_notifications_read",
            Effect::MarkAllRead { .. } => "mark_all_notifications_read",
            Effect::LoadUnread => "unread_counts",
            Effect::MarkThreadRead { .. } => "mark_read",
            Effect::LoadQueue { .. } => "mod_queue",
            Effect::LoadLog { .. } => "mod_log",
            Effect::LoadWrites => "writes",
            Effect::LoadAcknowledged => "acknowledged_public_actions",
            Effect::Acknowledge { .. } => "acknowledge_public_action",
            Effect::Open { .. } => "forum_open",
            Effect::Follow { on: true, .. } => "forum_follow",
            Effect::Follow { on: false, .. } => "forum_unfollow",
            Effect::Join { .. } => "join",
            Effect::Leave { .. } => "leave",
            Effect::LoadMembers { .. } => "members",
            Effect::Access { op, .. } => match op {
                AccessOp::Grant => "grant",
                AccessOp::Revoke => "revoke",
                AccessOp::Approve => "approve",
                AccessOp::Reject => "reject",
            },
            Effect::Adopt { .. } => "mod_adopt",
            Effect::ForumCreate { .. } => "forum_create",
            Effect::ForumEdit { .. } => "forum_update",
            Effect::StartEdit { .. } | Effect::Edit { .. } => "edit",
            Effect::ItemWrite { op, .. } => match op {
                ItemOp::Delete => "delete",
                ItemOp::Retract => "retract",
                ItemOp::Redact => "redact_revision",
            },
            Effect::LoadRaw { .. } => "raw",
            Effect::Bookmark { .. } => "bookmark",
            Effect::Subscribe { .. } => "subscribe",
            Effect::Mute { .. } => "mute",
            Effect::KarmaNotifications { cadence: None } => "karma_notifications",
            Effect::KarmaNotifications { cadence: Some(_) } => "set_karma_notifications",
            Effect::Settings { set: None } => "settings",
            Effect::Settings { set: Some(_) } => "set_setting",
            Effect::Retry { .. } => "retry_write",
            Effect::FetchMedia { .. } => "load_media",
            Effect::DecodeMedia { .. } | Effect::Quit => return None,
        })
    }
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
    /// [`Effect::ModsSet`] finished.
    ModsSetDone {
        /// The screen to reload.
        screen: ScreenId,
        /// The action ticket, or why it was refused.
        result: Result<ActionTicket, EngineError>,
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
    /// [`Effect::LoadActivity`] finished.
    ActivityLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The list.
        result: Result<Box<ActivityData>, EngineError>,
    },
    /// [`Effect::LoadHistory`] finished.
    HistoryLoaded {
        /// The screen.
        screen: ScreenId,
        /// Who asked.
        origin: Origin,
        /// The versions.
        result: Result<HistoryView, String>,
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
    /// [`Effect::LoadAcknowledged`] finished.
    AcknowledgedLoaded {
        /// The account they were read for (the active one; `None`: no account).
        account: Option<String>,
        /// Its acknowledgements.
        result: Result<Vec<PublicActionKind>, EngineError>,
    },
    /// [`Effect::Open`] resolved its reference.
    Opened(Result<Ref, EngineError>),
    /// A write a `:` command queued (or why it was refused); `what` names it.
    Written {
        /// What the status line calls it (`"Join"`, `"Delete"`).
        what: &'static str,
        /// The screen to reload.
        screen: Option<ScreenId>,
        /// The tickets.
        result: Result<Vec<WriteTicket>, EngineError>,
    },
    /// A local-state command finished: what the status line says, or why it failed.
    LocalDone(Result<String, EngineError>),
    /// [`Effect::StartEdit`]: the editor session and what the edit would suspend
    /// (`edit_preflight`), or why there is none.
    EditReady(Result<(EditorRequest, Vec<ActionNotice>), EngineError>),
    /// [`Effect::LoadMembers`] finished.
    MembersLoaded(Result<Vec<MemberView>, EngineError>),
    /// [`Effect::LoadRaw`] finished.
    RawLoaded(Result<RawView, EngineError>),
    /// [`Effect::Settings`] finished.
    SettingsLoaded(Result<Settings, EngineError>),
    /// [`Effect::FetchMedia`] finished.
    MediaFetched {
        /// The blob's CID.
        cid: String,
        /// Its bytes, or why there are none.
        result: Result<MediaBytes, EngineError>,
    },
    /// An image's decode finished: it is drawn, or why not.
    MediaDecoded {
        /// The blob's CID.
        cid: String,
        /// The outcome.
        result: Result<(), Placeholder>,
    },
    /// A decoded image was dropped to bound memory: it loads again on request.
    MediaEvicted {
        /// The blob's CID.
        cid: String,
    },
    /// An image worker has a result: the runtime polls its images (the state has nothing to do).
    ImagesReady,
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

/// What the status line calls a write that creates an open forum; its message carries the
/// L7 label (`threat-model.md` L7).
pub const NEW_OPEN_FORUM: &str = "New open forum";

/// What the status line calls a forum edit that sets open membership (with the L7 label).
pub const OPEN_FORUM_EDIT: &str = "Forum edit (open membership)";

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

/// Which half of the Forum screen shows (`synthesis-v1.md` §6.11: "Forum (sort and filter
/// tabs; Posts and Activity)").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForumTab {
    /// The post list (`list_posts`).
    #[default]
    Posts,
    /// Recent Discussion (`recent_discussion`, `social.md` §12.5).
    Activity,
}

/// The Activity tab (`recent_discussion`): the forum's posts with their newest comments.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityData {
    /// The list.
    pub list: DiscussionList,
    /// The bodies of the comments it shows, by comment uri.
    pub bodies: BTreeMap<String, Body>,
}

/// The Forum screen: the post list, and the Activity tab.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumScreen {
    /// The forum uri.
    pub uri: String,
    /// The sort (`s`: magic, top, new, old, recent comments; `social.md` §12.2).
    pub sort: PostSort,
    /// The filter (`f`: frontpage, all, curated; `social.md` §12.3).
    pub filter: PostFilter,
    /// The data, once loaded.
    pub data: Option<ForumData>,
    /// A live update waiting for `R`.
    pub pending: Option<ForumData>,
    /// Which tab shows (`Tab` switches).
    pub tab: ForumTab,
    /// The Activity tab (`recent_discussion`), once loaded.
    pub activity: Option<ActivityData>,
    /// A live update of it waiting for `R`.
    pub activity_pending: Option<ActivityData>,
    /// The Posts cursor.
    pub pos: ListPos,
    /// The Activity cursor (each tab keeps its own place).
    pub activity_pos: ListPos,
}

impl ForumScreen {
    fn uris(d: &ForumData) -> Vec<String> {
        d.list.items.iter().map(|p| p.uri.clone()).collect()
    }

    fn activity_uris(d: &ActivityData) -> Vec<String> {
        d.list.items.iter().map(|i| i.post.uri.clone()).collect()
    }

    /// The cursor of what shows.
    pub fn cursor(&self) -> &ListPos {
        match self.tab {
            ForumTab::Posts => &self.pos,
            ForumTab::Activity => &self.activity_pos,
        }
    }

    /// The cursor of what shows, to move.
    pub fn cursor_mut(&mut self) -> &mut ListPos {
        match self.tab {
            ForumTab::Posts => &mut self.pos,
            ForumTab::Activity => &mut self.activity_pos,
        }
    }

    /// The posts the current tab lists.
    pub fn selected(&self) -> Option<&lessdugin_core::dto::PostSummary> {
        let i = self.cursor().cursor;
        match self.tab {
            ForumTab::Posts => self.data.as_ref()?.list.items.get(i),
            ForumTab::Activity => self.activity.as_ref()?.list.items.get(i).map(|x| &x.post),
        }
    }

    /// The rows the current tab has.
    pub fn row_count(&self) -> usize {
        match self.tab {
            ForumTab::Posts => self.data.as_ref().map_or(0, |d| d.list.items.len()),
            ForumTab::Activity => self.activity.as_ref().map_or(0, |d| d.list.items.len()),
        }
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
    /// The comment sort (`s`: magic, top, new, old, latest reply; `social.md` §12.4).
    pub opts: ThreadOpts,
    /// The data, once loaded.
    pub data: Option<ThreadData>,
    /// A live update waiting for `R`.
    pub pending: Option<ThreadData>,
    /// The version history of the opened item, once loaded (`h` toggles the panel; M4).
    pub history: Option<Result<HistoryView, String>>,
    /// Comments whose fold the reader switched (`Space`): a collapsed comment expands, any
    /// other one folds its replies.
    pub collapsed: BTreeSet<String>,
    /// The viewer's DID: their own `Collapsed(NewUser)` comments show expanded.
    pub viewer: Option<String>,
    /// The images asked for, by blob CID.
    pub media: BTreeMap<String, MediaState>,
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
        self.layout = thread::layout_for(
            data,
            &self.collapsed,
            width,
            self.viewer.as_deref(),
            &self.media,
        );
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
    /// How the entry's `#restrict` assertion is completed (the engine keeps the dimension it
    /// leaves out).
    pub merge: Option<RestrictDim>,
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
    /// As [`ModEntry::merge`].
    pub merge: Option<RestrictDim>,
}

/// The "this is public" interstitial (`social.md` §17, `threat-model.md` PRIV-9): the first
/// public action of a kind the account has not acknowledged waits behind it until the user
/// goes on (`y`) or cancels (`n`).
#[derive(Clone, Debug, PartialEq)]
pub struct PublicNotice {
    /// The kinds it covers, in [`PublicActionKind`] order (one, unless one key queued two).
    pub kinds: Vec<PublicActionKind>,
    /// The writes it holds.
    pub held: Vec<Effect>,
}

/// A text panel over the screen (`:members`, `:raw`, `:settings`): the navigation keys
/// scroll it, any other key closes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// Its title.
    pub title: String,
    /// Its lines.
    pub lines: Vec<TermText>,
    /// The first line shown.
    pub scroll: usize,
}

/// A yes/no question before a write that cannot be taken back (`:delete`, `:redact`), or an
/// edit that suspends a moderator's assertions (`:edit`).
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    /// The question.
    pub text: String,
    /// What `y` runs.
    pub effects: Vec<Effect>,
}

/// What the mod menu and `:mod` act on: the selected item, its thread's post, and their
/// authors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModSubject {
    /// The selected post or comment.
    pub item: String,
    /// Its forum.
    pub forum: String,
    /// The item's author.
    pub author: String,
    /// The thread's post (the item itself when a post is selected).
    pub post: String,
    /// The post's author: the one author moderation belongs to (`social.md` §4.9).
    pub post_author: String,
    /// The item is a comment.
    pub comment: bool,
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
                filter: f.filter,
                origin,
            },
            ScreenKind::Thread(t) => Effect::LoadThread {
                screen,
                uri: t.uri.clone(),
                opts: t.opts.clone(),
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

    /// The loads of this screen (the Mod screen loads its queue and its log together; the Forum
    /// screen loads its Activity tab when that is what shows).
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
            ScreenKind::Forum(f) if f.tab == ForumTab::Activity || f.activity.is_some() => {
                let mut fx = vec![self.load(origin)];
                fx.push(Effect::LoadActivity {
                    screen: self.id,
                    forum: f.uri.clone(),
                    origin,
                });
                fx
            }
            _ => vec![self.load(origin)],
        }
    }

    /// True if a `Changed` topic concerns this screen.
    fn concerns(&self, topic: &Topic) -> bool {
        match (&self.kind, topic) {
            (ScreenKind::Home(_), Topic::Forum { .. } | Topic::Accounts) => true,
            // notifications are computed lazily: a forum or thread change is what brings new
            // ones (`engine-api.md` §3.4), and nothing emits `Notifications` for them
            (
                ScreenKind::Notifications(_),
                Topic::Notifications | Topic::Forum { .. } | Topic::Thread { .. },
            ) => true,
            (ScreenKind::Forum(f), Topic::Forum { uri }) => *uri == f.uri,
            // new applicants and assertions arrive with the forum's changes
            (ScreenKind::Mod(m), Topic::Forum { uri }) => *uri == m.forum,
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
                if let Some(p) = &s.pending {
                    Some(new_of(
                        s.data.as_ref().map(ForumScreen::uris).unwrap_or_default(),
                        ForumScreen::uris(p),
                    ))
                } else {
                    let p = s.activity_pending.as_ref()?;
                    Some(new_of(
                        s.activity
                            .as_ref()
                            .map(ForumScreen::activity_uris)
                            .unwrap_or_default(),
                        ForumScreen::activity_uris(p),
                    ))
                }
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
                    s.data
                        .as_ref()
                        .map(NotificationsScreen::uris)
                        .unwrap_or_default(),
                    NotificationsScreen::uris(p),
                ))
            }
        }
    }
}

/// The opinion key an intent corresponds to (the gate speaks in keys' terms).
fn op_of(intent: &OpinionIntent) -> Op {
    match intent {
        OpinionIntent::Karma { value } => match value {
            Some(KarmaValue::Up) => Op::Up,
            Some(KarmaValue::StrongUp) => Op::StrongUp,
            Some(KarmaValue::Down) => Op::Down,
            Some(KarmaValue::StrongDown) => Op::StrongDown,
            None => Op::Clear,
        },
        OpinionIntent::Agreement { value } => match value {
            Some(AgreementValue::Agree) => Op::Agree,
            Some(AgreementValue::StrongAgree) => Op::StrongAgree,
            Some(AgreementValue::Disagree) => Op::Disagree,
            Some(AgreementValue::StrongDisagree) => Op::StrongDisagree,
            None => Op::Clear,
        },
        // reacts are gated by open_react, not by the opinion gate
        OpinionIntent::React { .. } | OpinionIntent::Tag { .. } => Op::Clear,
    }
}

/// Completes the last word of a command line against the verbs and their common arguments
/// (`Tab` in the overlay).
fn complete(line: &mut String) {
    const VERBS: &[&str] = &[
        "vote",
        "agree",
        "react",
        "unreact",
        "mod",
        "mods",
        "notifications",
        "read",
        "read-all",
        "activity",
        "sort",
        "filter",
        "user",
        "open",
        "follow",
        "unfollow",
        "join",
        "leave",
        "members",
        "grant",
        "revoke",
        "approve",
        "reject",
        "forum",
        "edit",
        "delete",
        "retract",
        "redact",
        "raw",
        "bookmark",
        "unbookmark",
        "subscribe",
        "unsubscribe",
        "mute",
        "unmute",
        "karma-notifications",
        "settings",
        "set",
        "unset",
        "undo",
        "retry",
        "help",
    ];
    const ARGS: &[&str] = &[
        "up",
        "strong-up",
        "down",
        "strong-down",
        "for",
        "against",
        "strong-for",
        "strong-against",
        "clear",
        "hide",
        "spam",
        "unhide",
        "keep",
        "ban",
        "unban",
        "pin",
        "unpin",
        "lock",
        "unlock",
        "tier",
        "restrict",
        "unrestrict",
        "nullify",
        "unnullify",
        "approve",
        "personal",
        "frontpage",
        "curated",
        "all",
        "unread",
        "magic",
        "top",
        "new",
        "old",
        "recent-comments",
        "latest-reply",
    ];
    // the byte after the last whitespace character, which may be wider than one byte (an NBSP,
    // an IME's full-width space): `i + 1` would slice inside it
    let start = line
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &line[start..];
    if word.is_empty() && start > 0 {
        return;
    }
    let set: &[&str] = if start == 0 { VERBS } else { ARGS };
    let mut hits = set.iter().filter(|w| w.starts_with(word));
    let Some(first) = hits.next() else {
        return;
    };
    // only complete when the prefix is ambiguous-but-there or unique: replace the word
    line.truncate(start);
    line.push_str(first);
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
    /// The `:` command line overlay (`synthesis-v1.md` §6.11 Overlays): the text so far.
    pub command: Option<String>,
    /// The public-action interstitial, while one is open.
    pub public_notice: Option<PublicNotice>,
    /// A confirmation, while one is open.
    pub confirm: Option<Confirm>,
    /// A text panel, while one is open.
    pub panel: Option<Panel>,
    /// The public actions the account has acknowledged (`None` until loaded: every kind asks).
    pub acknowledged: Option<BTreeSet<PublicActionKind>>,
    /// The account [`State::acknowledged`] was read for.
    acknowledged_for: Option<String>,
    /// [`Effect::LoadAcknowledged`] was sent.
    acknowledged_asked: bool,
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
            command: None,
            public_notice: None,
            confirm: None,
            panel: None,
            acknowledged: None,
            acknowledged_for: account.as_ref().map(|a| a.did.clone()),
            acknowledged_asked: false,
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

    /// The forum on top, if a forum is on top.
    pub fn top_forum_screen(&self) -> Option<&ForumScreen> {
        match &self.top()?.kind {
            ScreenKind::Forum(f) => Some(f),
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
            Action::ModsSetDone { screen, result } => {
                self.mods_set_done(screen, result, &mut fx);
            }
            Action::ModActionDone {
                forum: _,
                screen,
                result,
            } => self.mod_action_done(screen, result, &mut fx),
            Action::QueueLoaded { screen, result, .. } => {
                self.mod_loaded(screen, Some(result), None, &mut fx)
            }
            Action::LogLoaded { screen, result, .. } => {
                self.mod_loaded(screen, None, Some(result), &mut fx)
            }
            Action::ActivityLoaded {
                screen,
                origin,
                result,
            } => self.activity_loaded(screen, origin, result, &mut fx),
            Action::NotificationsLoaded {
                screen,
                origin,
                result,
            } => self.notifications_loaded(screen, origin, result, &mut fx),
            Action::HistoryLoaded {
                screen,
                origin,
                result,
            } => {
                let _ = origin;
                if let Some(i) = self.index_of(screen)
                    && let ScreenKind::Thread(t) = &mut self.stack[i].kind
                {
                    t.history = Some(result);
                }
            }
            Action::MarkedRead { screen, result } => self.marked_read(screen, result, &mut fx),
            Action::UnreadLoaded(result) => {
                if let Ok(u) = result {
                    self.unread = Some(u);
                }
            }
            Action::AcknowledgedLoaded { account, result } => match result {
                // another account is active now (another client switched): its own
                // acknowledgements replace the previous account's
                Ok(kinds) if account != self.acknowledged_for => {
                    self.acknowledged = Some(kinds.into_iter().collect());
                    self.acknowledged_for = account;
                }
                // acknowledgements are never withdrawn, and one made here may not be stored
                // yet when this list was read: keep both
                Ok(kinds) => self.acknowledged.get_or_insert_default().extend(kinds),
                Err(e) => tracing::debug!(error = %e, "acknowledged_public_actions failed"),
            },
            Action::Opened(result) => match result {
                Ok(Ref::Forum { uri }) => self.push_forum(uri, &mut fx),
                Ok(Ref::Post { uri } | Ref::Comment { uri }) => self.push_thread(uri, &mut fx),
                Ok(_) => self.say(Tone::Info, lit("That is not a forum, a post or a comment.")),
                Err(e) => self.say(Tone::Error, cat([lit("Cannot open it: "), tt(&e.message)])),
            },
            Action::Written {
                what,
                screen,
                result,
            } => self.written(what, screen, result, &mut fx),
            Action::LocalDone(result) => match result {
                Ok(msg) => self.say(Tone::Good, tt(&msg)),
                Err(e) => self.say(Tone::Error, tt(&e.message)),
            },
            Action::EditReady(result) => match result {
                Ok((req, notices)) if notices.is_empty() => fx.push(Effect::Editor(req)),
                // an edit suspends the keep, tier or pin in effect on the item: ask first
                // (`social.md` §4.5, `threat-model.md` SOC-3)
                Ok((req, notices)) => {
                    let mut text = String::new();
                    for n in &notices {
                        let mut c = n.message.chars();
                        text.extend(c.next().map(|f| f.to_ascii_uppercase()));
                        text.push_str(c.as_str());
                        text.push_str(". ");
                    }
                    text.push_str("Edit it anyway?");
                    self.confirm = Some(Confirm {
                        text,
                        effects: vec![Effect::Editor(req)],
                    });
                }
                Err(e) => self.say(Tone::Error, cat([lit("Cannot edit it: "), tt(&e.message)])),
            },
            Action::MembersLoaded(result) => match result {
                Ok(m) => self.show_panel("Members", members_lines(&m)),
                Err(e) => self.say(Tone::Error, tt(&e.message)),
            },
            Action::RawLoaded(result) => match result {
                Ok(r) => self.show_panel("Raw", raw_lines(&r)),
                Err(e) => self.say(Tone::Error, tt(&e.message)),
            },
            Action::SettingsLoaded(result) => match result {
                Ok(set) => self.show_panel("Settings", settings_lines(&set)),
                Err(e) => self.say(Tone::Error, tt(&e.message)),
            },
            Action::MediaFetched { cid, result } => {
                let next = match result {
                    // decoded only for a thread screen that still shows it
                    Ok(bytes) if self.wants_media(&cid) => {
                        fx.push(Effect::DecodeMedia {
                            cid: cid.clone(),
                            bytes,
                        });
                        Some(MediaState::Decoding)
                    }
                    Ok(_) => None,
                    Err(e) => Some(MediaState::Failed(format!("could not load: {}", e.message))),
                };
                self.set_media(&cid, next);
            }
            Action::MediaDecoded { cid, result } => {
                let next = match result {
                    Ok(()) => MediaState::Shown,
                    Err(p) => MediaState::Failed(p.text().to_owned()),
                };
                self.set_media(&cid, Some(next));
            }
            Action::MediaEvicted { cid } => self.set_media(&cid, None),
            Action::ImagesReady => {}
        }
        self.hold_public(&mut fx);
        self.fit();
        fx
    }

    /// Takes the public writes of a kind the account has not acknowledged out of `fx` and
    /// holds them behind the interstitial (`engine-api.md` §3.6: every client shows it before
    /// the first action of a kind; the engine does not enforce it). Every write the TUI makes
    /// passes here, whatever key or command queued it.
    fn hold_public(&mut self, fx: &mut Vec<Effect>) {
        let acked = self.acknowledged.clone().unwrap_or_default();
        let (held, rest): (Vec<Effect>, Vec<Effect>) = std::mem::take(fx)
            .into_iter()
            .partition(|e| e.public_kind().is_some_and(|k| !acked.contains(&k)));
        *fx = rest;
        if held.is_empty() {
            return;
        }
        let notice = self.public_notice.get_or_insert(PublicNotice {
            kinds: Vec::new(),
            held: Vec::new(),
        });
        for e in held {
            if let Some(k) = e.public_kind()
                && !notice.kinds.contains(&k)
            {
                notice.kinds.push(k);
            }
            notice.held.push(e);
        }
        notice.kinds.sort();
        self.say(Tone::Warn, lit("This is public: y to go on, n to cancel."));
    }

    /// A key while a confirmation is open: `y` runs its writes, `n` or Esc drops them.
    fn confirm_key(&mut self, k: &KeyEvent, fx: &mut Vec<Effect>) {
        use crossterm::event::{KeyCode, KeyModifiers};
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
            self.confirm = None;
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        match k.code {
            KeyCode::Char('y' | 'Y') => {
                if let Some(c) = self.confirm.take() {
                    self.status = None;
                    fx.extend(c.effects);
                }
            }
            KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => {
                self.confirm = None;
                self.say(Tone::Info, lit("Cancelled."));
            }
            _ => {}
        }
    }

    /// True if a thread screen on the stack asked for the image `cid`.
    fn wants_media(&self, cid: &str) -> bool {
        self.stack
            .iter()
            .any(|s| matches!(&s.kind, ScreenKind::Thread(t) if t.media.contains_key(cid)))
    }

    /// Sets where the image `cid` is on every thread screen that asked for it (`None`: not
    /// asked for any more), and lays the screens out again.
    fn set_media(&mut self, cid: &str, state: Option<MediaState>) {
        let (w, h) = (self.width, self.content_height());
        for s in &mut self.stack {
            if let ScreenKind::Thread(t) = &mut s.kind
                && t.media.contains_key(cid)
            {
                match &state {
                    Some(st) => {
                        t.media.insert(cid.to_owned(), st.clone());
                    }
                    None => {
                        t.media.remove(cid);
                    }
                }
                t.relayout(w, h);
            }
        }
    }

    /// `i`: loads the selected item's images. The key is the reader's consent (`sync.md`
    /// §11.3: what the autoload policy does not load is "click to load"): their author's PDS
    /// sees this device's address. A comment folded by the fold shows neither its images nor
    /// the host they load from, so it is expanded first.
    fn load_media(&mut self, fx: &mut Vec<Effect>) {
        let Some(t) = self.top_thread() else {
            self.say(Tone::Info, lit("Open a post to load its images."));
            return;
        };
        let Some(span) = t.layout.items.get(t.pos.selected) else {
            return;
        };
        let uri = span.uri.clone();
        if span.collapsed && !t.collapsed.contains(&uri) {
            self.say(
                Tone::Info,
                lit(
                    "This comment is collapsed: press Space to show it, and where its images \
                     load from, first.",
                ),
            );
            return;
        }
        let media: Vec<lessdugin_core::dto::MediaView> = t
            .data
            .as_ref()
            .map(|d| item_media(d, &uri, false))
            .unwrap_or_default();
        if media.is_empty() {
            self.say(Tone::Info, lit("The selected item has no image to load."));
            return;
        }
        let n = self.request_media(media, fx);
        if n == 0 {
            self.say(Tone::Info, lit("Its images are already loaded or loading."));
        } else {
            self.say(
                Tone::Info,
                cat([lit("Loading "), count(n, "image", "images"), lit("...")]),
            );
        }
    }

    /// Asks for each image of `media` the top thread does not have yet (one over the input cap
    /// is marked so, never fetched); returns how many were asked for.
    fn request_media(
        &mut self,
        media: Vec<lessdugin_core::dto::MediaView>,
        fx: &mut Vec<Effect>,
    ) -> u32 {
        let (w, h) = (self.width, self.content_height());
        let Some(ScreenKind::Thread(t)) = self.stack.last_mut().map(|s| &mut s.kind) else {
            return 0;
        };
        let mut n = 0;
        for m in media {
            if matches!(
                t.media.get(&m.cid),
                Some(MediaState::Fetching | MediaState::Decoding | MediaState::Shown)
            ) {
                continue;
            }
            if m.size_bytes > crate::image::limits::MAX_INPUT_BYTES as u64 {
                t.media.insert(
                    m.cid.clone(),
                    MediaState::Failed(Placeholder::TooLarge.text().to_owned()),
                );
                continue;
            }
            t.media.insert(m.cid.clone(), MediaState::Fetching);
            fx.push(Effect::FetchMedia { media: Box::new(m) });
            n += 1;
        }
        t.relayout(w, h);
        n
    }

    /// The images of the thread on top that load without a key (`sync.md` §11.3,
    /// `threat-model.md` PRIV-1): the engine's verdict (`MediaView.autoload`), only for a
    /// `Visible` item, an inline image type and at most `BLOB_AUTOLOAD_MAX_BYTES`.
    fn autoload_media(&mut self, fx: &mut Vec<Effect>) {
        let Some(t) = self.top_thread() else {
            return;
        };
        let Some(d) = &t.data else {
            return;
        };
        let wanted: Vec<lessdugin_core::dto::MediaView> = thread::item_uris(d)
            .iter()
            .flat_map(|uri| item_media(d, uri, true))
            .filter(|m| {
                m.autoload
                    && m.size_bytes <= lessdugin_core::media::policy::AUTOLOAD_MAX_BYTES
                    && !t.media.contains_key(&m.cid)
            })
            .collect();
        if !wanted.is_empty() {
            self.request_media(wanted, fx);
        }
    }

    /// Opens a text panel.
    fn show_panel(&mut self, title: &str, lines: Vec<TermText>) {
        self.panel = Some(Panel {
            title: title.to_owned(),
            lines,
            scroll: 0,
        });
    }

    /// [`Action::Written`]: the tickets are followed, the screen reloads.
    fn written(
        &mut self,
        what: &'static str,
        screen: Option<ScreenId>,
        result: Result<Vec<WriteTicket>, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(tickets) => {
                for t in tickets {
                    if !matches!(
                        t.state,
                        WriteState::Done { .. } | WriteState::Failed { .. } | WriteState::Undone
                    ) {
                        self.writes.insert(t.id.clone(), t.state);
                    }
                }
                let mut msg = if self.offline {
                    format!("{what}: queued; it is sent when you are online.")
                } else {
                    format!("{what}: queued; sending...")
                };
                // creating an open forum, or opening one, says what open mode relies on
                // (`threat-model.md` L7, as the CLI's `forum create --mode open`)
                if matches!(what, NEW_OPEN_FORUM | OPEN_FORUM_EDIT) {
                    msg = format!("{msg} {}.", crate::view::OPEN_MODE_LABEL);
                }
                self.say(Tone::Good, tt(&msg));
                if let Some(i) = screen.and_then(|id| self.index_of(id)) {
                    self.follow_own = screen;
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => self.say(Tone::Error, tt(&format!("{what} refused: {}", e.message))),
        }
    }

    /// A key while the interstitial is open: `y` sends the held writes and records the
    /// acknowledgement, `n` or Esc drops them. Enter does nothing here: it is the key that
    /// queued most of the held writes, and a second press (or a key repeat) must not publish
    /// past a notice the user has not read (`threat-model.md` PRIV-9).
    fn public_notice_key(&mut self, k: &KeyEvent, fx: &mut Vec<Effect>) {
        use crossterm::event::{KeyCode, KeyModifiers};
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
            self.public_notice = None;
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        match k.code {
            KeyCode::Char('y' | 'Y') => {
                let Some(n) = self.public_notice.take() else {
                    return;
                };
                let acked = self.acknowledged.get_or_insert_default();
                for kind in n.kinds {
                    acked.insert(kind);
                    fx.push(Effect::Acknowledge { kind });
                }
                self.status = None;
                fx.extend(n.held);
            }
            KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => {
                self.public_notice = None;
                self.say(Tone::Info, lit("Cancelled: nothing was published."));
            }
            _ => {}
        }
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
                let len = s.row_count();
                let rows = h / LIST_ROWS;
                s.cursor_mut().reveal(len, rows);
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
        // the acknowledgements the interstitial needs, once there is an account to ask about
        if self.account.is_some() && !self.acknowledged_asked {
            self.acknowledged_asked = true;
            fx.push(Effect::LoadAcknowledged);
        }
        i
    }

    fn push_forum(&mut self, uri: String, fx: &mut Vec<Effect>) {
        let i = self.push(
            ScreenKind::Forum(Box::new(ForumScreen {
                uri: uri.clone(),
                sort: PostSort::Magic,
                filter: PostFilter::Frontpage,
                data: None,
                pending: None,
                tab: ForumTab::Posts,
                activity: None,
                activity_pending: None,
                pos: ListPos::default(),
                activity_pos: ListPos::default(),
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
                viewer: self.account.as_ref().map(|a| a.did.clone()),
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
                    // presses R; the screen keeps what it shows, marked stale so the user's
                    // next move reloads it (a genuine update is late, never lost)
                    s.stale = true;
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
                    let pos = &mut s.pos;
                    receive_list(
                        &mut s.data,
                        &mut s.pending,
                        pos,
                        p,
                        Origin::User,
                        ForumScreen::uris,
                    );
                }
                if let Some(p) = s.activity_pending.take() {
                    let pos = &mut s.activity_pos;
                    receive_list(
                        &mut s.activity,
                        &mut s.activity_pending,
                        pos,
                        p,
                        Origin::User,
                        ForumScreen::activity_uris,
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
        // the public-action interstitial is answered before anything else, then a confirmation
        if self.public_notice.is_some() {
            self.public_notice_key(k, fx);
            return;
        }
        if self.confirm.is_some() {
            self.confirm_key(k, fx);
            return;
        }
        // a prompt captures the raw keys (it edits a text line)
        if self.prompt.is_some() {
            self.prompt_key(k, fx);
            return;
        }
        // so does the `:` command line
        if self.command.is_some() {
            self.command_key(k, fx);
            return;
        }
        let cmd = keys::command(k);
        if cmd == Some(Cmd::Quit) {
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        // a panel scrolls like the help overlay, and any other key closes it
        if let Some(p) = &mut self.panel {
            match cmd {
                Some(Cmd::Down) => p.scroll = p.scroll.saturating_add(1),
                Some(Cmd::Up) => p.scroll = p.scroll.saturating_sub(1),
                Some(Cmd::HalfDown) => p.scroll = p.scroll.saturating_add(5),
                Some(Cmd::HalfUp) => p.scroll = p.scroll.saturating_sub(5),
                Some(Cmd::Top) => p.scroll = 0,
                Some(Cmd::Bottom) => p.scroll = usize::MAX,
                _ => self.panel = None,
            }
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
            Cmd::Filter => self.filter(fx),
            Cmd::Reply => self.compose_comment(true, fx),
            Cmd::CommentOnPost => self.compose_comment(false, fx),
            Cmd::NewPost => self.compose_post(fx),
            Cmd::Opinion(op) => self.opinion(op, fx),
            Cmd::Undo => self.undo_last(fx),
            Cmd::ReactPalette => self.open_react(),
            Cmd::History => self.open_history(fx),
            Cmd::LoadMedia => self.load_media(fx),
            Cmd::ModMenu => self.open_mod_menu(),
            Cmd::Notifications => self.open_notifications(fx),
            Cmd::Command => self.command = Some(String::new()),
            Cmd::MarkAllRead => self.mark_all_read(fx),
            Cmd::Stance => {
                // on the Mod screen, `Tab` switches the queue/log tab; on a forum, Posts and
                // Activity (§6.11); elsewhere it is the react palette's stance key, which only
                // makes sense inside the palette
                match self.stack.last().map(|s| &s.kind) {
                    Some(ScreenKind::Mod(m)) => {
                        let tab = match m.tab {
                            ModTab::Queue => ModTab::Log,
                            ModTab::Log => ModTab::Queue,
                        };
                        self.switch_mod_tab(tab);
                    }
                    Some(ScreenKind::Forum(_)) => self.switch_forum_tab(fx),
                    _ => self.say(
                        Tone::Info,
                        lit("Open the react palette (x) to set a stance."),
                    ),
                }
            }
            nav => self.navigate(nav, fx),
        }
    }

    fn navigate(&mut self, cmd: Cmd, fx: &mut Vec<Effect>) {
        let (w, h) = (self.width, self.content_height());
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        let top = &mut self.stack[i];
        top.settling = false;
        top.live_burst = 0;
        if top.stale {
            // live updates were capped while the user did nothing: the move picks them up
            top.stale = false;
            self.reload(i, Origin::Live, fx);
        }
        let top = &mut self.stack[i];
        match &mut top.kind {
            ScreenKind::Home(s) => {
                let len = s.data.as_ref().map_or(0, |d| d.forums.len());
                s.pos.apply(cmd, len, h / LIST_ROWS);
            }
            ScreenKind::Forum(s) => {
                let len = s.row_count();
                let rows = h / LIST_ROWS;
                s.cursor_mut().apply(cmd, len, rows);
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
                        if !it.comment || (it.replies == 0 && !it.collapsed) {
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
            ScreenKind::Thread(_) => self.navigate(Cmd::Collapse, fx),
            // on the Mod screen, Enter approves the selected queue row (an applicant)
            ScreenKind::Mod(_) => self.approve_selected(fx),
            // on the Notifications screen, Enter opens the notification's thread
            ScreenKind::Notifications(s) => {
                if let Some(n) = s.data.as_ref().and_then(|d| d.items.get(s.pos.cursor)) {
                    let uri = n.subject.clone();
                    self.push_thread(uri, fx);
                }
            }
        }
    }

    /// `s`: the next post sort on a forum (`social.md` §12.2), the next comment sort on a
    /// thread (§12.4).
    fn sort(&mut self, fx: &mut Vec<Effect>) {
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        match &mut self.stack[i].kind {
            ScreenKind::Forum(s) => {
                let next = match s.sort {
                    PostSort::Magic => PostSort::Top,
                    PostSort::Top => PostSort::New,
                    PostSort::New => PostSort::Old,
                    PostSort::Old => PostSort::RecentComments,
                    PostSort::RecentComments => PostSort::Magic,
                };
                self.set_post_sort(next, fx);
            }
            ScreenKind::Thread(t) => {
                let next = match t.opts.sort {
                    CommentSort::Magic => CommentSort::Top,
                    CommentSort::Top => CommentSort::New,
                    CommentSort::New => CommentSort::Old,
                    CommentSort::Old => CommentSort::LatestReply,
                    CommentSort::LatestReply => CommentSort::Magic,
                };
                self.set_comment_sort(next, fx);
            }
            _ => self.say(Tone::Info, lit("Sorting is for forums and threads.")),
        }
    }

    /// Lists the forum on top in `sort`.
    fn set_post_sort(&mut self, sort: PostSort, fx: &mut Vec<Effect>) {
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        let ScreenKind::Forum(s) = &mut self.stack[i].kind else {
            self.say(Tone::Info, lit("Open a forum to sort its posts."));
            return;
        };
        s.sort = sort;
        s.tab = ForumTab::Posts;
        s.pos = ListPos::default();
        s.pending = None;
        self.reload(i, Origin::User, fx);
    }

    /// Shows the thread on top in `sort` (the selection stays on its item).
    fn set_comment_sort(&mut self, sort: CommentSort, fx: &mut Vec<Effect>) {
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        let ScreenKind::Thread(t) = &mut self.stack[i].kind else {
            self.say(Tone::Info, lit("Open a thread to sort its comments."));
            return;
        };
        t.opts.sort = sort;
        t.pending = None;
        self.reload(i, Origin::User, fx);
    }

    /// `f`: the next filter of the forum on top (frontpage, all, curated; `social.md` §12.3).
    fn filter(&mut self, fx: &mut Vec<Effect>) {
        let next = match self.top_forum_screen().map(|f| f.filter) {
            Some(PostFilter::Frontpage) => PostFilter::All,
            Some(PostFilter::All) => PostFilter::Curated,
            Some(PostFilter::Curated) => PostFilter::Frontpage,
            None => {
                self.say(Tone::Info, lit("Open a forum to filter its posts."));
                return;
            }
        };
        self.set_filter(next, fx);
    }

    /// Lists the forum on top with `filter`.
    fn set_filter(&mut self, filter: PostFilter, fx: &mut Vec<Effect>) {
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        let ScreenKind::Forum(s) = &mut self.stack[i].kind else {
            self.say(Tone::Info, lit("Open a forum to filter its posts."));
            return;
        };
        s.filter = filter;
        s.tab = ForumTab::Posts;
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
                        lit("You follow no forum yet: open one with `:open <owner>/<slug>`."),
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
                self.say(
                    Tone::Info,
                    lit("Notifications are local; sync a forum with R on it."),
                );
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
            Compose::Edit { uri, base } => {
                let mut draft = (**base).clone();
                match &mut draft {
                    // a quick take has no title line (data-model.md PO4): the text is its body
                    Draft::Post { post, .. } if post.kind == PostKind::QuickTake => {
                        post.body = lessdugin_core::dto::BodyDraft::text(text.clone());
                    }
                    Draft::Post { post, .. } => match editor::split_post(&text) {
                        Ok((title, body)) => {
                            // an unchanged first line keeps the title as it was (`split_post`
                            // drops a leading `#`, which an article's title may start with)
                            let first = text.trim_start().lines().next().unwrap_or("").trim();
                            if post.title.as_deref().map(str::trim) != Some(first) {
                                post.title = Some(title);
                            }
                            post.body = lessdugin_core::dto::BodyDraft::text(body);
                        }
                        Err(why) => {
                            self.say(
                                Tone::Error,
                                cat([
                                    lit("Not edited: "),
                                    tt(why),
                                    lit(". Run :edit to edit it again."),
                                ]),
                            );
                            return;
                        }
                    },
                    Draft::Comment { comment, .. } => {
                        comment.body = lessdugin_core::dto::BodyDraft::text(text.clone());
                    }
                }
                self.say(Tone::Info, lit("Queuing the edit..."));
                fx.push(Effect::Edit {
                    uri: uri.clone(),
                    draft,
                    text,
                });
            }
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

    /// The item an opinion command acts on, with its forum: the subject the command line named
    /// (a uri or any reference `parse_ref` takes; its forum when a screen on the stack shows
    /// it, else none, and the engine checks it), or the selection.
    fn command_subject(&self, named: Option<&str>) -> Option<(String, String)> {
        match named {
            Some(n) => Some((n.to_owned(), self.shown_forum(n).unwrap_or_default())),
            None => self.opinion_subject(),
        }
    }

    /// The forum of `uri` when a screen on the stack shows it (a post in a list or the Activity
    /// tab, an item of a thread).
    fn shown_forum(&self, uri: &str) -> Option<String> {
        self.stack.iter().rev().find_map(|s| match &s.kind {
            ScreenKind::Forum(f) => {
                let listed =
                    f.data
                        .as_ref()
                        .is_some_and(|d| d.list.items.iter().any(|p| p.uri == uri))
                        || f.activity.as_ref().is_some_and(|a| {
                            a.list.items.iter().any(|i| {
                                i.post.uri == uri || i.comments.iter().any(|c| c.uri == uri)
                            })
                        });
                listed.then(|| f.uri.clone())
            }
            ScreenKind::Thread(t) => {
                let d = t.data.as_ref()?;
                let ThreadRoot::Post(p) = &d.view.post else {
                    return None;
                };
                (p.post.uri == uri || d.view.comments.iter().any(|c| c.uri == uri))
                    .then(|| p.post.forum.clone())
            }
            _ => None,
        })
    }

    /// True if the thread on top holds `uri` (its react tallies are known).
    fn thread_holds(&self, uri: &str) -> bool {
        self.top_thread()
            .and_then(|t| t.data.as_ref())
            .is_some_and(|d| {
                thread::root_uri(d) == uri || d.view.comments.iter().any(|c| c.uri == uri)
            })
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
    /// stance toggles off; a stance switch is one add, which replaces the stance in place).
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
            // the opposite stance, or none: one add (AddReact replaces a held stance in place;
            // a separate remove would be a second task the runtime may run first, and a remove
            // that lands after the add deletes the react)
            Some(_) | None => fx.push(react(o.stance, false)),
        }
        self.react = None;
        self.say(Tone::Info, lit("Recording the react..."));
    }

    // -----------------------------------------------------------------------------------------
    // moderation (M3, synthesis-v1.md §6.11, social.md §4.4)

    /// The item and authors the mod menu acts on: the selected comment or post, its thread's
    /// post and their authors (on a list: the selected post, its own thread).
    fn mod_subject(&self) -> Option<ModSubject> {
        match &self.stack.last()?.kind {
            ScreenKind::Forum(s) => s.selected().map(|p| ModSubject {
                item: p.uri.clone(),
                forum: p.forum.clone(),
                author: p.author.did.clone(),
                post: p.uri.clone(),
                post_author: p.author.did.clone(),
                comment: false,
            }),
            ScreenKind::Thread(t) => {
                let d = t.data.as_ref()?;
                let ThreadRoot::Post(p) = &d.view.post else {
                    return None;
                };
                let uri = t.layout.items.get(t.pos.selected)?.uri.clone();
                let comment = d.view.comments.iter().find(|c| c.uri == uri);
                Some(ModSubject {
                    author: comment
                        .map_or_else(|| p.post.author.did.clone(), |c| c.author.did.clone()),
                    comment: comment.is_some(),
                    item: uri,
                    forum: p.post.forum.clone(),
                    post: p.post.uri.clone(),
                    post_author: p.post.author.did.clone(),
                })
            }
            _ => None,
        }
    }

    /// `m`: opens the mod menu for the selected item (and its author) when the viewer may
    /// moderate the forum (`ViewerCaps.moderate`, §4.4) or the thread (author moderation, §4.9),
    /// else says why not.
    fn open_mod_menu(&mut self) {
        let Some(subject) = self.mod_subject() else {
            self.say(Tone::Info, lit("Nothing selected to moderate."));
            return;
        };
        let caps = self.caps_of(&subject.forum).cloned();
        let Some(c) = caps.as_ref() else {
            // no forum header on the stack: the engine's own checks decide
            self.mod_menu = Some(self.mod_menu_for(&subject, None));
            return;
        };
        if !c.moderate && !c.author_mod_personal && !c.author_mod_frontpage {
            self.say(
                Tone::Warn,
                lit("You cannot moderate this forum (not an authority)."),
            );
            return;
        }
        self.mod_menu = Some(self.mod_menu_for(&subject, Some(c)));
    }

    /// The menu's entries for the selected item and authors, from the viewer's caps
    /// (`ViewerCaps.moderate`/`rank`/`curate`, `social.md` §4.4; the author tier of §4.9 in
    /// the viewer's own threads). The CLI's meaning of each verb: an override asserts the
    /// opposite value, a "retract" removes the viewer's own assertion. Nothing is offered whose
    /// subject would be the viewer.
    fn mod_menu_for(&self, m: &ModSubject, caps: Option<&ViewerCaps>) -> ModMenu {
        let moderate = caps.is_none_or(|c| c.moderate);
        let curate = caps.is_none_or(|c| c.curate);
        let author_mod = caps.is_none_or(|c| c.author_mod_personal || c.author_mod_frontpage);
        let me = self.account.as_ref().map(|a| a.did.as_str()).unwrap_or("");
        // author moderation belongs to the author of the thread's POST (§4.9), and acts on
        // other people's comments in it
        let own_thread = !me.is_empty() && me == m.post_author;
        let own_item = !me.is_empty() && me == m.author;
        let forum = m.forum.as_str();
        let assert_on =
            |scope: ModScope, subject: &str, action: ModActionValue| ModActionIntent::Assert {
                forum: forum.to_owned(),
                scope,
                subject: subject.to_owned(),
                action,
                reason: None,
                until_ms: None,
            };
        let remove_on = |scope: ModScope, subject: &str, key: &str| ModActionIntent::Remove {
            forum: forum.to_owned(),
            scope,
            subject: subject.to_owned(),
            action_key: key.to_owned(),
        };
        let mut e = Vec::new();
        let entry = |label: &'static str, intent| ModEntry {
            label,
            intent: Some(intent),
            queue: false,
            log: false,
            merge: None,
        };
        let pin_scope = if m.comment {
            PinScope::Thread
        } else {
            PinScope::Forum
        };
        let pin_key = if m.comment { "pin:thread" } else { "pin:forum" };
        if moderate {
            let f = ModScope::Forum;
            let hide = |spam| ModActionValue::Hide {
                hidden: true,
                spam,
                show_reason: true,
            };
            e.push(entry("hide this item", assert_on(f, &m.item, hide(false))));
            e.push(entry("hide as spam", assert_on(f, &m.item, hide(true))));
            e.push(entry(
                "keep visible (override a hide)",
                assert_on(
                    f,
                    &m.item,
                    ModActionValue::Hide {
                        hidden: false,
                        spam: false,
                        show_reason: true,
                    },
                ),
            ));
            e.push(entry("retract your hide", remove_on(f, &m.item, "hide")));
            if !own_item {
                e.push(entry(
                    "ban the author",
                    assert_on(
                        f,
                        &m.author,
                        ModActionValue::Access {
                            level: AccessLevel::Banned,
                        },
                    ),
                ));
                e.push(entry("unban the author", remove_on(f, &m.author, "access")));
            }
            e.push(entry(
                if m.comment {
                    "pin in the thread"
                } else {
                    "pin in the forum"
                },
                assert_on(
                    f,
                    &m.item,
                    ModActionValue::Pin {
                        scope: pin_scope,
                        pinned: true,
                    },
                ),
            ));
            e.push(entry(
                "unpin (override a pin)",
                assert_on(
                    f,
                    &m.item,
                    ModActionValue::Pin {
                        scope: pin_scope,
                        pinned: false,
                    },
                ),
            ));
            e.push(entry("retract your pin", remove_on(f, &m.item, pin_key)));
            // locks and tiers belong to the thread's post
            e.push(entry(
                "lock the thread",
                assert_on(f, &m.post, ModActionValue::Lock { locked: true }),
            ));
            e.push(entry(
                "unlock (override a lock)",
                assert_on(f, &m.post, ModActionValue::Lock { locked: false }),
            ));
            e.push(entry("retract your lock", remove_on(f, &m.post, "lock")));
            let tier = |t| ModActionValue::Tier {
                tier: t,
                notice: None,
            };
            e.push(entry(
                "tier: personal",
                assert_on(f, &m.post, tier(Tier::Personal)),
            ));
            e.push(entry(
                "tier: frontpage",
                assert_on(f, &m.post, tier(Tier::Frontpage)),
            ));
            if curate {
                e.push(entry(
                    "tier: curated",
                    assert_on(f, &m.post, tier(Tier::Curated)),
                ));
            }
            if !own_item {
                // each sets one dimension of the viewer's `#restrict` and the engine keeps the
                // other (m3r-2); the capability presets add to the capabilities it restricts
                // already
                let restrict = |label, capabilities, nullify_votes, merge| ModEntry {
                    merge,
                    ..entry(
                        label,
                        assert_on(
                            f,
                            &m.author,
                            ModActionValue::Restrict {
                                capabilities,
                                nullify_votes,
                            },
                        ),
                    )
                };
                e.push(restrict(
                    "restrict the author (posting)",
                    Some(vec![Capability::Post, Capability::Comment]),
                    None,
                    Some(RestrictDim::AddCapabilities),
                ));
                e.push(restrict(
                    "restrict the author (voting, reacting)",
                    Some(vec![Capability::Vote, Capability::React]),
                    None,
                    Some(RestrictDim::AddCapabilities),
                ));
                e.push(restrict(
                    "nullify the author's votes",
                    None,
                    Some(true),
                    None,
                ));
                e.push(restrict(
                    "restore the author's votes",
                    None,
                    Some(false),
                    None,
                ));
                e.push(entry(
                    "retract your restriction",
                    remove_on(f, &m.author, "restrict"),
                ));
            }
        }
        if own_thread && author_mod {
            // the author tier in the viewer's own thread (§4.9): other people's comments, and
            // the viewer's own post
            let a = ModScope::Author;
            if m.comment && !own_item {
                e.push(entry(
                    "hide this comment (your thread)",
                    assert_on(
                        a,
                        &m.item,
                        ModActionValue::Hide {
                            hidden: true,
                            spam: false,
                            show_reason: true,
                        },
                    ),
                ));
                e.push(entry(
                    "unhide this comment (your thread)",
                    remove_on(a, &m.item, "hide"),
                ));
                e.push(entry(
                    "pin this comment (your thread)",
                    assert_on(
                        a,
                        &m.item,
                        ModActionValue::Pin {
                            scope: PinScope::Thread,
                            pinned: true,
                        },
                    ),
                ));
                e.push(entry(
                    "unpin this comment (your thread)",
                    remove_on(a, &m.item, "pin:thread"),
                ));
                e.push(entry(
                    "ban the author from your threads",
                    assert_on(
                        a,
                        &m.author,
                        ModActionValue::Access {
                            level: AccessLevel::Banned,
                        },
                    ),
                ));
                e.push(entry(
                    "lift your ban of the author",
                    remove_on(a, &m.author, "access"),
                ));
            }
            e.push(entry(
                "lock your thread",
                assert_on(a, &m.post, ModActionValue::Lock { locked: true }),
            ));
            e.push(entry("unlock your thread", remove_on(a, &m.post, "lock")));
        }
        e.push(ModEntry {
            label: "mod queue",
            intent: None,
            queue: true,
            log: false,
            merge: None,
        });
        e.push(ModEntry {
            label: "mod log",
            intent: None,
            queue: false,
            log: true,
            merge: None,
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
        let merge = e.merge;
        self.mod_menu = None;
        // ask for the optional reason before the write
        self.prompt = Some(Prompt {
            label: e.label,
            text: String::new(),
            intent,
            forum: m.forum.clone(),
            screen: m.screen,
            merge,
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

    /// [`Action::ModsSetDone`]: reloads the forum after a moderator change.
    fn mods_set_done(
        &mut self,
        screen: ScreenId,
        result: Result<ActionTicket, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(t) => {
                let mut msg = lit("Moderator list updated.");
                if let Some(n) = t.notices.first() {
                    msg = cat([lit("Moderator list updated: "), tt(&n.message), lit(".")]);
                }
                self.say(Tone::Good, msg);
                self.follow_own = Some(screen);
                if let Some(i) = self.index_of(screen) {
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => self.say(
                Tone::Error,
                cat([lit("Moderator change refused: "), tt(&e.message)]),
            ),
        }
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

    /// `Tab` on a forum: switches Posts and Activity (`synthesis-v1.md` §6.11). The Activity
    /// tab is loaded the first time it shows.
    fn switch_forum_tab(&mut self, fx: &mut Vec<Effect>) {
        let Some(i) = self.stack.len().checked_sub(1) else {
            return;
        };
        let screen = self.stack[i].id;
        let ScreenKind::Forum(f) = &mut self.stack[i].kind else {
            return;
        };
        f.tab = match f.tab {
            ForumTab::Posts => ForumTab::Activity,
            ForumTab::Activity => ForumTab::Posts,
        };
        if f.tab == ForumTab::Activity && f.activity.is_none() {
            let forum = f.uri.clone();
            self.stack[i].loading += 1;
            fx.push(Effect::LoadActivity {
                screen,
                forum,
                origin: Origin::User,
            });
        }
        self.fit();
    }

    /// `N`: opens the Notifications screen on top of Home.
    /// `h`: shows the version history of the selected item (`engine-api.md` §3.6, M4), or
    /// hides it again. The panel is per-screen, so opening another thread starts closed.
    fn open_history(&mut self, fx: &mut Vec<Effect>) {
        let Some(screen) = self.stack.last() else {
            return;
        };
        let sid = screen.id;
        let ScreenKind::Thread(t) = &screen.kind else {
            self.say(
                Tone::Info,
                lit("History is shown on a thread: open the post first (Enter)."),
            );
            return;
        };
        // a second `h` closes the panel; a third re-asks the engine, because the item may have
        // been edited in another window since
        let already_open = matches!(&t.history, Some(Ok(_)));
        let uri = self
            .opinion_subject()
            .map_or_else(|| t.uri.clone(), |(u, _)| u);
        if already_open
            && let Some(ScreenKind::Thread(t)) = self.stack.last_mut().map(|s| &mut s.kind)
        {
            t.history = None;
            return;
        }
        if let Some(ScreenKind::Thread(t)) = self.stack.last_mut().map(|s| &mut s.kind) {
            t.history = Some(Err("loading…".to_owned()));
        }
        fx.push(Effect::LoadHistory {
            screen: sid,
            uri,
            origin: Origin::User,
        });
    }

    fn open_notifications(&mut self, fx: &mut Vec<Effect>) {
        if self.account.is_none() {
            self.say(
                Tone::Info,
                lit("Notifications need an account: run `lessdugin login`."),
            );
            return;
        }
        self.push(ScreenKind::Notifications(Box::default()), fx);
    }

    /// `M` and `:read-all`: marks every notification read, the listed ones and any past the
    /// list (`engine.mark_all_notifications_read`).
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
        // `unread` counts every unread notification, listed or not
        if s.data.as_ref().is_none_or(|d| d.unread == 0) {
            self.say(Tone::Info, lit("Nothing to mark read."));
            return;
        }
        let screen = self.stack.last().map_or(0, |x| x.id);
        fx.push(Effect::MarkAllRead { screen });
    }

    /// `:read`: marks the selected notification read.
    fn mark_selected_read(&mut self, fx: &mut Vec<Effect>) {
        let Some((id, screen)) = self.stack.last().and_then(|s| match &s.kind {
            ScreenKind::Notifications(n) => n
                .data
                .as_ref()
                .and_then(|d| d.items.get(n.pos.cursor))
                .map(|item| (item, s.id)),
            _ => None,
        }) else {
            self.say(
                Tone::Info,
                lit("Select a notification on the notifications screen (N)."),
            );
            return;
        };
        if id.read {
            self.say(Tone::Info, lit("That notification is already read."));
            return;
        }
        let ids = vec![id.id.clone()];
        fx.push(Effect::MarkRead { ids, screen });
    }

    /// [`Action::NotificationsLoaded`].
    fn notifications_loaded(
        &mut self,
        screen: ScreenId,
        origin: Origin,
        result: Result<NotificationList, EngineError>,
        fx: &mut Vec<Effect>,
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
        // a live reload asked for while this one ran (as `loaded`)
        if s.dirty && s.loading == 0 {
            s.dirty = false;
            self.reload(i, Origin::Live, fx);
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
                // Marking read is a user action, so the reload is `Origin::User`: with
                // `Origin::Live` an earlier burst of live reloads (`LIVE_RELOAD_CAP`) would
                // swallow it and the screen would keep showing the stale unread count.
                if let Some(i) = self.index_of(screen) {
                    self.stack[i].live_burst = 0;
                    self.reload(i, Origin::User, fx);
                }
            }
            Err(e) => self.say(
                Tone::Error,
                cat([lit("Marking read failed: "), tt(&e.message)]),
            ),
        }
    }

    /// [`Action::ActivityLoaded`]: the Forum screen's Activity tab.
    fn activity_loaded(
        &mut self,
        screen: ScreenId,
        origin: Origin,
        result: Result<Box<ActivityData>, EngineError>,
        fx: &mut Vec<Effect>,
    ) {
        let h = self.content_height();
        let Some(i) = self.index_of(screen) else {
            return;
        };
        let is_top = i + 1 == self.stack.len();
        let s = &mut self.stack[i];
        s.loading = s.loading.saturating_sub(1);
        let origin = if s.settling { Origin::User } else { origin };
        let ScreenKind::Forum(f) = &mut s.kind else {
            return;
        };
        let mut failed = None;
        match result {
            Ok(d) => {
                s.error = None;
                receive_list(
                    &mut f.activity,
                    &mut f.activity_pending,
                    &mut f.activity_pos,
                    *d,
                    origin,
                    ForumScreen::activity_uris,
                );
            }
            Err(e) => {
                failed = Some(tt(&e.message));
                s.error = Some(e);
            }
        }
        let len = f.row_count();
        f.cursor_mut().reveal(len, h / LIST_ROWS);
        if s.dirty && s.loading == 0 {
            s.dirty = false;
            self.reload(i, Origin::Live, fx);
        }
        if let (Some(msg), true, Origin::User) = (failed, is_top, origin) {
            self.say(Tone::Error, cat([lit("Could not load activity: "), msg]));
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

    /// A key while the `:` command line is open: printable characters edit it, Enter runs the
    /// command, Esc closes it, and Tab completes a verb (`synthesis-v1.md` §6.11 Overlays).
    fn command_key(&mut self, k: &KeyEvent, fx: &mut Vec<Effect>) {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c')) {
            self.command = None;
            self.quit = true;
            fx.push(Effect::Quit);
            return;
        }
        match k.code {
            KeyCode::Esc => self.command = None,
            KeyCode::Enter => {
                let Some(line) = self.command.take() else {
                    return;
                };
                self.run_command(&line, fx);
            }
            KeyCode::Tab => {
                if let Some(c) = &mut self.command {
                    complete(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(c) = &mut self.command {
                    c.pop();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(line) = &mut self.command {
                    // a command line is words: drop control characters
                    if !c.is_control() {
                        line.push(c);
                    }
                }
            }
            _ => {}
        }
    }

    /// Runs a `:` command line against the engine through the same effects the keys use
    /// (`synthesis-v1.md` §6.11: the backstop that makes every MVP action reachable).
    fn run_command(&mut self, line: &str, fx: &mut Vec<Effect>) {
        use crate::command::{ModTarget, Verb, agree_intent, mod_intent, vote_intent};
        let line = line.trim();
        if line.is_empty() || line == "help" || line == "?" {
            self.say(Tone::Info, tt(&crate::command::usage()));
            return;
        }
        let cmd = match crate::command::parse(line) {
            Ok(c) => c,
            Err(why) => {
                self.say(Tone::Error, tt(&why));
                return;
            }
        };
        match cmd.verb {
            // a subject named on the command line wins over the selection (as `mod`)
            Verb::Vote(v) => {
                let Some((subject, forum)) = self.command_subject(cmd.subject.as_deref()) else {
                    self.say(Tone::Info, lit("Nothing selected to vote on."));
                    return;
                };
                self.opinion_intent(vote_intent(v), subject, forum, "Vote", fx);
            }
            Verb::Agree(a) => {
                let Some((subject, forum)) = self.command_subject(cmd.subject.as_deref()) else {
                    self.say(Tone::Info, lit("Nothing selected to vote on."));
                    return;
                };
                self.opinion_intent(agree_intent(a), subject, forum, "Vote", fx);
            }
            Verb::React { id, against, quote } => {
                let Some((subject, forum)) = self.command_subject(cmd.subject.as_deref()) else {
                    self.say(Tone::Info, lit("Nothing selected to react to."));
                    return;
                };
                // the tallies are known only for the thread on top; elsewhere the engine says
                // whether there is anything to react against
                if against
                    && self.thread_holds(&subject)
                    && !self.react_tallies(&subject).contains_key(&id)
                {
                    self.say(
                        Tone::Warn,
                        tt(&format!(
                            "Nobody has reacted {id} here yet, so there is nothing to react \
                             against."
                        )),
                    );
                    return;
                }
                let stance = if against {
                    Stance::Against
                } else {
                    Stance::For
                };
                self.opinion_intent(
                    OpinionIntent::React {
                        id,
                        stance,
                        // `exact` only: the engine derives the rest from the item's text
                        quote: quote.map(|exact| TextQuote {
                            exact,
                            prefix: None,
                            suffix: None,
                            start: None,
                        }),
                        remove: false,
                    },
                    subject,
                    forum,
                    "React",
                    fx,
                );
            }
            Verb::Unreact { id } => {
                let Some((subject, forum)) = self.command_subject(cmd.subject.as_deref()) else {
                    self.say(Tone::Info, lit("Nothing selected to react to."));
                    return;
                };
                let held = self.react_tallies(&subject).get(&id).and_then(|t| t.mine);
                // a removal takes the react back whatever its stance, so an item the screen does
                // not show is asked for with either; the engine says when there is none
                let held = match held {
                    None if !self.thread_holds(&subject) => Some(Stance::For),
                    h => h,
                };
                let Some(stance) = held else {
                    self.say(
                        Tone::Info,
                        tt(&format!(
                            "You have no {id} react on that item to take back."
                        )),
                    );
                    return;
                };
                self.opinion_intent(
                    OpinionIntent::React {
                        id,
                        stance,
                        quote: None,
                        remove: true,
                    },
                    subject,
                    forum,
                    "React",
                    fx,
                );
            }
            Verb::Mod(m) => {
                // a subject named on the command line wins; else the selection (the item, its
                // thread's post or its author, as the verb acts on)
                let named = cmd.subject.clone();
                let subject = match named.or_else(|| {
                    self.mod_subject().map(|sel| match m.target() {
                        ModTarget::Author => sel.author,
                        ModTarget::Post => sel.post,
                        ModTarget::Item => sel.item,
                    })
                }) {
                    Some(s) => s,
                    None => {
                        self.say(Tone::Info, lit("Nothing selected to moderate."));
                        return;
                    }
                };
                let Some(forum) = self.forum_for_subject(&subject) else {
                    self.say(Tone::Info, lit("Open a forum first."));
                    return;
                };
                if !self.can_write() {
                    return;
                }
                let Some(screen) = self.stack.last().map(|s| s.id) else {
                    return;
                };
                let Some(intent) = mod_intent(&m, &forum, &subject, cmd.reason.clone(), &cmd.opts)
                else {
                    // `approve` is a grant
                    fx.push(Effect::Grant {
                        forum,
                        actors: vec![subject],
                        screen,
                    });
                    self.say(Tone::Info, lit("Approving..."));
                    return;
                };
                self.say(Tone::Info, lit("Mod action..."));
                fx.push(Effect::ModAction {
                    intent,
                    forum,
                    screen,
                    merge: None,
                });
            }
            Verb::Notifications { all } => self.command_notifications(all, fx),
            Verb::Read => self.mark_selected_read(fx),
            Verb::ReadAll => self.mark_all_read(fx),
            Verb::Activity => {
                let Some(top) = self.stack.last_mut() else {
                    return;
                };
                if !matches!(&top.kind, ScreenKind::Forum(_)) {
                    self.say(Tone::Info, lit("Open a forum to see its activity."));
                    return;
                }
                if matches!(&top.kind, ScreenKind::Forum(f) if f.tab == ForumTab::Activity) {
                    self.say(Tone::Info, lit("The activity tab is already showing."));
                    return;
                }
                self.switch_forum_tab(fx);
            }
            Verb::Sort(name) => match self.stack.last().map(|s| &s.kind) {
                Some(ScreenKind::Forum(_)) => match name.post() {
                    Some(sort) => self.set_post_sort(sort, fx),
                    None => self.say(Tone::Info, lit("latest-reply sorts a thread's comments.")),
                },
                Some(ScreenKind::Thread(_)) => match name.comment() {
                    Some(sort) => self.set_comment_sort(sort, fx),
                    None => self.say(Tone::Info, lit("recent-comments sorts a forum's posts.")),
                },
                _ => self.say(Tone::Info, lit("Open a forum or a thread to sort it.")),
            },
            Verb::Filter(filter) => self.set_filter(filter, fx),
            Verb::User { actor } => {
                let actor = actor.or_else(|| self.mod_subject().map(|m| m.author));
                match actor {
                    Some(a) => self.show_user(&a, fx),
                    None => self.say(Tone::Info, lit("Nothing selected: name an actor.")),
                }
            }
            Verb::ModsList { role } => self.show_mods(role.as_deref()),
            Verb::ModsSet {
                subject,
                role,
                remove,
            } => {
                let Some(forum) = self.top_forum() else {
                    self.say(Tone::Info, lit("Open a forum first."));
                    return;
                };
                if !self.can_write() {
                    return;
                }
                let Some(screen) = self.stack.last().map(|s| s.id) else {
                    return;
                };
                let change = ModsChange::Add {
                    did: subject,
                    role,
                    title: None,
                };
                let change = if remove {
                    match change {
                        ModsChange::Add { did, .. } => ModsChange::Remove {
                            did,
                            skip_adopt: false,
                        },
                        r => r,
                    }
                } else {
                    change
                };
                self.say(Tone::Info, lit("Updating the moderator list..."));
                fx.push(Effect::ModsSet {
                    forum,
                    change,
                    screen,
                });
            }
            other => self.run_more(other, cmd.subject, fx),
        }
    }

    /// The `:` verbs past the M3 opinion and moderation set (`engine-api.md` §3.2, §3.5,
    /// §3.6): forums and membership, your own items, and the local state.
    fn run_more(
        &mut self,
        verb: crate::command::Verb,
        subject: Option<String>,
        fx: &mut Vec<Effect>,
    ) {
        use crate::command::{SubTarget, Verb};
        let screen = self.top().map_or(0, |s| s.id);
        // the forum a membership or forum verb acts on: the one on screen
        let forum = |st: &mut Self| -> Option<String> {
            let f = st.top_forum();
            if f.is_none() {
                st.say(Tone::Info, lit("Open a forum first."));
            }
            f
        };
        // the item a verb on one item acts on: the one named, else the selection
        let item = |st: &mut Self, what: &str| -> Option<(String, String)> {
            let it = st.command_subject(subject.as_deref());
            if it.is_none() {
                st.say(Tone::Info, tt(&format!("Nothing selected to {what}.")));
            }
            it
        };
        match verb {
            Verb::Open { target } => fx.push(Effect::Open { target }),
            Verb::Follow { forum: named, on } => {
                let Some(f) = named.or_else(|| forum(self)) else {
                    return;
                };
                fx.push(Effect::Follow {
                    forum: f.clone(),
                    on,
                });
                // a forum followed by name opens
                if on
                    && !self
                        .stack
                        .iter()
                        .any(|s| matches!(&s.kind, ScreenKind::Forum(x) if x.uri == f))
                {
                    fx.push(Effect::Open { target: f });
                }
            }
            Verb::Join { note } => {
                let Some(forum) = forum(self) else { return };
                if self.can_write() {
                    fx.push(Effect::Join {
                        forum,
                        note,
                        screen,
                    });
                }
            }
            Verb::Leave => {
                let Some(forum) = forum(self) else { return };
                if self.can_write() {
                    fx.push(Effect::Leave { forum, screen });
                }
            }
            Verb::Members { status, pending } => {
                let Some(forum) = forum(self) else { return };
                fx.push(Effect::LoadMembers {
                    forum,
                    status,
                    pending,
                });
            }
            Verb::Access { op, actors } => {
                let Some(forum) = forum(self) else { return };
                if self.can_write() {
                    fx.push(Effect::Access {
                        op,
                        forum,
                        actors,
                        screen,
                    });
                }
            }
            Verb::Adopt { former } => {
                let Some(forum) = forum(self) else { return };
                if self.can_write() {
                    fx.push(Effect::Adopt {
                        forum,
                        former,
                        screen,
                    });
                }
            }
            Verb::ForumCreate { slug, name, open } => {
                if !self.can_write() {
                    return;
                }
                let mut draft = ForumDraft::new(slug, name);
                if open {
                    draft.membership = MembershipMode::Open;
                }
                fx.push(Effect::ForumCreate { draft });
            }
            Verb::ForumEdit(changes) => {
                let Some(forum) = forum(self) else { return };
                if self.can_write() {
                    fx.push(Effect::ForumEdit {
                        forum,
                        changes,
                        screen,
                    });
                }
            }
            Verb::Edit => {
                let Some((uri, forum)) = item(self, "edit") else {
                    return;
                };
                if self.can_write() {
                    fx.push(Effect::StartEdit { uri, forum });
                }
            }
            Verb::Delete | Verb::Retract | Verb::Redact => {
                let (op, what) = match verb {
                    Verb::Delete => (ItemOp::Delete, "delete"),
                    Verb::Retract => (ItemOp::Retract, "retract"),
                    _ => (ItemOp::Redact, "redact"),
                };
                let Some((uri, forum)) = item(self, what) else {
                    return;
                };
                if !self.can_write() {
                    return;
                }
                let e = Effect::ItemWrite {
                    op,
                    uri,
                    forum,
                    screen,
                };
                // neither a deletion nor a redaction can be undone: ask first
                let question = match op {
                    ItemOp::Delete => Some("Delete this item, with its revisions?"),
                    ItemOp::Redact => {
                        Some("Delete this item's version history? It cannot be undone.")
                    }
                    ItemOp::Retract => None,
                };
                match question {
                    Some(q) => {
                        self.confirm = Some(Confirm {
                            text: q.to_owned(),
                            effects: vec![e],
                        });
                    }
                    None => fx.push(e),
                }
            }
            Verb::Raw => {
                let Some((uri, forum)) = item(self, "show") else {
                    return;
                };
                fx.push(Effect::LoadRaw { uri, forum });
            }
            Verb::Bookmark { on } => {
                let Some((uri, forum)) = item(self, "bookmark") else {
                    return;
                };
                if self.can_write() {
                    fx.push(Effect::Bookmark { uri, forum, on });
                }
            }
            Verb::Subscribe { target, on } => {
                let subscription = match target {
                    SubTarget::Post(named) => {
                        // a comment's subscription is its thread's post
                        let post = named.or_else(|| {
                            self.top_thread()
                                .and_then(|t| t.post())
                                .map(|p| p.post.uri.clone())
                                .or_else(|| {
                                    self.top_forum_screen()?.selected().map(|p| p.uri.clone())
                                })
                        });
                        let Some(uri) = post else {
                            self.say(Tone::Info, lit("Nothing selected to subscribe to."));
                            return;
                        };
                        Subscription::Post { uri }
                    }
                    SubTarget::Forum => {
                        let Some(uri) = forum(self) else { return };
                        Subscription::Forum { uri }
                    }
                    SubTarget::User(did) => Subscription::User { did },
                    SubTarget::Tag(slug) => {
                        let Some(forum) = forum(self) else { return };
                        Subscription::Tag { forum, slug }
                    }
                };
                if self.can_write() {
                    fx.push(Effect::Subscribe { subscription, on });
                }
            }
            Verb::Mute { actor, on } => {
                let Some(actor) = actor.or_else(|| self.mod_subject().map(|m| m.author)) else {
                    self.say(Tone::Info, lit("Nothing selected: name an actor."));
                    return;
                };
                if self.can_write() {
                    fx.push(Effect::Mute { actor, on });
                }
            }
            Verb::KarmaNotifications(cadence) => {
                if cadence.is_none() || self.can_write() {
                    fx.push(Effect::KarmaNotifications { cadence });
                }
            }
            Verb::Settings { key, value } => fx.push(Effect::Settings {
                set: key.map(|k| (k, value)),
            }),
            Verb::Undo => self.undo_last(fx),
            Verb::Retry { id } => {
                let id = match (id, self.failed.len()) {
                    (Some(id), _) => id,
                    (None, 0) => {
                        self.say(Tone::Info, lit("No failed write to send again."));
                        return;
                    }
                    (None, 1) => self.failed.iter().next().cloned().unwrap_or_default(),
                    (None, _) => {
                        let ids: Vec<&str> = self.failed.iter().map(String::as_str).collect();
                        self.say(
                            Tone::Info,
                            tt(&format!("Which write? retry one of: {}", ids.join(", "))),
                        );
                        return;
                    }
                };
                if self.can_write() {
                    fx.push(Effect::Retry { id });
                }
            }
            // the M3 verbs are run_command's
            _ => {}
        }
    }

    /// `notifications [all|unread]`: opens the Notifications screen, or switches the one on top.
    fn command_notifications(&mut self, all: bool, fx: &mut Vec<Effect>) {
        let on_top = matches!(
            self.stack.last().map(|s| &s.kind),
            Some(ScreenKind::Notifications(_))
        );
        if on_top {
            let Some(ScreenKind::Notifications(n)) = self.stack.last().map(|s| &s.kind) else {
                return;
            };
            if n.all == all {
                self.say(
                    Tone::Info,
                    lit(if all {
                        "Already showing all notifications."
                    } else {
                        "Already showing the unread notifications."
                    }),
                );
                return;
            }
            if let Some(ScreenKind::Notifications(n)) = self.stack.last_mut().map(|s| &mut s.kind) {
                n.all = all;
                n.data = None;
            }
            if let Some(i) = self.stack.len().checked_sub(1) {
                self.reload(i, Origin::User, fx);
            }
            return;
        }
        self.open_notifications(fx);
        // `all` after the push: the screen loads the unread ones when it opens, so switch it
        // and reload once it exists
        if all {
            if let Some(ScreenKind::Notifications(n)) = self.stack.last_mut().map(|s| &mut s.kind) {
                n.all = true;
                n.data = None;
            }
            if let Some(i) = self.stack.len().checked_sub(1) {
                self.reload(i, Origin::User, fx);
            }
        }
    }

    /// The forum a subject is in: the one a screen on the stack shows it in, else the forum
    /// the keys would use (`top_forum`). A command line that names an at-uri or a DID does not
    /// say which forum it means, so the forum on the stack stands in for it; the engine is
    /// authoritative about whether the action applies.
    fn forum_for_subject(&self, subject: &str) -> Option<String> {
        for s in self.stack.iter().rev() {
            match &s.kind {
                ScreenKind::Forum(f) => {
                    let lists = f.data.as_ref().is_some_and(|d| {
                        d.list
                            .items
                            .iter()
                            .any(|p| p.uri == subject || p.author.did == subject)
                    }) || f.activity.as_ref().is_some_and(|a| {
                        a.list
                            .items
                            .iter()
                            .any(|i| i.post.uri == subject || i.post.author.did == subject)
                    });
                    if lists {
                        return Some(f.uri.clone());
                    }
                }
                ScreenKind::Thread(t) => {
                    if let Some(d) = &t.data {
                        match &d.view.post {
                            ThreadRoot::Post(p) => return Some(p.post.forum.clone()),
                            ThreadRoot::Missing { .. } => {
                                return self.top_forum();
                            }
                        }
                    }
                }
                ScreenKind::Home(_) | ScreenKind::Mod(_) | ScreenKind::Notifications(_) => {}
            }
        }
        self.top_forum()
    }

    /// One opinion effect, after the same gates the opinion keys use.
    fn opinion_intent(
        &mut self,
        intent: OpinionIntent,
        subject: String,
        forum: String,
        label: &'static str,
        fx: &mut Vec<Effect>,
    ) {
        if !self.can_write() {
            return;
        }
        if let Some(why) = self.opinion_gate(op_of(&intent), self.caps_of(&forum)) {
            self.say(Tone::Warn, why);
            return;
        }
        let Some(screen) = self.stack.last().map(|s| s.id) else {
            return;
        };
        fx.push(Effect::Opinion {
            subject,
            intent,
            forum,
            screen,
            label,
        });
    }

    /// `user [actor]`: who an actor is here, from what the loaded forum view holds (their role
    /// in the moderator list, and whether they wrote the selected item).
    fn show_user(&mut self, actor: &str, _fx: &mut Vec<Effect>) {
        let Some(forum) = self.top_forum() else {
            self.say(Tone::Info, lit("Open a forum first."));
            return;
        };
        let Some(view) = self.stack.iter().rev().find_map(|s| match &s.kind {
            ScreenKind::Forum(f) if f.uri == forum => f.data.as_ref().map(|d| &d.view),
            _ => None,
        }) else {
            self.say(Tone::Info, lit("The forum is still loading."));
            return;
        };
        let role = view
            .moderators
            .iter()
            .find(|m| m.actor.did == actor || m.actor.handle.as_deref() == Some(actor));
        let wrote_selected = self.mod_subject().is_some_and(|m| m.author == actor);
        match role {
            Some(m) => {
                let mut parts = vec![
                    crate::text::actor(&m.actor),
                    tt(&format!("{:?}", m.role).to_lowercase()),
                ];
                if let Some(t) = &m.title {
                    parts.push(tt(&format!("\"{t}\"")));
                }
                if !m.shown {
                    parts.push(lit("not a member"));
                }
                self.say(Tone::Info, crate::text::joined(parts));
            }
            None if wrote_selected => self.say(
                Tone::Info,
                tt(&format!(
                    "{actor} wrote the selected item, and is not a declared moderator here."
                )),
            ),
            None => self.say(
                Tone::Info,
                tt(&format!(
                    "{actor} is not a declared moderator of this forum."
                )),
            ),
        }
    }

    /// `mods [role]`: the forum's declared moderators from its view.
    fn show_mods(&mut self, role: Option<&str>) {
        let Some(forum) = self.top_forum() else {
            self.say(Tone::Info, lit("Open a forum first."));
            return;
        };
        let Some(view) = self.stack.iter().rev().find_map(|s| match &s.kind {
            ScreenKind::Forum(f) if f.uri == forum => f.data.as_ref().map(|d| &d.view),
            _ => None,
        }) else {
            self.say(Tone::Info, lit("The forum is still loading."));
            return;
        };
        if view.moderators.is_empty() {
            self.say(Tone::Info, lit("This forum has no declared moderators."));
            return;
        }
        let wanted = role.map(str::to_lowercase);
        let names: Vec<String> = view
            .moderators
            .iter()
            .filter(|m| {
                wanted
                    .as_ref()
                    .is_none_or(|w| format!("{:?}", m.role).to_lowercase().contains(w.as_str()))
            })
            .map(|m| {
                format!(
                    "{} ({:?})",
                    m.actor
                        .handle
                        .clone()
                        .unwrap_or_else(|| m.actor.short_did.clone()),
                    m.role
                )
            })
            .collect();
        if names.is_empty() {
            self.say(Tone::Info, lit("No moderators with that role."));
            return;
        }
        self.say(Tone::Info, tt(&format!("Moderators: {}", names.join(", "))));
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
                    merge: p.merge,
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
        fx: &mut Vec<Effect>,
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
        if s.dirty && s.loading == 0 {
            s.dirty = false;
            self.reload(i, Origin::Live, fx);
        }
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
        // a load the user asked for (opening the thread, R, an own write) shows the thread to
        // them: it is read. A live load is not, nor one that only counts as the user's while
        // the screen settles (marking emits `Changed [Thread]`, which would reload it again)
        let viewed = origin == Origin::User && is_top;
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
                        // a load for another sort or filter is superseded by the one in flight
                        if d.list.sort == s.sort && d.list.filter == s.filter {
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
                    // as for a forum: a load in another comment sort is superseded
                    (ScreenKind::Thread(t), Loaded::Thread(d)) if d.view.sort != t.opts.sort => {}
                    (ScreenKind::Thread(t), Loaded::Thread(d)) => {
                        if viewed && let ThreadRoot::Post(p) = &d.view.post {
                            fx.push(Effect::MarkThreadRead {
                                post: p.post.uri.clone(),
                            });
                        }
                        t.receive(*d, origin, w, h);
                    }
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
        // the thread on top shows what it now holds: what the autoload policy loads, loads
        if is_top && matches!(self.stack[i].kind, ScreenKind::Thread(_)) {
            self.autoload_media(fx);
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
                // the header's badge follows the notifications (re-queried on a Notifications,
                // Forum or Thread topic, `engine-api.md` §3.4)
                if topics.iter().any(|t| {
                    matches!(
                        t,
                        Topic::Notifications | Topic::Forum { .. } | Topic::Thread { .. }
                    )
                }) {
                    fx.push(Effect::LoadUnread);
                }
                // another client on this device may have acknowledged an interstitial
                if self.account.is_some() && topics.contains(&Topic::Accounts) {
                    fx.push(Effect::LoadAcknowledged);
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

/// The inline images of the item `uri` of a thread, when its body shows (`visible_only`: only
/// for a `Visible` item, as autoload requires).
fn item_media(
    d: &ThreadData,
    uri: &str,
    visible_only: bool,
) -> Vec<lessdugin_core::dto::MediaView> {
    use lessdugin_core::dto::ItemState;
    let shown = |st: &ItemState| match st {
        ItemState::Visible => true,
        ItemState::Collapsed { .. } => !visible_only,
        _ => false,
    };
    let media = match &d.view.post {
        ThreadRoot::Post(p) if p.post.uri == uri => shown(&p.post.state).then_some(&p.body.media),
        _ => d
            .view
            .comments
            .iter()
            .find(|c| c.uri == uri && !c.missing)
            .and_then(|c| shown(&c.state).then_some(&c.body.media)),
    };
    media
        .into_iter()
        .flatten()
        .filter(|m| thread::inline_image(&m.mime))
        .cloned()
        .collect()
}

/// The `:members` panel: one member per line, with what `MemberView` says of them.
fn members_lines(members: &[MemberView]) -> Vec<TermText> {
    if members.is_empty() {
        return vec![lit("No members match.")];
    }
    members
        .iter()
        .map(|m| {
            let role = match m.status {
                ForumRole::Owner => "owner",
                ForumRole::Admin => "admin",
                ForumRole::Moderator => "moderator",
                ForumRole::Member => "member",
                ForumRole::Established => "established",
                ForumRole::Newcomer => "newcomer",
                ForumRole::None => "not a participant",
            };
            let mut parts = vec![actor(&m.actor), lit(role)];
            if m.applicant {
                parts.push(lit("applicant"));
            }
            if m.granted {
                parts.push(lit("granted"));
            }
            if let Some(k) = m.karma {
                parts.push(tt(&format!("karma {k}")));
            }
            if m.banned {
                parts.push(lit("banned"));
            }
            if m.nullified {
                parts.push(lit("votes nullified"));
            }
            if !m.restricted.is_empty() {
                parts.push(tt(&format!("restricted: {}", m.restricted.join(", "))));
            }
            if let Some(n) = &m.membership_note {
                parts.push(cat([lit("note: "), tt(n)]));
            }
            crate::text::joined(parts)
        })
        .collect()
}

/// The `:raw` panel (`social.md` §10.3, "shows everything, with annotations"): the record's
/// state, validity and score, the mod actions naming it, and its JSON.
fn raw_lines(r: &RawView) -> Vec<TermText> {
    let json = |v: &serde_json::Value| serde_json::to_string_pretty(v).unwrap_or_default();
    let mut out = vec![tt(&r.uri)];
    if let Some(c) = &r.cid {
        out.push(cat([lit("cid "), tt(c)]));
    }
    if let Some(st) = &r.state {
        out.push(cat([
            lit("state "),
            tt(&serde_json::to_string(st).unwrap_or_default()),
        ]));
    }
    if let Some(sc) = &r.score {
        out.push(tt(&format!("score {}", sc.score)));
    }
    if r.validity.is_empty() {
        out.push(lit("valid"));
    }
    for v in &r.validity {
        out.push(cat([lit("violation: "), tt(v)]));
    }
    out.push(tt(&format!("{} mod actions name it", r.assertions.len())));
    for a in &r.assertions {
        out.extend(json(a).lines().map(|l| cat([lit("  "), tt(l)])));
    }
    match &r.record {
        Some(v) => {
            out.push(lit("record:"));
            out.extend(json(v).lines().map(tt));
        }
        None => out.push(lit("The record is not cached.")),
    }
    out
}

/// The `:settings` panel.
fn settings_lines(s: &Settings) -> Vec<TermText> {
    let v = serde_json::to_value(s).unwrap_or_default();
    let mut out = Vec::new();
    if let Some(o) = v.as_object() {
        for (k, val) in o {
            out.push(tt(&format!("{k}: {val}")));
        }
    }
    out.push(lit(
        "Change one with `set <key> <value>`; `unset <key>` restores it.",
    ));
    out
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
                report: Vec::new(),
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
                    report: Vec::new(),
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

    #[test]
    fn the_command_line_completes_verbs_and_arguments() {
        let mut line = String::from("vo");
        complete(&mut line);
        assert_eq!(line, "vote");

        // a unique argument completes too
        let mut line = String::from("vote str");
        complete(&mut line);
        assert_eq!(line, "vote strong-up");

        // no completion available leaves the line alone
        let mut line = String::from("vote zzz");
        complete(&mut line);
        assert_eq!(line, "vote zzz");

        // a trailing space does nothing
        let mut line = String::from("vote ");
        complete(&mut line);
        assert_eq!(line, "vote ");
    }

    /// A writing account.
    fn me() -> AccountView {
        AccountView {
            did: "did:plc:me".into(),
            handle: None,
            pds: "https://pds.example".into(),
            auth_kind: lessdugin_core::dto::AuthKind::AppPassword,
            active: true,
        }
    }

    fn notification_loads(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| matches!(e, Effect::LoadNotifications { .. }))
            .count()
    }

    /// Notifications are computed lazily: the Notifications screen re-queries on a Forum or
    /// Thread topic (nothing emits `Notifications` for a new reply), and a reload asked for
    /// while its load runs is not dropped.
    #[test]
    fn the_notifications_screen_follows_forum_and_thread_changes() {
        let mut st = State::new(80, 24, Some(me()), true);
        st.start(None);
        let fx = st.update(Action::Key(keys::parse_key("N").unwrap()));
        assert_eq!(notification_loads(&fx), 1, "{fx:?}");
        let screen = st.top().unwrap().id;
        // a change while the first load runs is kept for when it finishes
        let fx = changed(
            &mut st,
            Topic::Thread {
                uri: "at://t".into(),
            },
        );
        assert_eq!(notification_loads(&fx), 0, "{fx:?}");
        let fx = st.update(Action::NotificationsLoaded {
            screen,
            origin: Origin::User,
            result: Ok(NotificationList {
                dto: Default::default(),
                items: Vec::new(),
                unread: 0,
            }),
        });
        assert_eq!(notification_loads(&fx), 1, "the dirty reload runs: {fx:?}");
        st.update(Action::NotificationsLoaded {
            screen,
            origin: Origin::Live,
            result: Ok(NotificationList {
                dto: Default::default(),
                items: Vec::new(),
                unread: 0,
            }),
        });
        // a forum change re-queries them too
        let fx = changed(
            &mut st,
            Topic::Forum {
                uri: "at://f".into(),
            },
        );
        assert_eq!(notification_loads(&fx), 1, "{fx:?}");
    }

    fn forum_loads(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| matches!(e, Effect::LoadForum { .. }))
            .count()
    }

    /// Past `LIVE_RELOAD_CAP` live reloads with no key in between, a change is not loaded at
    /// once, but it is not lost either: the user's next move reloads the screen.
    #[test]
    fn a_capped_live_update_reloads_on_the_next_move() {
        let mut st = State::new(80, 24, None, true);
        st.start(None);
        st.open_forum("at://f".into());
        let screen = st.top().unwrap().id;
        let finish = |st: &mut State| {
            st.update(Action::ForumLoaded {
                screen,
                origin: Origin::Live,
                result: Err(EngineError::internal("offline")),
            });
        };
        finish(&mut st);
        for _ in 0..LIVE_RELOAD_CAP {
            let fx = changed(
                &mut st,
                Topic::Forum {
                    uri: "at://f".into(),
                },
            );
            assert_eq!(forum_loads(&fx), 1, "{fx:?}");
            finish(&mut st);
        }
        let fx = changed(
            &mut st,
            Topic::Forum {
                uri: "at://f".into(),
            },
        );
        assert_eq!(forum_loads(&fx), 0, "capped: {fx:?}");
        assert!(st.top().unwrap().stale);
        let fx = st.update(Action::Key(keys::parse_key("j").unwrap()));
        assert_eq!(forum_loads(&fx), 1, "the move reloads: {fx:?}");
        assert!(!st.top().unwrap().stale);
    }

    /// The Mod screen follows its forum's changes (a new applicant), and a reload asked for
    /// while its loads run is not dropped.
    #[test]
    fn the_mod_screen_follows_its_forum() {
        let mut st = State::new(80, 24, None, true);
        st.start(None);
        st.open_forum("at://f".into());
        let mut fx = Vec::new();
        st.push(
            ScreenKind::Mod(Box::new(ModScreen {
                forum: "at://f".into(),
                ..ModScreen::default()
            })),
            &mut fx,
        );
        let screen = st.top().unwrap().id;
        let queue_loads = |fx: &[Effect]| {
            fx.iter()
                .filter(|e| matches!(e, Effect::LoadQueue { .. }))
                .count()
        };
        let fx = changed(
            &mut st,
            Topic::Forum {
                uri: "at://f".into(),
            },
        );
        assert_eq!(queue_loads(&fx), 0, "loading: kept for later: {fx:?}");
        let empty = || ModLog {
            dto: Default::default(),
            entries: Vec::new(),
            cursor: None,
        };
        st.update(Action::LogLoaded {
            screen,
            origin: Origin::User,
            result: Ok(empty()),
        });
        let fx = st.update(Action::LogLoaded {
            screen,
            origin: Origin::User,
            result: Ok(empty()),
        });
        assert_eq!(queue_loads(&fx), 1, "the dirty reload runs: {fx:?}");
        st.update(Action::LogLoaded {
            screen,
            origin: Origin::Live,
            result: Ok(empty()),
        });
        st.update(Action::LogLoaded {
            screen,
            origin: Origin::Live,
            result: Ok(empty()),
        });
        let fx = changed(
            &mut st,
            Topic::Forum {
                uri: "at://f".into(),
            },
        );
        assert_eq!(queue_loads(&fx), 1, "{fx:?}");
        let fx = changed(
            &mut st,
            Topic::Forum {
                uri: "at://g".into(),
            },
        );
        assert_eq!(queue_loads(&fx), 0, "another forum: {fx:?}");
    }

    /// The subject of every mod-menu entry that acts on an actor (a ban, a restriction).
    fn actor_subjects(m: &ModMenu) -> Vec<(String, ModScope)> {
        m.entries
            .iter()
            .filter_map(|e| match e.intent.as_ref()? {
                ModActionIntent::Assert {
                    subject,
                    scope,
                    action: ModActionValue::Access { .. } | ModActionValue::Restrict { .. },
                    ..
                } => Some((subject.clone(), *scope)),
                _ => None,
            })
            .collect()
    }

    fn author_entries(m: &ModMenu) -> Vec<&'static str> {
        m.entries
            .iter()
            .filter(|e| {
                matches!(
                    e.intent,
                    Some(ModActionIntent::Assert {
                        scope: ModScope::Author,
                        ..
                    })
                )
            })
            .map(|e| e.label)
            .collect()
    }

    /// Author moderation (`social.md` §4.9) belongs to the author of the thread's POST and acts
    /// on other people's comments there; nothing in the menu ever names the viewer as the
    /// subject of a ban or restriction.
    #[test]
    fn the_mod_menu_offers_author_moderation_in_the_viewers_own_thread() {
        let st = State::new(80, 24, Some(me()), true);
        let author_caps = ViewerCaps {
            author_mod_personal: true,
            author_mod_frontpage: true,
            ..ViewerCaps::default()
        };
        let sel = |author: &str, post_author: &str| ModSubject {
            item: "at://did:plc:x/c/1".into(),
            forum: "at://f".into(),
            author: author.into(),
            post: "at://did:plc:y/p/1".into(),
            post_author: post_author.into(),
            comment: true,
        };
        // someone else's comment in the viewer's own thread: hide, pin and ban, author scope
        let m = st.mod_menu_for(&sel("did:plc:other", "did:plc:me"), Some(&author_caps));
        let labels = author_entries(&m);
        assert!(
            labels.contains(&"hide this comment (your thread)"),
            "{labels:?}"
        );
        assert!(
            labels.contains(&"ban the author from your threads"),
            "{labels:?}"
        );
        assert_eq!(
            actor_subjects(&m),
            [("did:plc:other".to_owned(), ModScope::Author)],
            "{m:?}"
        );
        // the viewer's own comment in their own thread: nothing aimed at themselves
        let m = st.mod_menu_for(&sel("did:plc:me", "did:plc:me"), Some(&author_caps));
        assert!(actor_subjects(&m).is_empty(), "{m:?}");
        assert!(
            !author_entries(&m).contains(&"hide this comment (your thread)"),
            "{m:?}"
        );
        // the viewer's own comment in someone else's thread: no author tier at all
        let m = st.mod_menu_for(&sel("did:plc:me", "did:plc:other"), Some(&author_caps));
        assert!(author_entries(&m).is_empty(), "{m:?}");
        assert!(actor_subjects(&m).is_empty(), "{m:?}");
        // a moderator on their own item: no forum-tier ban or restriction of themselves
        let modcaps = ViewerCaps {
            moderate: true,
            rank: Some(1),
            ..ViewerCaps::default()
        };
        let m = st.mod_menu_for(&sel("did:plc:me", "did:plc:other"), Some(&modcaps));
        assert!(actor_subjects(&m).is_empty(), "{m:?}");
    }

    /// The forum tier's overrides assert the opposite value (`social.md` §4.2); a pin of a
    /// comment is a thread pin; a lock or a tier acts on the thread's post.
    #[test]
    fn the_mod_menu_overrides_assert_and_pins_by_kind() {
        let st = State::new(80, 24, Some(me()), true);
        let modcaps = ViewerCaps {
            moderate: true,
            rank: Some(2),
            curate: true,
            ..ViewerCaps::default()
        };
        let m = st.mod_menu_for(
            &ModSubject {
                item: "at://did:plc:x/c/1".into(),
                forum: "at://f".into(),
                author: "did:plc:x".into(),
                post: "at://did:plc:y/p/1".into(),
                post_author: "did:plc:y".into(),
                comment: true,
            },
            Some(&modcaps),
        );
        let of = |label: &str| {
            m.entries
                .iter()
                .find(|e| e.label == label)
                .and_then(|e| e.intent.clone())
                .unwrap_or_else(|| panic!("no {label:?} in {m:?}"))
        };
        assert!(matches!(
            of("keep visible (override a hide)"),
            ModActionIntent::Assert {
                action: ModActionValue::Hide { hidden: false, .. },
                ..
            }
        ));
        assert!(matches!(
            of("unlock (override a lock)"),
            ModActionIntent::Assert {
                action: ModActionValue::Lock { locked: false },
                ref subject,
                ..
            } if subject == "at://did:plc:y/p/1"
        ));
        assert!(matches!(
            of("pin in the thread"),
            ModActionIntent::Assert {
                action: ModActionValue::Pin {
                    scope: PinScope::Thread,
                    pinned: true
                },
                ..
            }
        ));
        assert!(matches!(
            of("unpin (override a pin)"),
            ModActionIntent::Assert {
                action: ModActionValue::Pin {
                    scope: PinScope::Thread,
                    pinned: false
                },
                ..
            }
        ));
        assert!(matches!(
            of("retract your hide"),
            ModActionIntent::Remove { ref action_key, .. } if action_key == "hide"
        ));
        // a nullification names only nullifyVotes: the engine keeps the restriction (m3r-2)
        assert!(matches!(
            of("nullify the author's votes"),
            ModActionIntent::Assert {
                action: ModActionValue::Restrict {
                    capabilities: None,
                    nullify_votes: Some(true)
                },
                ..
            }
        ));
        assert!(matches!(
            of("restore the author's votes"),
            ModActionIntent::Assert {
                action: ModActionValue::Restrict {
                    capabilities: None,
                    nullify_votes: Some(false)
                },
                ..
            }
        ));
        // the capability presets add to what the record restricts already
        for label in [
            "restrict the author (posting)",
            "restrict the author (voting, reacting)",
        ] {
            let e = m.entries.iter().find(|e| e.label == label).unwrap();
            assert_eq!(e.merge, Some(RestrictDim::AddCapabilities), "{label}");
        }
    }

    /// A multi-byte space (NBSP from macOS Option-Space, U+3000 from a CJK IME) before `Tab`
    /// completes the word after it instead of slicing inside the space and panicking.
    #[test]
    fn completion_after_a_wide_space_does_not_panic() {
        for space in ['\u{a0}', '\u{3000}'] {
            let mut line = format!("vote{space}str");
            complete(&mut line);
            assert_eq!(line, format!("vote{space}strong-up"));
            let mut line = format!("vote{space}");
            complete(&mut line);
            assert_eq!(line, format!("vote{space}"));

            // the same through the key loop: the overlay keeps the text, the TUI keeps running
            let mut st = State::new(80, 24, None, true);
            st.start(None);
            st.update(Action::Key(keys::parse_key(":").unwrap()));
            for c in format!("vote{space}").chars() {
                st.update(Action::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )));
            }
            st.update(Action::Key(keys::parse_key("Tab").unwrap()));
            assert_eq!(st.command.as_deref(), Some(format!("vote{space}").as_str()));
        }
    }
}
