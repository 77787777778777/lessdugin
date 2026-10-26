// SPDX-License-Identifier: AGPL-3.0-or-later
//! Arguments of the M3 social commands and M4 `export` (`docs/design/synthesis-v1.md` §6.10): opinions
//! (`vote`, `agree`, `react`), `notifications`, `activity`, `mods`, `mod`.
//!
//! References are the ones `refs` resolves (`3`, `c12`, an at-uri, an SPA link,
//! `<handle>/<rkey>`); actor references are a handle or a DID.

use clap::{Args, Subcommand, ValueEnum};
use lessdugin_core::dto::{AgreementValue, KarmaValue, ModRole, ModScope, Stance};

/// The value of `vote` (the karma axis).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum KarmaArg {
    /// A normal upvote.
    Up,
    /// A normal downvote.
    Down,
    /// A strong upvote (more karma power).
    StrongUp,
    /// A strong downvote.
    StrongDown,
    /// Remove your karma vote.
    Clear,
}

impl From<KarmaArg> for Option<KarmaValue> {
    fn from(k: KarmaArg) -> Self {
        match k {
            KarmaArg::Up => Some(KarmaValue::Up),
            KarmaArg::Down => Some(KarmaValue::Down),
            KarmaArg::StrongUp => Some(KarmaValue::StrongUp),
            KarmaArg::StrongDown => Some(KarmaValue::StrongDown),
            KarmaArg::Clear => None,
        }
    }
}

/// The value of `agree` (the agreement axis).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum AgreementArg {
    /// Normal agreement.
    Agree,
    /// Normal disagreement.
    Disagree,
    /// Strong agreement.
    StrongAgree,
    /// Strong disagreement.
    StrongDisagree,
    /// Remove your agreement vote.
    Clear,
}

impl From<AgreementArg> for Option<AgreementValue> {
    fn from(a: AgreementArg) -> Self {
        match a {
            AgreementArg::Agree => Some(AgreementValue::Agree),
            AgreementArg::Disagree => Some(AgreementValue::Disagree),
            AgreementArg::StrongAgree => Some(AgreementValue::StrongAgree),
            AgreementArg::StrongDisagree => Some(AgreementValue::StrongDisagree),
            AgreementArg::Clear => None,
        }
    }
}

/// `vote`.
#[derive(Debug, Args)]
pub struct VoteArgs {
    /// The post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// The vote.
    #[arg(value_enum)]
    pub value: KarmaArg,
}

/// `agree`.
#[derive(Debug, Args)]
pub struct AgreeArgs {
    /// The post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// The agreement vote.
    #[arg(value_enum)]
    pub value: AgreementArg,
}

/// `react`.
#[derive(Debug, Args)]
pub struct ReactArgs {
    /// The post or comment.
    #[arg(value_name = "REF")]
    pub reference: String,
    /// A react of the forum's palette (`lessdugin reacts` lists the default one).
    #[arg(value_name = "REACT")]
    pub react: String,
    /// Anchor the react to this quoted passage of the item's text.
    #[arg(long, value_name = "TEXT")]
    pub quote: Option<String>,
    /// React against (oppose) instead of for.
    #[arg(long, conflicts_with = "remove")]
    pub against: bool,
    /// Remove your react (with the same quote, if it was anchored).
    #[arg(long)]
    pub remove: bool,
}

/// `notifications`.
#[derive(Debug, Args)]
pub struct NotificationsArgs {
    /// Show read notifications too.
    #[arg(long)]
    pub all: bool,
    /// Mark every shown notification read.
    #[arg(long)]
    pub mark_read: bool,
    /// At most this many notifications (1–200).
    #[arg(long, short = 'n', default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=200))]
    pub limit: u32,
}

/// `activity`.
#[derive(Debug, Args)]
pub struct ActivityArgs {
    /// The forum (default: --forum, then the current forum).
    #[arg(value_name = "FORUM")]
    pub forum: Option<String>,
    /// At most this many entries (1–100).
    #[arg(long, short = 'n', default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
}

/// `mods`.
#[derive(Debug, Args)]
pub struct ModsArgs {
    /// What to do.
    #[command(subcommand)]
    pub action: ModsAction,
}

/// `mods …` (`social.md` §4.3; a removal runs `mod adopt` first, `data-model.md` §8.6).
#[derive(Debug, Subcommand)]
pub enum ModsAction {
    /// Add a moderator or admin, or change their role.
    Add {
        /// A handle or DID.
        #[arg(value_name = "ACTOR")]
        actor: String,
        /// The role.
        #[arg(long, value_enum, default_value_t = ModRoleArg::Moderator)]
        role: ModRoleArg,
    },
    /// Remove a moderator (their assertions are adopted first).
    Rm {
        /// A handle or DID.
        #[arg(value_name = "ACTOR")]
        actor: String,
        /// Remove without adopting (a rogue moderator: their assertions lapse).
        #[arg(long)]
        skip_adopt: bool,
    },
}

/// `--role` of `mods add`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ModRoleArg {
    /// A moderator (rank 1).
    #[default]
    Moderator,
    /// An admin (rank 2).
    Admin,
}

impl From<ModRoleArg> for ModRole {
    fn from(r: ModRoleArg) -> Self {
        match r {
            ModRoleArg::Moderator => ModRole::Moderator,
            ModRoleArg::Admin => ModRole::Admin,
        }
    }
}

/// `mod`.
#[derive(Debug, Args)]
pub struct ModArgs {
    /// What to do.
    #[command(subcommand)]
    pub action: ModAction,
}

/// `mod …` (`social.md` §4; every action is a public record, `data-model.md` §6.7).
#[derive(Debug, Subcommand)]
pub enum ModAction {
    /// Hide an item (with an optional reason; spam hides it everywhere).
    Hide(ModCommon),
    /// Remove your hide assertion.
    Unhide(ModSubject),
    /// Keep an item visible despite a lower rank hiding it.
    Keep(ModCommon),
    /// Ban an actor from the forum (hides all of their content).
    Ban(ModCommon),
    /// Remove your ban.
    Unban(ModSubject),
    /// Approve an applicant (the grant; same as `approve`).
    Approve(ModSubject),
    /// Restrict an actor's capabilities.
    Restrict(RestrictArgs),
    /// Remove your restriction (also lifts a nullification).
    Unrestrict(ModSubject),
    /// Nullify an actor's votes (a restrict with `nullifyVotes`; their opinions stop counting).
    Nullify(ModCommon),
    /// Move your own item to the personal tier.
    Personal(ModSubject),
    /// Move an item to the frontpage tier.
    Frontpage(ModSubject),
    /// Move an item to the curated tier (admins and the owner).
    Curate(ModSubject),
    /// Pin an item.
    Pin(PinArgs),
    /// Unpin an item.
    Unpin(PinArgs),
    /// Lock a post (comments stay only for its allow-list) or the forum (read-only).
    Lock(ModSubject),
    /// Unlock.
    Unlock(ModSubject),
    /// The queue for moderators: applicants, tag conflicts, stale assertions, newcomers.
    Queue {
        /// The forum (default: --forum, then the current forum).
        #[arg(value_name = "FORUM")]
        forum: Option<String>,
    },
    /// The public log of every moderation action.
    Log {
        /// The forum (default: --forum, then the current forum).
        #[arg(value_name = "FORUM")]
        forum: Option<String>,
        /// At most this many entries (1–200).
        #[arg(long, short = 'n', default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: u32,
    },
    /// Re-issue a former moderator's assertions under yourself.
    Adopt {
        /// The former moderator (handle or DID).
        #[arg(value_name = "ACTOR")]
        actor: String,
        /// The forum (default: --forum, then the current forum).
        #[arg(long, value_name = "FORUM")]
        forum: Option<String>,
    },
}

/// The subject of a mod action: an at-uri or ref (an item), a handle/DID (an actor) or
/// `tag:<slug>`.
#[derive(Debug, Args)]
pub struct ModSubject {
    /// The item, actor or `tag:<slug>`.
    #[arg(value_name = "SUBJECT")]
    pub subject: String,
    /// The forum (default: --forum, then the current forum, else the subject's forum).
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
}

/// A mod action with the common options.
#[derive(Debug, Args)]
pub struct ModCommon {
    /// The item, actor or `tag:<slug>`.
    #[arg(value_name = "SUBJECT")]
    pub subject: String,
    /// The forum (default: --forum, then the current forum, else the subject's forum).
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
    /// Why (a public record; shown according to the forum's policy).
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// Hide as spam (`mod hide` only: spam hides the author's content forum-wide).
    #[arg(long)]
    pub spam: bool,
    /// Act as the post's author, not as the forum (author moderation, `social.md` §4.9).
    #[arg(long)]
    pub author_scope: bool,
    /// The action stops applying at this time (RFC 3339, e.g. 2026-11-01T00:00:00Z).
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,
}

/// `mod restrict`.
#[derive(Debug, Args)]
pub struct RestrictArgs {
    /// The actor (handle or DID).
    #[arg(value_name = "ACTOR")]
    pub subject: String,
    /// The forum (default: --forum, then the current forum).
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
    /// The capabilities to take away (repeatable): post, comment, vote, react, tag, wiki,
    /// sequence. Without one, every capability is restricted.
    #[arg(long = "cap", value_enum)]
    pub capabilities: Vec<CapabilityArg>,
    /// Why (public).
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
    /// Act as the post's author (author moderation).
    #[arg(long)]
    pub author_scope: bool,
    /// The restriction ends at this time (RFC 3339).
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,
}

/// A restrictable capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CapabilityArg {
    /// Writing posts.
    Post,
    /// Writing comments.
    Comment,
    /// Karma and agreement votes.
    Vote,
    /// Reacts.
    React,
    /// Tag votes.
    Tag,
    /// Wiki edits.
    Wiki,
    /// Sequence edits.
    Sequence,
}

impl From<CapabilityArg> for lessdugin_core::dto::Capability {
    fn from(c: CapabilityArg) -> Self {
        use lessdugin_core::dto::Capability as C;
        match c {
            CapabilityArg::Post => C::Post,
            CapabilityArg::Comment => C::Comment,
            CapabilityArg::Vote => C::Vote,
            CapabilityArg::React => C::React,
            CapabilityArg::Tag => C::Tag,
            CapabilityArg::Wiki => C::Wiki,
            CapabilityArg::Sequence => C::Sequence,
        }
    }
}

/// `mod pin` / `mod unpin`.
#[derive(Debug, Args)]
pub struct PinArgs {
    /// The item.
    #[arg(value_name = "SUBJECT")]
    pub subject: String,
    /// The forum (default: --forum, then the current forum, else the subject's forum).
    #[arg(long, value_name = "FORUM")]
    pub forum: Option<String>,
    /// Pin to the whole forum instead of its thread.
    #[arg(long, value_enum, default_value_t = PinScopeArg::Thread)]
    pub scope: PinScopeArg,
    /// Why (public).
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
}

/// `--scope` of `mod pin`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum PinScopeArg {
    /// The item's thread.
    #[default]
    Thread,
    /// The whole forum.
    Forum,
}

impl From<PinScopeArg> for lessdugin_core::dto::PinScope {
    fn from(s: PinScopeArg) -> Self {
        match s {
            PinScopeArg::Thread => lessdugin_core::dto::PinScope::Thread,
            PinScopeArg::Forum => lessdugin_core::dto::PinScope::Forum,
        }
    }
}

/// `--scope` of the mod intents: forum tier or the author's tier.
pub fn scope_of(author_scope: bool) -> ModScope {
    if author_scope {
        ModScope::Author
    } else {
        ModScope::Forum
    }
}

/// The `against` flag as a stance.
pub fn stance_of(against: bool) -> Stance {
    if against {
        Stance::Against
    } else {
        Stance::For
    }
}

/// Which local state `export` covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ExportScopeArg {
    /// Your own state (the default, `threat-model.md` PRIV-8).
    #[default]
    Mine,
    /// Every account on this device.
    Everything,
}

/// `export`: write this device's local state as JSON.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// What to export.
    #[arg(long, value_enum, default_value_t = ExportScopeArg::Mine)]
    pub scope: ExportScopeArg,
    /// Write to this file instead of stdout.
    #[arg(long, short = 'o', value_name = "PATH")]
    pub out: Option<std::path::PathBuf>,
    /// Import instead: merge a file written by `export` back into this device.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["scope", "out"])]
    pub import: Option<std::path::PathBuf>,
}
