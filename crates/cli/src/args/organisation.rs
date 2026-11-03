// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search, organisation, cached user pages and sequence writes.

use clap::{Args, Subcommand};

/// `search <text>`.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Text to find in titles and rendered post/comment bodies.
    #[arg(value_name = "TEXT")]
    pub query: String,
    /// Restrict the search to one forum (default: all synced forums).
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
    /// Restrict results to this author (handle or DID).
    #[arg(long, value_name = "ACTOR")]
    pub author: Option<String>,
    /// Maximum results (at most 100).
    #[arg(long, default_value_t = 50)]
    pub limit: u32,
}

/// `user <actor>`.
#[derive(Debug, Args)]
pub struct UserArgs {
    /// Handle or DID.
    #[arg(value_name = "ACTOR")]
    pub actor: String,
    /// Also show role and karma in this forum.
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
}

/// `tag [slug]`.
#[derive(Debug, Args)]
pub struct TagArgs {
    /// A tag slug; without one, list the forum's tags.
    #[arg(value_name = "SLUG")]
    pub slug: Option<String>,
    /// The forum (default: the current forum).
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
}

/// `wiki edit <slug>`.
#[derive(Debug, Args)]
pub struct WikiArgs {
    /// What to do.
    #[command(subcommand)]
    pub action: WikiAction,
}

/// Wiki actions.
#[derive(Debug, Subcommand)]
pub enum WikiAction {
    /// Create a new immutable revision of a tag page.
    Edit {
        /// The tag/page slug.
        #[arg(value_name = "SLUG")]
        slug: String,
        /// Page name.
        #[arg(long)]
        name: String,
        /// Markdown source (`-` reads stdin).
        #[arg(long, default_value = "-")]
        file: String,
        /// Plain-text description.
        #[arg(long)]
        description: Option<String>,
        /// Change summary.
        #[arg(long)]
        summary: Option<String>,
        /// Whether this page defines a tag (default true).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        tag: bool,
        /// Parent tag slug (repeatable).
        #[arg(long = "parent-tag")]
        parent_tags: Vec<String>,
        /// The revision the text was edited from, as `tag <slug>` shows it (default: the page
        /// as this device last showed it). The edit is refused when the page has moved on.
        #[arg(long, value_name = "REVISION")]
        base: Option<String>,
        /// The forum (default: the current forum).
        #[arg(long, value_name = "FORUM")]
        forum: Option<String>,
    },
}

/// `sequence …`.
#[derive(Debug, Args)]
pub struct SequenceArgs {
    /// What to do.
    #[command(subcommand)]
    pub action: SequenceAction,
}

/// A chapter in a sequence JSON draft.
#[derive(Debug, Args)]
pub struct SequencePositionArgs {
    /// Zero-based chapter number.
    #[arg(long)]
    pub chapter: usize,
    /// Zero-based entry position within the chapter.
    #[arg(long)]
    pub index: usize,
}

/// Sequence library and write actions.
#[derive(Debug, Subcommand)]
pub enum SequenceAction {
    /// List a forum's sequences.
    Ls {
        /// The forum (default: the current forum).
        #[arg(long, value_name = "FORUM")]
        forum: Option<String>,
    },
    /// Read a sequence.
    Read {
        /// Sequence at-uri.
        #[arg(value_name = "URI")]
        uri: String,
    },
    /// Create a sequence from a JSON `SequenceDraft` (`-` reads stdin).
    Create {
        /// Draft JSON.
        #[arg(value_name = "FILE", default_value = "-")]
        file: String,
    },
    /// Replace the editable fields of your sequence from a JSON `SequenceDraft`.
    Edit {
        /// Sequence at-uri.
        #[arg(value_name = "URI")]
        uri: String,
        /// Draft JSON (`-` reads stdin).
        #[arg(value_name = "FILE", default_value = "-")]
        file: String,
    },
    /// Insert a post in a sequence chapter.
    Add {
        /// Sequence at-uri.
        #[arg(value_name = "URI")]
        uri: String,
        /// Post at-uri.
        #[arg(value_name = "POST")]
        post: String,
        #[command(flatten)]
        at: SequencePositionArgs,
    },
    /// Remove a post from every chapter of a sequence.
    Remove {
        /// Sequence at-uri.
        #[arg(value_name = "URI")]
        uri: String,
        /// Post at-uri.
        #[arg(value_name = "POST")]
        post: String,
    },
    /// Move a chapter-local entry.
    Move {
        /// Sequence at-uri.
        #[arg(value_name = "URI")]
        uri: String,
        #[command(flatten)]
        from: SequencePositionArgs,
        /// Destination chapter.
        #[arg(long)]
        to_chapter: usize,
        /// Destination position within that chapter.
        #[arg(long)]
        to_index: usize,
    },
}
