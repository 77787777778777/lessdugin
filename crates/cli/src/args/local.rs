// SPDX-License-Identifier: AGPL-3.0-or-later
//! Arguments of the local-state commands (`engine-api.md` §3.6, `docs/design/synthesis-v1.md`
//! §6.10): `bookmark`, `subscribe`, `unsubscribe`, `subs` and `config`. Nothing here writes to
//! a repository: bookmarks, subscriptions and settings live in this device's `state` store.

use clap::{Args, Subcommand};

/// `bookmark`.
#[derive(Debug, Args)]
pub struct BookmarkArgs {
    /// What to do (default: list).
    #[command(subcommand)]
    pub action: Option<BookmarkAction>,
}

/// `bookmark …` (local).
#[derive(Debug, Subcommand)]
pub enum BookmarkAction {
    /// Bookmark a post.
    Add {
        /// The post.
        #[arg(value_name = "REF")]
        reference: String,
    },
    /// Remove a bookmark.
    Rm {
        /// The post.
        #[arg(value_name = "REF")]
        reference: String,
    },
    /// List the bookmarks, newest first.
    Ls,
}

/// `subscribe` / `unsubscribe`.
#[derive(Debug, Args)]
pub struct SubscribeArgs {
    /// What to subscribe to.
    #[command(subcommand)]
    pub target: SubscribeTarget,
}

/// What a subscription names (`social.md` §16): its new items become notifications.
#[derive(Debug, Subcommand)]
pub enum SubscribeTarget {
    /// New comments on a post.
    Post {
        /// The post.
        #[arg(value_name = "REF")]
        reference: String,
    },
    /// New posts in a forum.
    Forum {
        /// The forum (default: --forum, then the current forum).
        #[arg(value_name = "FORUM")]
        forum: Option<String>,
    },
    /// New posts by a user.
    User {
        /// A handle or DID.
        #[arg(value_name = "ACTOR")]
        actor: String,
    },
    /// New posts with a tag, in a forum.
    Tag {
        /// The tag's slug.
        #[arg(value_name = "TAG")]
        slug: String,
        /// The forum (default: --forum, then the current forum).
        #[arg(long, value_name = "FORUM")]
        forum: Option<String>,
    },
}

/// `config`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// What to do.
    #[command(subcommand)]
    pub action: ConfigAction,
}

/// `config …`: this device's settings (`engine-api.md` §3.6 `settings`, `set_setting`).
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Show the settings in effect, or one of them.
    Get {
        /// A setting (sources, verify, indexers, externalImagesForums, mediaAutoloadHosts,
        /// karmaNotifications, acknowledgedPublicActions).
        #[arg(value_name = "KEY")]
        key: Option<String>,
    },
    /// Set a setting, or return it to its default.
    Set {
        /// The setting (a key `config get` shows, or mediaAutoload, externalImages,
        /// mediaHosts).
        #[arg(value_name = "KEY")]
        key: String,
        /// The value: a word (`direct`, `off`, `weekly`), a comma-separated list for a list
        /// setting (`a.example,b.example`) or a JSON list. Without one, the setting returns to
        /// its default.
        #[arg(value_name = "VALUE")]
        value: Option<String>,
    },
}
