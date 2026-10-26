// SPDX-License-Identifier: AGPL-3.0-or-later
//! Arguments of the M2 forum and content commands (`docs/design/synthesis-v1.md` §6.10).
//!
//! References (`REF`): `3` (from the last `ls` in this terminal), `c12` (a comment label from
//! the last `read`), an at-uri, an SPA link, or `<handle>/<rkey>` (a post of the current
//! forum). Forum references (`FORUM`): `<handle|did>/<slug>`, an at-uri, an SPA link, an alias
//! or a slug of a followed forum, or a number from the last `forum ls`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use lessdugin_core::dto::{CommentSort, MembershipMode, PostKind, PostSort};

/// `--sort` of `ls`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum PostSortArg {
    /// Score with time decay (the M3 default, `engine-api.md` §3.4).
    #[default]
    Magic,
    /// Highest score first.
    Top,
    /// Newest first.
    New,
    /// Oldest first.
    Old,
    /// Most recent comment first.
    RecentComments,
}

impl From<PostSortArg> for PostSort {
    fn from(s: PostSortArg) -> Self {
        match s {
            PostSortArg::New => PostSort::New,
            PostSortArg::Old => PostSort::Old,
            PostSortArg::Top => PostSort::Top,
            PostSortArg::Magic => PostSort::Magic,
            PostSortArg::RecentComments => PostSort::RecentComments,
        }
    }
}

/// `--sort` of `read` (comments).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum CommentSortArg {
    /// Score with time decay.
    #[default]
    Magic,
    /// Highest score first.
    Top,
    /// Newest first.
    New,
    /// Oldest first.
    Old,
    /// Most recent reply first.
    LatestReply,
}

impl From<CommentSortArg> for CommentSort {
    fn from(s: CommentSortArg) -> Self {
        match s {
            CommentSortArg::Magic => CommentSort::Magic,
            CommentSortArg::Top => CommentSort::Top,
            CommentSortArg::New => CommentSort::New,
            CommentSortArg::Old => CommentSort::Old,
            CommentSortArg::LatestReply => CommentSort::LatestReply,
        }
    }
}

/// `--kind` of a post.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PostKindArg {
    /// An article (the default).
    Article,
    /// A link post (needs `--link`).
    Link,
    /// A question.
    Question,
    /// A quick take (no title needed).
    QuickTake,
}

impl From<PostKindArg> for PostKind {
    fn from(k: PostKindArg) -> Self {
        match k {
            PostKindArg::Article => PostKind::Article,
            PostKindArg::Link => PostKind::Link,
            PostKindArg::Question => PostKind::Question,
            PostKindArg::QuickTake => PostKind::QuickTake,
        }
    }
}

/// `--mode` of `forum create`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    /// Members need a grant from the owner or an admin (`approve`).
    #[default]
    Approval,
    /// Anyone who joins is a member.
    Open,
}

impl From<ModeArg> for MembershipMode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Approval => MembershipMode::Approval,
            ModeArg::Open => MembershipMode::Open,
        }
    }
}

/// `read`.
#[derive(Debug, Args)]
pub struct ReadArgs {
    /// The post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// Comment order.
    #[arg(long, value_enum, default_value_t = CommentSortArg::Magic)]
    pub sort: CommentSortArg,
    /// Only the post.
    #[arg(long)]
    pub no_comments: bool,
    /// The stored record with its validity and moderation annotations (RawView).
    #[arg(long, conflicts_with = "no_comments")]
    pub raw: bool,
    /// Print directly instead of through `less -R`.
    #[arg(long)]
    pub no_pager: bool,
}

/// `ls`.
#[derive(Debug, Args)]
pub struct LsArgs {
    /// The forum (default: --forum, then the current forum).
    #[arg(value_name = "FORUM")]
    pub forum: Option<String>,
    /// Post order.
    #[arg(long, value_enum, default_value_t = PostSortArg::Magic)]
    pub sort: PostSortArg,
    /// Every listed post, of any tier and kind.
    #[arg(long, conflicts_with_all = ["frontpage", "curated"])]
    pub all: bool,
    /// Frontpage posts only (the default, `social.md` §12.3).
    #[arg(long, conflicts_with = "curated")]
    pub frontpage: bool,
    /// Curated posts only.
    #[arg(long)]
    pub curated: bool,
    /// Only posts of this kind.
    #[arg(long, value_enum)]
    pub kind: Option<PostKindArg>,
    /// Only posts with this tag.
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,
    /// Only posts by this author (handle or DID).
    #[arg(long, value_name = "ACTOR")]
    pub author: Option<String>,
    /// At most this many posts (1–500).
    #[arg(long, short = 'n', default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=500))]
    pub limit: u32,
}

/// `post`.
#[derive(Debug, Args)]
pub struct PostArgs {
    /// A markdown file (with optional YAML frontmatter), or `-` for standard input. Without
    /// one, $EDITOR opens a new draft.
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// The title.
    #[arg(long)]
    pub title: Option<String>,
    /// The kind of post.
    #[arg(long, value_enum)]
    pub kind: Option<PostKindArg>,
    /// The URL of a link post.
    #[arg(long, value_name = "URL")]
    pub link: Option<String>,
    /// A tag (repeatable).
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// Save as a draft instead of publishing.
    #[arg(long)]
    pub draft: bool,
    /// Publish without asking.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// `comment`.
#[derive(Debug, Args)]
pub struct CommentArgs {
    /// The post (a top-level comment) or the comment replied to.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// A markdown file, or `-` for standard input. Without one (or -m), $EDITOR opens.
    #[arg(value_name = "FILE", conflicts_with = "message")]
    pub file: Option<PathBuf>,
    /// The comment text.
    #[arg(long, short = 'm', value_name = "TEXT")]
    pub message: Option<String>,
}

/// `edit`.
#[derive(Debug, Args)]
pub struct EditArgs {
    /// Your post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// The new text: a markdown file (a post's may have frontmatter), or `-` for standard
    /// input. Without one (or -m), $EDITOR opens on the current text.
    #[arg(value_name = "FILE", conflicts_with = "message")]
    pub file: Option<PathBuf>,
    /// The new text.
    #[arg(long, short = 'm', value_name = "TEXT")]
    pub message: Option<String>,
    /// A new title (posts).
    #[arg(long)]
    pub title: Option<String>,
    /// An edit summary, kept with the old version.
    #[arg(long, value_name = "TEXT")]
    pub summary: Option<String>,
    /// Do not keep the old version as a revision.
    #[arg(long)]
    pub no_history: bool,
}

/// `delete`.
#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Your post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// Do not ask for confirmation (needed when not running interactively).
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Commands that take one reference.
#[derive(Debug, Args)]
pub struct RefArgs {
    /// The post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
}

/// `sync`.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// The forum (default: --forum, then the current forum, else every followed forum).
    #[arg(value_name = "FORUM")]
    pub forum: Option<String>,
    /// Re-list every repository instead of checking heads.
    #[arg(long)]
    pub full: bool,
}

/// `join`.
#[derive(Debug, Args)]
pub struct JoinArgs {
    /// The forum.
    #[arg(value_name = "FORUM")]
    pub forum: String,
    /// A note for the moderators (approval-mode forums).
    #[arg(long, value_name = "TEXT")]
    pub note: Option<String>,
    /// A short local name for the forum.
    #[arg(long, value_name = "ALIAS")]
    pub alias: Option<String>,
}

/// `leave`.
#[derive(Debug, Args)]
pub struct LeaveArgs {
    /// The forum (default: --forum, then the current forum).
    #[arg(value_name = "FORUM")]
    pub forum: Option<String>,
}

/// `forum`.
#[derive(Debug, Args)]
pub struct ForumArgs {
    /// What to do.
    #[command(subcommand)]
    pub action: ForumAction,
}

/// `forum …`.
#[derive(Debug, Subcommand)]
pub enum ForumAction {
    /// List the followed forums, numbered.
    Ls,
    /// Show a forum's header: owner, mode, moderators, your role.
    Show {
        /// The forum (default: --forum, then the current forum).
        #[arg(value_name = "FORUM")]
        forum: Option<String>,
    },
    /// Follow a forum (local).
    Follow {
        /// The forum.
        #[arg(value_name = "FORUM")]
        forum: String,
        /// A short local name for it.
        #[arg(long, value_name = "ALIAS")]
        alias: Option<String>,
    },
    /// Stop following a forum (local).
    Unfollow {
        /// The forum (default: --forum, then the current forum).
        #[arg(value_name = "FORUM")]
        forum: Option<String>,
    },
    /// Make a forum the current one.
    Use {
        /// The forum.
        #[arg(value_name = "FORUM")]
        forum: String,
    },
    /// Create a forum owned by your account (the membership mode is always written).
    Create {
        /// The forum's slug (its record key, fixed).
        slug: String,
        /// Who may take part.
        #[arg(long, value_enum, default_value_t = ModeArg::Approval)]
        mode: ModeArg,
        /// The display name (default: the slug).
        #[arg(long)]
        name: Option<String>,
        /// A one-line description.
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
        /// The about text (markdown).
        #[arg(long, value_name = "MARKDOWN")]
        about: Option<String>,
        /// New posts start on the frontpage (default: personal).
        #[arg(long)]
        frontpage: bool,
    },
}

/// `members`.
#[derive(Debug, Args)]
pub struct MembersArgs {
    /// The forum (default: --forum, then the current forum).
    #[arg(value_name = "FORUM")]
    pub forum: Option<String>,
    /// Applicants only (a membership and no grant).
    #[arg(long)]
    pub pending: bool,
}

/// `approve`.
#[derive(Debug, Args)]
pub struct ApproveArgs {
    /// Handles or DIDs (in --forum, else the current forum).
    #[arg(value_name = "ACTOR", required = true, num_args = 1..)]
    pub actors: Vec<String>,
}

/// `draft`.
#[derive(Debug, Args)]
pub struct DraftArgs {
    /// What to do (default: list).
    #[command(subcommand)]
    pub action: Option<DraftAction>,
}

/// `draft …`.
#[derive(Debug, Subcommand)]
pub enum DraftAction {
    /// List the drafts.
    Ls,
    /// Edit a draft in $EDITOR.
    Edit {
        /// The draft id.
        id: String,
    },
    /// Delete a draft.
    Rm {
        /// The draft id.
        id: String,
    },
    /// Publish a draft.
    Publish {
        /// The draft id.
        id: String,
        /// Publish without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// `outbox`.
#[derive(Debug, Args)]
pub struct OutboxArgs {
    /// What to do (default: list).
    #[command(subcommand)]
    pub action: Option<OutboxAction>,
}

/// `outbox …`.
#[derive(Debug, Subcommand)]
pub enum OutboxAction {
    /// List the queued and recent writes.
    Ls,
    /// Send the queued writes of the account now.
    Flush,
    /// Queue a failed write again and send it.
    Retry {
        /// The write id.
        id: String,
    },
    /// Remove a write that was not sent yet.
    Undo {
        /// The write id.
        id: String,
    },
}

/// `redact`: delete an item's version history (`engine-api.md` §3.6 `redact_revision`, M4).
///
/// Deleting revision records cannot be undone, so `--yes` is required rather than assumed: a
/// command that destroys history should not be one flag away from a typo.
#[derive(Debug, Args)]
pub struct RedactArgs {
    /// The post or comment whose history goes.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// Confirm the deletion (without it the command only reports what it would delete).
    #[arg(long)]
    pub yes: bool,
}
