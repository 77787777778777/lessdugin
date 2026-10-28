// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `:` command line (`synthesis-v1.md` §6.11 Overlays: "`:` command line (accepts any CLI
//! subcommand)").
//!
//! The CLI is a separate crate, so the TUI does not call it. Instead this is a thin parser
//! over the M3 verbs that maps each one onto the same `engine.*` calls the keys already make
//! (`docs/plan.md` §M3: "every MVP action is reachable in all three clients (in the TUI at
//! least through the `:` command line)"). The verbs mirror the CLI's names where they exist:
//!
//! | Command | Engine call | Same as key |
//! |---|---|---|
//! | `vote up\|down\|strong-up\|strong-down\|clear` | `opinion` (`Karma`) | `u`/`U`/`d`/`D`/`0` |
//! | `agree for\|against\|strong-for\|strong-against\|clear` | `opinion` (`Agreement`) | `a`/`A`/`z`/`Z`/`0` |
//! | `react <id> [for\|against]` | `opinion` (`React`) | `x`, then Enter |
//! | `unreact <id>` | `opinion` (`React`, remove) | `x`, then Enter again |
//! | `mod <action> [subject] [reason] [--until WHEN] [--author]` | `mod_action` | `m` |
//! | `notifications [all]`, `read`, `read-all` | `notifications`, `mark_notifications_read`, `mark_all_notifications_read` | `N`, `a`, `M` |
//! | `activity` | `recent_discussion` | `Tab` on a forum |
//! | `sort <sort>`, `filter <filter>` | `list_posts`, `get_thread` | `s`, `f` |
//! | `user [actor]` | the forum view's `moderators` | - |
//! | `mods [role]` | the forum view's `moderators` | - |
//! | `mods add <actor> [admin|moderator]`, `mods remove <actor>` | `mods_set` | - |
//! | `open <ref>`, `follow [forum]`, `unfollow [forum]` | `forum_open`, `forum_follow`, `forum_unfollow` | `Enter` |
//! | `join [note]`, `leave` | `join`, `leave` | - |
//! | `members [pending\|role]` | `members` | - |
//! | `grant`, `revoke`, `approve`, `reject <actor...>` | the same | `Enter` on an applicant |
//! | `mod adopt <actor>` | `mod_adopt` | - |
//! | `forum create <slug> <name> [--open]`, `forum edit [--name ...]` | `forum_create`, `forum_update` | - |
//! | `edit`, `delete`, `retract`, `redact`, `raw` `[subject]` | the same, `redact_revision` | - |
//! | `bookmark`, `subscribe`, `mute <actor>` (and `un...`) | `bookmark`, `subscribe`, `mute` | - |
//! | `karma-notifications [cadence]`, `settings`, `set <key> <value>`, `unset <key>` | the same | - |
//! | `undo`, `retry [write]` | `undo`, `retry_write` | `Backspace` |
//!
//! [`REACH`] says how the TUI reaches each operation of `engine-api.md` §3.2, §3.5 and §3.6;
//! a test holds it to the spec, so an operation cannot go missing. `parse` is pure (no engine,
//! no state); `State::command` runs it.

use lessdugin_core::dto::{
    AccessLevel, AgreementValue, Capability, CommentSort, DefaultTier, ForumRole, KarmaCadence,
    KarmaValue, MembershipMode, ModActionIntent, ModActionValue, ModRole, ModScope, PinScope,
    PostFilter, PostSort, Tier,
};

/// What the `:` command line was asked to do, once parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verb {
    /// `vote up|down|strong-up|strong-down|clear`: the karma axis.
    Vote(Vote),
    /// `agree for|against|strong-for|strong-against|clear`: the agreement axis.
    Agree(Agree),
    /// `react <id> [for|against] [subject] [--quote <text>]`: one react from the `default/1`
    /// palette, on the whole item or anchored to a passage of it.
    React {
        /// The palette id (`default/1`).
        id: String,
        /// Against, when the verb said so.
        against: bool,
        /// The quoted passage (`quote.exact`; the engine derives the rest).
        quote: Option<String>,
    },
    /// `unreact <id>`: take a react back.
    Unreact {
        /// The palette id.
        id: String,
    },
    /// `mod <action> [subject...]`: a forum-tier moderation action.
    Mod(ModVerb),
    /// `notifications [all]`: the Notifications screen, unread-only or all.
    Notifications {
        /// List every notification, not just the unread ones.
        all: bool,
    },
    /// `read`: mark the selected notification read.
    Read,
    /// `read-all`: mark every notification read.
    ReadAll,
    /// `activity`: the forum's Recent Discussion tab.
    Activity,
    /// `sort <sort>`: a forum's post sort or a thread's comment sort (`s`).
    Sort(SortName),
    /// `filter frontpage|all|curated`: a forum's post filter (`f`).
    Filter(PostFilter),
    /// `user [actor]`: who an actor is and what they posted here.
    User {
        /// The actor, when named (else the selected item's author).
        actor: Option<String>,
    },
    /// `mods [role]`: lists the forum's declared moderators.
    ModsList {
        /// The role filter, when named (`owner`, `admin`, `moderator`).
        role: Option<String>,
    },
    /// `mods add <actor> [admin|moderator]`, `mods remove <actor>`: `mods_set`.
    ModsSet {
        /// The DID or handle.
        subject: String,
        /// The role it takes (ignored when removing).
        role: ModRole,
        /// Remove instead of add.
        remove: bool,
    },
    /// `open <ref>`: a forum, post or comment by any reference `parse_ref` takes, or a forum
    /// as `owner/slug`.
    Open {
        /// The reference.
        target: String,
    },
    /// `follow [forum]`, `unfollow [forum]` (the forum on screen when none is named).
    Follow {
        /// The forum, when named.
        forum: Option<String>,
        /// Follow, or stop following.
        on: bool,
    },
    /// `join [note]`: join the forum on screen, or apply to it (approval mode).
    Join {
        /// The membership note.
        note: Option<String>,
    },
    /// `leave`: delete the membership.
    Leave,
    /// `members [pending|owner|admin|moderator|member|established|newcomer|none]`.
    Members {
        /// The role kept, when named.
        status: Option<ForumRole>,
        /// The applicants only.
        pending: bool,
    },
    /// `grant|revoke|approve|reject <actor...>`.
    Access {
        /// Which.
        op: AccessOp,
        /// The DIDs or handles.
        actors: Vec<String>,
    },
    /// `mod adopt <actor>`: re-issue a former moderator's assertions.
    Adopt {
        /// The former moderator.
        former: String,
    },
    /// `forum create <slug> <name> [--open]`.
    ForumCreate {
        /// The rkey.
        slug: String,
        /// The name.
        name: String,
        /// Open mode (approval otherwise).
        open: bool,
    },
    /// `forum edit [--name N] [--description D] [--mode open|approval] [--tier T]
    /// [--report-contact C]`: the forum on screen; what is not named is kept.
    ForumEdit(ForumChanges),
    /// `edit [subject]`: your post or comment, in `$EDITOR`.
    Edit,
    /// `delete [subject]`: your post or comment, with its revisions (asks first).
    Delete,
    /// `retract [subject]`: your comment stays, marked retracted.
    Retract,
    /// `redact [subject]`: the version history of your post or comment (asks first).
    Redact,
    /// `raw [subject]`: the stored record, its validity and the mod actions naming it.
    Raw,
    /// `bookmark [subject]`, `unbookmark [subject]`.
    Bookmark {
        /// Bookmark, or remove the bookmark.
        on: bool,
    },
    /// `subscribe [post|forum|user <actor>|tag <slug>]`, `unsubscribe ...`.
    Subscribe {
        /// What.
        target: SubTarget,
        /// Subscribe, or stop.
        on: bool,
    },
    /// `mute <actor>`, `unmute <actor>` (the selected item's author when none is named).
    Mute {
        /// The DID or handle, when named.
        actor: Option<String>,
        /// Mute, or unmute.
        on: bool,
    },
    /// `karma-notifications [realtime|daily|weekly]`: show or set the cadence.
    KarmaNotifications(Option<KarmaCadence>),
    /// `settings`: the device's settings; `set <key> <value>`, `unset <key>`.
    Settings {
        /// The key set or reset.
        key: Option<String>,
        /// Its value (`None` with a key: back to the default).
        value: Option<String>,
    },
    /// `undo`: the last opinion still in its undo window (`Backspace`).
    Undo,
    /// `retry [write id]`: a failed write again (the only one when none is named).
    Retry {
        /// The write id.
        id: Option<String>,
    },
}

/// What `grant`, `revoke`, `approve` and `reject` do (`engine-api.md` §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessOp {
    /// `#access{member}` by the active account.
    Grant,
    /// Its removal.
    Revoke,
    /// A grant of an applicant, by an authority (approval mode).
    Approve,
    /// Dismiss applicants on this device.
    Reject,
}

/// What `subscribe` names (`social.md` §16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubTarget {
    /// New comments on a post: the one selected, or named.
    Post(Option<String>),
    /// New posts in the forum on screen.
    Forum,
    /// New posts by a user.
    User(String),
    /// New posts with a tag, in the forum on screen.
    Tag(String),
}

/// The fields `forum edit` changes; the others keep the declaration's values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForumChanges {
    /// `--name`.
    pub name: Option<String>,
    /// `--description` (empty: removed).
    pub description: Option<String>,
    /// `--mode open|approval`.
    pub membership: Option<MembershipMode>,
    /// `--tier personal|frontpage`.
    pub default_tier: Option<DefaultTier>,
    /// `--report-contact` (empty: removed).
    pub report_contact: Option<String>,
}

/// How the TUI reaches an engine operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// These keys, in turn, on the Thread screen of a post with a comment, the comment
    /// selected.
    Keys(&'static [&'static str]),
    /// This command line, on that screen.
    Command(&'static str),
    /// No user action: the TUI calls it by itself, as the text says.
    Automatic(&'static str),
    /// Not in the TUI, for the reason given (a later milestone, or another client's job).
    NotHere(&'static str),
}

/// How the TUI reaches each operation of `engine-api.md` §3.2 (forums and membership), §3.5
/// (writes) and §3.6 (local state): `docs/plan.md` §M3, "every MVP action is reachable in all
/// three clients (in the TUI at least through the `:` command line)".
pub const REACH: &[(&str, Reach)] = &[
    // §3.2
    ("forum_open", Reach::Command("open alice.example.com/meta")),
    ("forum_discover", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("forum_follow", Reach::Command("follow")),
    ("forum_unfollow", Reach::Command("unfollow")),
    (
        "followed",
        Reach::Automatic("the Home screen lists the followed forums"),
    ),
    (
        "forum_create",
        Reach::Command("forum create tui-made A forum made here"),
    ),
    (
        "forum_update",
        Reach::Command("forum edit --name Meta, renamed"),
    ),
    ("join", Reach::Command("join hello")),
    ("leave", Reach::Command("leave")),
    ("members", Reach::Command("members")),
    (
        "grant",
        Reach::Command("grant did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
    ),
    (
        "revoke",
        Reach::Command("revoke did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
    ),
    (
        "approve",
        Reach::Command("approve did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
    ),
    (
        "reject",
        Reach::Command("reject did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
    ),
    (
        "rejected_applicants",
        Reach::Automatic("`members pending` leaves out the applicants rejected here"),
    ),
    (
        "mods_set",
        Reach::Command("mods add did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
    ),
    // §3.5
    ("publish", Reach::Keys(&["P"])),
    ("comment", Reach::Keys(&["r"])),
    ("edit", Reach::Command("edit")),
    ("delete", Reach::Command("delete")),
    ("retract", Reach::Command("retract")),
    ("opinion", Reach::Keys(&["u"])),
    ("mod_action", Reach::Command("mod hide")),
    (
        "mod_adopt",
        Reach::Command("mod adopt did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
    ),
    ("wiki_edit", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("seq_create", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("seq_edit", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("seq_add", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("seq_remove", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("seq_move", Reach::NotHere("M5 (engine-api.md §3.9)")),
    ("redact_revision", Reach::Command("redact")),
    (
        "prepare_media",
        Reach::NotHere("attachments are added in the CLI and the SPA; the TUI writes text"),
    ),
    ("undo", Reach::Command("undo")),
    ("retry_write", Reach::Command("retry 1")),
    (
        "write_status",
        Reach::Automatic("write events update the header's queued count"),
    ),
    (
        "writes",
        Reach::Automatic("the outbox of earlier sessions is loaded at start"),
    ),
    // §3.6
    (
        "mark_read",
        Reach::Automatic("opening a thread marks it read"),
    ),
    ("mark_notifications_read", Reach::Command("read")),
    ("mark_all_notifications_read", Reach::Command("read-all")),
    ("bookmark", Reach::Command("bookmark")),
    ("subscribe", Reach::Command("subscribe")),
    ("mute", Reach::Command("mute")),
    (
        "acknowledged_public_actions",
        Reach::Automatic("read at start for the public-action interstitial"),
    ),
    (
        "public_action_acknowledged",
        Reach::Automatic("the interstitial asks acknowledged_public_actions for every kind"),
    ),
    (
        "acknowledge_public_action",
        Reach::Automatic("going past the interstitial (`y`) records it"),
    ),
    ("karma_notifications", Reach::Command("karma-notifications")),
    (
        "set_karma_notifications",
        Reach::Command("karma-notifications weekly"),
    ),
    (
        "save_draft",
        Reach::NotHere(
            "the TUI keeps unsent text for the next editor session; drafts are the CLI's and the SPA's",
        ),
    ),
    (
        "delete_draft",
        Reach::NotHere(
            "the TUI keeps unsent text for the next editor session; drafts are the CLI's and the SPA's",
        ),
    ),
    ("settings", Reach::Command("settings")),
    ("set_setting", Reach::Command("set mediaAutoload off")),
    ("export_local", Reach::NotHere("a file: `lessdugin export`")),
    (
        "import_local",
        Reach::NotHere("a file: `lessdugin export --import`"),
    ),
];

/// A sort `sort` names: a post sort (`social.md` §12.2) or a comment sort (§12.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortName {
    /// `magic` (both).
    Magic,
    /// `top` (both).
    Top,
    /// `new` (both).
    New,
    /// `old` (both).
    Old,
    /// `recent-comments` (posts).
    RecentComments,
    /// `latest-reply` (comments).
    LatestReply,
}

impl SortName {
    /// The post sort, if it is one.
    pub fn post(self) -> Option<PostSort> {
        Some(match self {
            SortName::Magic => PostSort::Magic,
            SortName::Top => PostSort::Top,
            SortName::New => PostSort::New,
            SortName::Old => PostSort::Old,
            SortName::RecentComments => PostSort::RecentComments,
            SortName::LatestReply => return None,
        })
    }

    /// The comment sort, if it is one.
    pub fn comment(self) -> Option<CommentSort> {
        Some(match self {
            SortName::Magic => CommentSort::Magic,
            SortName::Top => CommentSort::Top,
            SortName::New => CommentSort::New,
            SortName::Old => CommentSort::Old,
            SortName::LatestReply => CommentSort::LatestReply,
            SortName::RecentComments => return None,
        })
    }
}

/// The karma axis's values (`vote`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vote {
    /// `up`.
    Up,
    /// `strong-up`.
    StrongUp,
    /// `down`.
    Down,
    /// `strong-down`.
    StrongDown,
    /// `clear`.
    Clear,
}

/// The agreement axis's values (`agree`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agree {
    /// `for`.
    For,
    /// `strong-for`.
    StrongFor,
    /// `against`.
    Against,
    /// `strong-against`.
    StrongAgainst,
    /// `clear`.
    Clear,
}

/// A moderation action (`mod ...`), each of which becomes a [`ModActionIntent`]. The verbs and
/// their meaning are the CLI's (`lessdugin mod ...`): `keep`, `unlock` and `unpin` assert the
/// opposite value (how a higher rank overrides another moderator, `social.md` §4.2), while
/// `unhide`, `unban` and `unrestrict` remove the account's own assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModVerb {
    /// `hide [reason]` (`--spam`: as spam).
    Hide,
    /// `spam [reason]`.
    Spam,
    /// `unhide`: removes your hide.
    Unhide,
    /// `keep [reason]`: keeps the item visible over a lower rank's hide (`#hide{hidden: false}`).
    Keep,
    /// `ban [reason]`.
    Ban,
    /// `unban`: removes your ban.
    Unban,
    /// `pin`: in the forum's lists (a post), or in its thread (a comment).
    Pin,
    /// `unpin`: asserts `pinned: false`.
    Unpin,
    /// `lock`.
    Lock,
    /// `unlock`: asserts `locked: false` (it overrides a lower rank's lock).
    Unlock,
    /// `tier personal|frontpage|curated [reason]`.
    Tier(Tier),
    /// `restrict [cap,cap...] [reason]`, `--cap <cap>`: the capabilities taken away (every one
    /// when none is named, as the CLI). A nullification the account already asserted stays.
    Restrict(Vec<Capability>),
    /// `unrestrict`: removes your restriction (and nullification).
    Unrestrict,
    /// `nullify [reason]`: the actor's votes stop counting; a restriction stays.
    Nullify,
    /// `unnullify`: the actor's votes count again; a restriction stays.
    Unnullify,
    /// `approve`: grants a membership (`engine.grant`).
    Approve,
}

/// What a [`ModVerb`] acts on when the command line names no subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModTarget {
    /// The selected item (a post or a comment).
    Item,
    /// The selected item's thread post (a lock, a tier).
    Post,
    /// The selected item's author (a ban, a restriction, an approval).
    Author,
}

impl ModVerb {
    /// What the verb acts on by default.
    pub fn target(&self) -> ModTarget {
        match self {
            ModVerb::Ban
            | ModVerb::Unban
            | ModVerb::Restrict(_)
            | ModVerb::Unrestrict
            | ModVerb::Nullify
            | ModVerb::Unnullify
            | ModVerb::Approve => ModTarget::Author,
            ModVerb::Lock | ModVerb::Unlock | ModVerb::Tier(_) => ModTarget::Post,
            ModVerb::Hide | ModVerb::Spam | ModVerb::Unhide | ModVerb::Keep => ModTarget::Item,
            ModVerb::Pin | ModVerb::Unpin => ModTarget::Item,
        }
    }
}

/// How a `#restrict` assertion is completed before it is sent. A record states the complete
/// state of its key (`social.md` §4.2); the engine keeps the dimension an assertion leaves out
/// (`engine-api.md` §3.5), so a nullification never lifts a restriction, nor a restriction a
/// nullification. Adding to the capabilities restricted already is the TUI's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestrictDim {
    /// Capabilities added to the ones the record restricts already (the mod menu's presets:
    /// "voting" after "posting" restricts both).
    AddCapabilities,
}

/// The options of a `mod` command line (the CLI's flags).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModOpts {
    /// `--until <RFC 3339>`: the assertion lapses then.
    pub until_ms: Option<i64>,
    /// `--author` (`--author-scope`): act as the post's author (`social.md` §4.9).
    pub author_scope: bool,
}

/// One parsed command line: a verb, its optional explicit subject, and an optional reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// What to do.
    pub verb: Verb,
    /// A subject named on the command line (a uri, a DID or a handle). `None`: the selection.
    pub subject: Option<String>,
    /// The reason (a moderation action's `reason`, or the `user`/`mods` selector).
    pub reason: Option<String>,
    /// A `mod` command's options.
    pub opts: ModOpts,
}

/// What a command line said, or why it made no sense.
pub type Parsed = Result<Command, String>;

/// Parses a command line (without its leading `:`).
///
/// # Errors
/// Returns the usage message for an unknown verb or an unusable argument.
pub fn parse(line: &str) -> Parsed {
    let mut words = line.split_whitespace();
    let Some(verb) = words.next() else {
        return Err(usage());
    };
    let rest: Vec<&str> = words.collect();
    match verb {
        "vote" => Ok(Command {
            verb: Verb::Vote(vote(rest.first().copied())?),
            subject: rest.get(1).map(|s| (*s).to_owned()),
            reason: None,
            opts: ModOpts::default(),
        }),
        "agree" => Ok(Command {
            verb: Verb::Agree(agree(rest.first().copied())?),
            subject: rest.get(1).map(|s| (*s).to_owned()),
            reason: None,
            opts: ModOpts::default(),
        }),
        "react" => {
            // `--quote` takes the rest of the line: the passage, as it reads
            let (rest, quote) = match rest.iter().position(|w| *w == "--quote") {
                Some(i) => {
                    let q = trim_reason(&rest[i + 1..].join(" "));
                    if q.is_none() {
                        return Err("--quote needs the passage it anchors to.".into());
                    }
                    (rest[..i].to_vec(), q)
                }
                None => (rest, None),
            };
            let Some(id) = rest.first() else {
                return Err("react needs a react id (try `react heart`).".into());
            };
            let against = match rest.get(1).copied() {
                None | Some("for") => false,
                Some("against") => true,
                Some(other) => {
                    return Err(format!("react takes `for` or `against`, not `{other}`."));
                }
            };
            Ok(Command {
                verb: Verb::React {
                    id: (*id).to_owned(),
                    against,
                    quote,
                },
                subject: rest.get(2).map(|s| (*s).to_owned()),
                reason: None,
                opts: ModOpts::default(),
            })
        }
        "unreact" => {
            let Some(id) = rest.first() else {
                return Err("unreact needs a react id.".into());
            };
            Ok(Command {
                verb: Verb::Unreact {
                    id: (*id).to_owned(),
                },
                subject: rest.get(1).map(|s| (*s).to_owned()),
                reason: None,
                opts: ModOpts::default(),
            })
        }
        "mod" | "moderate" => mod_verb(&rest),
        "activity" => Ok(Command {
            verb: Verb::Activity,
            subject: None,
            reason: None,
            opts: ModOpts::default(),
        }),
        "sort" => {
            let name = match rest.first().copied() {
                Some("magic") => SortName::Magic,
                Some("top") => SortName::Top,
                Some("new") => SortName::New,
                Some("old") => SortName::Old,
                Some("recent" | "recent-comments" | "recentComments") => SortName::RecentComments,
                Some("latest" | "latest-reply" | "latestReply") => SortName::LatestReply,
                other => {
                    return Err(format!(
                        "sort takes magic, top, new, old, recent-comments (posts) or \
                         latest-reply (comments), not `{}`.",
                        other.unwrap_or("")
                    ));
                }
            };
            Ok(Command {
                verb: Verb::Sort(name),
                subject: None,
                reason: None,
                opts: ModOpts::default(),
            })
        }
        "filter" => {
            let filter = match rest.first().copied() {
                Some("frontpage" | "front") => PostFilter::Frontpage,
                Some("all") => PostFilter::All,
                Some("curated") => PostFilter::Curated,
                other => {
                    return Err(format!(
                        "filter takes frontpage, all or curated, not `{}`.",
                        other.unwrap_or("")
                    ));
                }
            };
            Ok(Command {
                verb: Verb::Filter(filter),
                subject: None,
                reason: None,
                opts: ModOpts::default(),
            })
        }
        "notifications" | "notifs" => match rest.first().copied() {
            None | Some("unread") => Ok(Command {
                verb: Verb::Notifications { all: false },
                subject: None,
                reason: None,
                opts: ModOpts::default(),
            }),
            Some("all") => Ok(Command {
                verb: Verb::Notifications { all: true },
                subject: None,
                reason: None,
                opts: ModOpts::default(),
            }),
            Some(other) => Err(format!(
                "notifications takes `all` or `unread`, not `{other}`."
            )),
        },
        "read" => Ok(Command {
            verb: Verb::Read,
            subject: None,
            reason: None,
            opts: ModOpts::default(),
        }),
        "read-all" | "mark-read" => Ok(Command {
            verb: Verb::ReadAll,
            subject: None,
            reason: None,
            opts: ModOpts::default(),
        }),
        "user" | "whoami" => Ok(Command {
            verb: Verb::User {
                actor: rest.first().map(|s| (*s).to_owned()),
            },
            subject: None,
            reason: None,
            opts: ModOpts::default(),
        }),
        "mods" | "moderators" => {
            // `mods`: list them; `mods add <did> [role]` / `mods remove <did>`: `mods_set`
            match rest.first().copied() {
                Some("add") => {
                    let Some(did) = rest.get(1).copied() else {
                        return Err("mods add needs a DID or handle.".into());
                    };
                    let role = match rest.get(2).copied() {
                        None | Some("moderator") | Some("mod") => ModRole::Moderator,
                        Some("admin") => ModRole::Admin,
                        Some(other) => {
                            return Err(format!(
                                "mods add takes admin or moderator, not `{other}`."
                            ));
                        }
                    };
                    Ok(Command {
                        verb: Verb::ModsSet {
                            subject: (*did).to_owned(),
                            role,
                            remove: false,
                        },
                        subject: None,
                        reason: None,
                        opts: ModOpts::default(),
                    })
                }
                Some("remove") | Some("rm") => {
                    let Some(did) = rest.get(1).copied() else {
                        return Err("mods remove needs a DID or handle.".into());
                    };
                    Ok(Command {
                        verb: Verb::ModsSet {
                            subject: (*did).to_owned(),
                            role: ModRole::Moderator,
                            remove: true,
                        },
                        subject: None,
                        reason: None,
                        opts: ModOpts::default(),
                    })
                }
                None => Ok(Command {
                    verb: Verb::ModsList { role: None },
                    subject: None,
                    reason: None,
                    opts: ModOpts::default(),
                }),
                Some(role) => Ok(Command {
                    verb: Verb::ModsList {
                        role: Some((*role).to_owned()),
                    },
                    subject: None,
                    reason: None,
                    opts: ModOpts::default(),
                }),
            }
        }
        other => match more(other, &rest) {
            Some(parsed) => parsed,
            None => Err(format!(
                "Unknown command `{other}`. {}\n{}",
                if other == "help" { "" } else { "Try `help`." },
                usage()
            )),
        },
    }
}

/// A command with `verb` and nothing else.
fn bare(verb: Verb, subject: Option<&&str>) -> Parsed {
    Ok(Command {
        verb,
        subject: subject.map(|s| (*s).to_owned()),
        reason: None,
        opts: ModOpts::default(),
    })
}

/// The words of `args` as one text, without surrounding quotes; `None` when there are none.
fn text_of(args: &[&str]) -> Option<String> {
    trim_reason(&args.join(" "))
}

/// The verbs past the M3 opinion and moderation set: forums and membership, your own items,
/// and the local state (`engine-api.md` §3.2, §3.5, §3.6). `None`: not a verb.
fn more(verb: &str, rest: &[&str]) -> Option<Parsed> {
    let actors = |what: &str| -> Result<Vec<String>, String> {
        if rest.is_empty() {
            return Err(format!("{what} needs one or more DIDs or handles."));
        }
        Ok(rest.iter().map(|s| (*s).to_owned()).collect())
    };
    let access = |op: AccessOp, what: &str| {
        actors(what).map(|actors| Command {
            verb: Verb::Access { op, actors },
            subject: None,
            reason: None,
            opts: ModOpts::default(),
        })
    };
    Some(match verb {
        "open" | "goto" => match rest.first() {
            Some(t) => bare(
                Verb::Open {
                    target: (*t).to_owned(),
                },
                None,
            ),
            None => Err(
                "open needs a forum (owner/slug), a post or a comment (a link or an at:// uri)."
                    .into(),
            ),
        },
        "follow" | "unfollow" => bare(
            Verb::Follow {
                forum: rest.first().map(|s| (*s).to_owned()),
                on: verb == "follow",
            },
            None,
        ),
        "join" | "apply" => bare(
            Verb::Join {
                note: text_of(rest),
            },
            None,
        ),
        "leave" => bare(Verb::Leave, None),
        "members" => {
            let (status, pending) = match rest.first().copied() {
                None => (None, false),
                Some("pending" | "applicants") => (None, true),
                Some(role) => match serde_json::from_value::<ForumRole>(serde_json::Value::String(
                    role.to_owned(),
                )) {
                    Ok(r) => (Some(r), false),
                    Err(_) => {
                        return Some(Err(format!(
                            "members takes pending, owner, admin, moderator, member, \
                             established, newcomer or none, not `{role}`."
                        )));
                    }
                },
            };
            bare(Verb::Members { status, pending }, None)
        }
        "grant" => access(AccessOp::Grant, "grant"),
        "revoke" => access(AccessOp::Revoke, "revoke"),
        "approve" => access(AccessOp::Approve, "approve"),
        "reject" => access(AccessOp::Reject, "reject"),
        "forum" => forum_verb(rest),
        "edit" => bare(Verb::Edit, rest.first()),
        "delete" | "rm" => bare(Verb::Delete, rest.first()),
        "retract" => bare(Verb::Retract, rest.first()),
        "redact" => bare(Verb::Redact, rest.first()),
        "raw" => bare(Verb::Raw, rest.first()),
        "bookmark" | "unbookmark" => bare(
            Verb::Bookmark {
                on: verb == "bookmark",
            },
            rest.first(),
        ),
        "subscribe" | "unsubscribe" => {
            let target = match (rest.first().copied(), rest.get(1)) {
                (None, _) => SubTarget::Post(None),
                (Some("post"), s) => SubTarget::Post(s.map(|s| (*s).to_owned())),
                (Some("forum"), _) => SubTarget::Forum,
                (Some("user"), Some(a)) => SubTarget::User((*a).to_owned()),
                (Some("tag"), Some(t)) => SubTarget::Tag((*t).to_owned()),
                (Some(other), _) => {
                    return Some(Err(format!(
                        "{verb} takes post [subject], forum, user <actor> or tag <slug>, not \
                         `{other}`."
                    )));
                }
            };
            bare(
                Verb::Subscribe {
                    target,
                    on: verb == "subscribe",
                },
                None,
            )
        }
        "mute" | "unmute" => bare(
            Verb::Mute {
                actor: rest.first().map(|s| (*s).to_owned()),
                on: verb == "mute",
            },
            None,
        ),
        "karma-notifications" => {
            let cadence = match rest.first().copied() {
                None => None,
                Some("realtime") => Some(KarmaCadence::Realtime),
                Some("daily") => Some(KarmaCadence::Daily),
                Some("weekly") => Some(KarmaCadence::Weekly),
                Some(other) => {
                    return Some(Err(format!(
                        "karma-notifications takes realtime, daily or weekly, not `{other}`."
                    )));
                }
            };
            bare(Verb::KarmaNotifications(cadence), None)
        }
        "settings" => bare(
            Verb::Settings {
                key: None,
                value: None,
            },
            None,
        ),
        "set" | "unset" => {
            let Some(key) = rest.first() else {
                return Some(Err(format!("{verb} needs a setting (try `settings`).")));
            };
            let value = text_of(&rest[1..]);
            if verb == "set" && value.is_none() {
                return Some(Err(format!("set {key} needs a value.")));
            }
            bare(
                Verb::Settings {
                    key: Some((*key).to_owned()),
                    value: if verb == "set" { value } else { None },
                },
                None,
            )
        }
        "undo" => bare(Verb::Undo, None),
        "retry" => bare(
            Verb::Retry {
                id: rest.first().map(|s| (*s).to_owned()),
            },
            None,
        ),
        _ => return None,
    })
}

/// `forum create <slug> <name...> [--open]`, `forum edit [--flag value...]`.
fn forum_verb(rest: &[&str]) -> Parsed {
    match rest.first().copied() {
        Some("create" | "new") => {
            let open = rest.contains(&"--open");
            let words: Vec<&str> = rest[1..]
                .iter()
                .copied()
                .filter(|w| *w != "--open")
                .collect();
            let (Some(slug), Some(name)) = (words.first(), text_of(words.get(1..).unwrap_or(&[])))
            else {
                return Err("forum create needs a slug and a name.".into());
            };
            bare(
                Verb::ForumCreate {
                    slug: (*slug).to_owned(),
                    name,
                    open,
                },
                None,
            )
        }
        Some("edit") => {
            // each flag's value runs to the next flag, so a name need not be quoted
            let mut c = ForumChanges::default();
            let mut args = rest[1..].iter().copied().peekable();
            while let Some(flag) = args.next() {
                let mut words = Vec::new();
                while let Some(w) = args.next_if(|w| !w.starts_with("--")) {
                    words.push(w);
                }
                let value = trim_reason(&words.join(" ")).unwrap_or_default();
                match flag {
                    "--name" if !value.is_empty() => c.name = Some(value),
                    "--description" => c.description = Some(value),
                    "--report-contact" => c.report_contact = Some(value),
                    "--mode" => {
                        c.membership = Some(match value.as_str() {
                            "open" => MembershipMode::Open,
                            "approval" => MembershipMode::Approval,
                            _ => return Err("--mode takes open or approval.".into()),
                        });
                    }
                    "--tier" => {
                        c.default_tier = Some(match value.as_str() {
                            "personal" => DefaultTier::Personal,
                            "frontpage" => DefaultTier::Frontpage,
                            _ => return Err("--tier takes personal or frontpage.".into()),
                        });
                    }
                    other => {
                        return Err(format!(
                            "forum edit takes --name, --description, --mode, --tier and \
                             --report-contact, not `{other}`."
                        ));
                    }
                }
            }
            if c == ForumChanges::default() {
                return Err(
                    "forum edit needs something to change (--name, --description, \
                            --mode, --tier, --report-contact)."
                        .into(),
                );
            }
            bare(Verb::ForumEdit(c), None)
        }
        _ => Err("forum takes create or edit.".into()),
    }
}

fn vote(v: Option<&str>) -> Result<Vote, String> {
    match v {
        Some("up") => Ok(Vote::Up),
        Some("strong-up") => Ok(Vote::StrongUp),
        Some("down") => Ok(Vote::Down),
        Some("strong-down") => Ok(Vote::StrongDown),
        Some("clear") | None => Ok(Vote::Clear),
        Some(other) => Err(format!(
            "vote takes up, strong-up, down, strong-down or clear, not `{other}`."
        )),
    }
}

fn agree(v: Option<&str>) -> Result<Agree, String> {
    match v {
        Some("for") => Ok(Agree::For),
        Some("strong-for") => Ok(Agree::StrongFor),
        Some("against") => Ok(Agree::Against),
        Some("strong-against") => Ok(Agree::StrongAgainst),
        Some("clear") | None => Ok(Agree::Clear),
        Some(other) => Err(format!(
            "agree takes for, strong-for, against, strong-against or clear, not `{other}`."
        )),
    }
}

/// The capabilities of a `restrict` argument (`vote,react`), if it is a list of them.
fn capabilities(arg: &str) -> Option<Vec<Capability>> {
    arg.split(',')
        .map(|c| serde_json::from_value(serde_json::Value::String(c.to_owned())).ok())
        .collect()
}

/// Every capability (`restrict` without a list, as the CLI).
pub const ALL_CAPABILITIES: [Capability; 7] = [
    Capability::Post,
    Capability::Comment,
    Capability::Vote,
    Capability::React,
    Capability::Tag,
    Capability::Wiki,
    Capability::Sequence,
];

/// A `mod` command line's arguments without its flags, its options, `--spam` and the `--cap`
/// capabilities.
type ModArgs<'a> = (Vec<&'a str>, ModOpts, bool, Vec<Capability>);

/// Takes the CLI-style flags out of a `mod` command line's arguments.
fn mod_flags<'a>(args: &[&'a str]) -> Result<ModArgs<'a>, String> {
    let mut rest = Vec::new();
    let mut opts = ModOpts::default();
    let mut spam = false;
    let mut caps = Vec::new();
    let mut it = args.iter().copied();
    while let Some(a) = it.next() {
        match a {
            "--until" => {
                let Some(when) = it.next() else {
                    return Err(
                        "--until needs a time (RFC 3339, e.g. 2026-11-01T00:00:00Z).".into(),
                    );
                };
                let d = lessdugin_atp::syntax::Datetime::parse(when)
                    .map_err(|e| format!("--until {when:?}: {e}"))?;
                opts.until_ms = Some(d.timestamp_micros() / 1000);
            }
            "--author" | "--author-scope" => opts.author_scope = true,
            "--spam" => spam = true,
            "--cap" => {
                let Some(c) = it.next().and_then(capabilities) else {
                    return Err(
                        "--cap takes post, comment, vote, react, tag, wiki or sequence.".into(),
                    );
                };
                caps.extend(c);
            }
            other => rest.push(other),
        }
    }
    Ok((rest, opts, spam, caps))
}

/// `mod <action> [subject] [reason...] [flags]`, `mod tier <tier> [reason...]`.
fn mod_verb(rest: &[&str]) -> Parsed {
    let Some(action) = rest.first().copied() else {
        return Err(format!(
            "mod needs an action (hide, spam, unhide, keep, ban, unban, pin, unpin, lock, unlock, \
             tier, restrict, unrestrict, nullify, unnullify, approve, adopt).\n{}",
            usage()
        ));
    };
    let (rest, opts, spam, mut caps) = mod_flags(&rest[1..])?;
    // `mod tier frontpage "because"`: the tier is the first argument, not a subject
    let mut args = &rest[..];
    let verb = match action {
        "hide" if spam => ModVerb::Spam,
        "hide" => ModVerb::Hide,
        "spam" => ModVerb::Spam,
        "unhide" => ModVerb::Unhide,
        "keep" => ModVerb::Keep,
        "ban" => ModVerb::Ban,
        "unban" => ModVerb::Unban,
        "pin" => ModVerb::Pin,
        "unpin" => ModVerb::Unpin,
        "lock" => ModVerb::Lock,
        "unlock" => ModVerb::Unlock,
        "restrict" => {
            // `restrict vote,react ...`: the capabilities come first
            if let Some(c) = args.first().and_then(|a| capabilities(a)) {
                caps.extend(c);
                args = &args[1..];
            }
            caps.dedup();
            ModVerb::Restrict(caps)
        }
        "unrestrict" => ModVerb::Unrestrict,
        "nullify" => ModVerb::Nullify,
        "unnullify" => ModVerb::Unnullify,
        "approve" => ModVerb::Approve,
        "adopt" => {
            let Some(former) = args.first() else {
                return Err("mod adopt needs the former moderator (a DID or handle).".into());
            };
            return Ok(Command {
                verb: Verb::Adopt {
                    former: (*former).to_owned(),
                },
                subject: None,
                reason: None,
                opts,
            });
        }
        // the CLI's tier verbs
        "personal" => ModVerb::Tier(Tier::Personal),
        "frontpage" => ModVerb::Tier(Tier::Frontpage),
        "curate" => ModVerb::Tier(Tier::Curated),
        "tier" => {
            let Some(name) = args.first().copied() else {
                return Err("mod tier needs personal, frontpage or curated.".into());
            };
            let tier = match name {
                "personal" => Tier::Personal,
                "frontpage" | "front" => Tier::Frontpage,
                "curated" | "curate" => Tier::Curated,
                other => {
                    return Err(format!(
                        "mod tier takes personal, frontpage or curated, not `{other}`."
                    ));
                }
            };
            args = &args[1..];
            ModVerb::Tier(tier)
        }
        other => {
            return Err(format!(
                "Unknown mod action `{other}`. Try hide, spam, unhide, keep, ban, unban, pin, \
                 unpin, lock, unlock, tier, restrict, unrestrict, nullify, unnullify, approve \
                 or adopt."
            ));
        }
    };
    let (subject, reason) = split_subject(args);
    Ok(Command {
        verb: Verb::Mod(verb),
        subject,
        reason,
        opts,
    })
}

/// An explicit subject is the first argument when it is after the action; a reason is the
/// rest. `mod hide` uses the selection and `mod hide at://...` names a subject.
fn split_subject(args: &[&str]) -> (Option<String>, Option<String>) {
    match args.split_first() {
        Some((first, tail))
            if {
                let f = *first;
                f.starts_with("at://")
                    || f.starts_with("did:")
                    || f.starts_with("tag:")
                    || f.contains('.')
            } =>
        {
            let reason = trim_reason(&tail.join(" "));
            (Some((*first).to_owned()), reason)
        }
        Some((first, tail)) => {
            let whole = if tail.is_empty() {
                (*first).to_owned()
            } else {
                std::iter::once(*first)
                    .chain(tail.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            (None, trim_reason(&whole))
        }
        None => (None, None),
    }
}

/// The reason, without the surrounding quotes a shell user (or a test) writes.
fn trim_reason(s: &str) -> Option<String> {
    let t: String = s.trim().to_owned();
    if t.is_empty() {
        return None;
    }
    let quoted = (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2);
    Some(if quoted {
        t[1..t.len() - 1].to_owned()
    } else {
        t
    })
}

/// The command line's help (shown by `help`, and on a parse error).
pub fn usage() -> String {
    [
        "vote up|strong-up|down|strong-down|clear [subject]",
        "agree for|strong-for|against|strong-against|clear [subject]",
        "react <id> [for|against] [subject]",
        "unreact <id> [subject]",
        "mod hide|spam|unhide|keep|ban|unban|pin|unpin|lock|unlock|unrestrict|nullify|unnullify|approve [subject] [reason]",
        "mod restrict [cap,cap...] [subject] [reason]",
        "mod tier personal|frontpage|curated [subject] [reason]",
        "  mod flags: --until <RFC 3339>, --author (author scope), --spam, --cap <cap>",
        "notifications [all|unread]",
        "read",
        "read-all",
        "activity",
        "sort magic|top|new|old|recent-comments|latest-reply",
        "filter frontpage|all|curated",
        "user [actor]",
        "mods [role]",
        "mods add <did|handle> [admin|moderator]",
        "mods remove <did|handle>",
        "mod adopt <did|handle>",
        "open <owner/slug|post|comment>",
        "follow [forum], unfollow [forum]",
        "join [note], leave",
        "members [pending|owner|admin|moderator|member|established|newcomer|none]",
        "grant|revoke|approve|reject <did|handle...>",
        "forum create <slug> <name> [--open]",
        "forum edit [--name N] [--description D] [--mode open|approval] [--tier personal|frontpage] [--report-contact C]",
        "edit|delete|retract|redact|raw [subject]",
        "react <id> [for|against] [subject] --quote <passage>",
        "bookmark [subject], unbookmark [subject]",
        "subscribe|unsubscribe [post [subject]|forum|user <actor>|tag <slug>]",
        "mute <actor>, unmute <actor>",
        "karma-notifications [realtime|daily|weekly]",
        "settings, set <key> <value>, unset <key>",
        "undo, retry [write id]",
    ]
    .join("\n")
}

/// The karma intent a [`Vote`] names.
pub fn vote_intent(v: Vote) -> lessdugin_core::dto::OpinionIntent {
    use lessdugin_core::dto::OpinionIntent;
    OpinionIntent::Karma {
        value: match v {
            Vote::Up => Some(KarmaValue::Up),
            Vote::StrongUp => Some(KarmaValue::StrongUp),
            Vote::Down => Some(KarmaValue::Down),
            Vote::StrongDown => Some(KarmaValue::StrongDown),
            Vote::Clear => None,
        },
    }
}

/// The agreement intent an [`Agree`] names.
pub fn agree_intent(a: Agree) -> lessdugin_core::dto::OpinionIntent {
    use lessdugin_core::dto::OpinionIntent;
    OpinionIntent::Agreement {
        value: match a {
            Agree::For => Some(AgreementValue::Agree),
            Agree::StrongFor => Some(AgreementValue::StrongAgree),
            Agree::Against => Some(AgreementValue::Disagree),
            Agree::StrongAgainst => Some(AgreementValue::StrongDisagree),
            Agree::Clear => None,
        },
    }
}

/// The pin scope of a subject: a comment is pinned in its thread, anything else in the forum.
pub fn pin_scope(subject: &str) -> PinScope {
    // at://<authority>/<collection>/<rkey>, or the SPA's comment link
    let comment = subject
        .strip_prefix("at://")
        .and_then(|rest| rest.split('/').nth(1))
        == Some(lessdugin_lexicon::collection::nsid::COMMENT)
        || subject.contains("#/c/");
    if comment {
        PinScope::Thread
    } else {
        PinScope::Forum
    }
}

/// The `ModActionIntent` a [`ModVerb`] names for `subject` in `forum`, with `reason` and the
/// command line's options; `None` for `approve`, which is a grant.
pub fn mod_intent(
    verb: &ModVerb,
    forum: &str,
    subject: &str,
    reason: Option<String>,
    opts: &ModOpts,
) -> Option<ModActionIntent> {
    let scope = if opts.author_scope {
        ModScope::Author
    } else {
        ModScope::Forum
    };
    let assert = |action: ModActionValue| ModActionIntent::Assert {
        forum: forum.to_owned(),
        scope,
        subject: subject.to_owned(),
        action,
        reason: reason.clone(),
        until_ms: opts.until_ms,
    };
    let remove = |key: &str| ModActionIntent::Remove {
        forum: forum.to_owned(),
        scope,
        subject: subject.to_owned(),
        action_key: key.to_owned(),
    };
    Some(match verb {
        ModVerb::Hide => assert(ModActionValue::Hide {
            hidden: true,
            spam: false,
            show_reason: true,
        }),
        ModVerb::Spam => assert(ModActionValue::Hide {
            hidden: true,
            spam: true,
            show_reason: true,
        }),
        ModVerb::Unhide => remove("hide"),
        ModVerb::Keep => assert(ModActionValue::Hide {
            hidden: false,
            spam: false,
            show_reason: true,
        }),
        ModVerb::Ban => assert(ModActionValue::Access {
            level: AccessLevel::Banned,
        }),
        ModVerb::Unban => remove("access"),
        ModVerb::Pin => assert(ModActionValue::Pin {
            scope: pin_scope(subject),
            pinned: true,
        }),
        ModVerb::Unpin => assert(ModActionValue::Pin {
            scope: pin_scope(subject),
            pinned: false,
        }),
        ModVerb::Lock => assert(ModActionValue::Lock { locked: true }),
        ModVerb::Unlock => assert(ModActionValue::Lock { locked: false }),
        ModVerb::Tier(tier) => assert(ModActionValue::Tier {
            tier: *tier,
            notice: None,
        }),
        // the engine keeps the dimension a verb leaves out (m3r-2)
        ModVerb::Restrict(caps) => assert(ModActionValue::Restrict {
            capabilities: Some(if caps.is_empty() {
                ALL_CAPABILITIES.to_vec()
            } else {
                caps.clone()
            }),
            nullify_votes: None,
        }),
        ModVerb::Nullify => assert(ModActionValue::Restrict {
            capabilities: None,
            nullify_votes: Some(true),
        }),
        ModVerb::Unnullify => assert(ModActionValue::Restrict {
            capabilities: None,
            nullify_votes: Some(false),
        }),
        ModVerb::Unrestrict => remove("restrict"),
        // `approve` is a grant, not a mod action: the caller sends `Effect::Grant` instead
        ModVerb::Approve => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_m3_verbs_parse() {
        assert_eq!(parse("vote up").unwrap().verb, Verb::Vote(Vote::Up));
        assert_eq!(
            parse("vote strong-down").unwrap().verb,
            Verb::Vote(Vote::StrongDown)
        );
        assert_eq!(
            parse("agree against").unwrap().verb,
            Verb::Agree(Agree::Against)
        );
        assert_eq!(
            parse("react heart").unwrap().verb,
            Verb::React {
                id: "heart".into(),
                against: false,
                quote: None,
            }
        );
        assert_eq!(
            parse("react heart against").unwrap().verb,
            Verb::React {
                id: "heart".into(),
                against: true,
                quote: None,
            }
        );
        assert_eq!(
            parse("unreact heart").unwrap().verb,
            Verb::Unreact { id: "heart".into() }
        );
        assert_eq!(parse("activity").unwrap().verb, Verb::Activity);
        assert_eq!(
            parse("notifications").unwrap().verb,
            Verb::Notifications { all: false }
        );
        assert_eq!(
            parse("notifications all").unwrap().verb,
            Verb::Notifications { all: true }
        );
        assert_eq!(parse("read").unwrap().verb, Verb::Read);
        assert_eq!(parse("read-all").unwrap().verb, Verb::ReadAll);
        assert_eq!(
            parse("user alice").unwrap().verb,
            Verb::User {
                actor: Some("alice".into())
            }
        );
        assert_eq!(parse("mods").unwrap().verb, Verb::ModsList { role: None });
    }

    #[test]
    fn mod_verbs_carry_their_subject_and_reason() {
        let c = parse("mod hide at://did:plc:x/app.bsky.feed.post/1 spam").unwrap();
        assert_eq!(c.verb, Verb::Mod(ModVerb::Hide));
        assert_eq!(
            c.subject.as_deref(),
            Some("at://did:plc:x/app.bsky.feed.post/1")
        );
        assert_eq!(c.reason.as_deref(), Some("spam"));

        // no subject: the selection
        let c = parse("mod ban bob.example.com because").unwrap();
        assert_eq!(c.verb, Verb::Mod(ModVerb::Ban));
        assert_eq!(c.subject.as_deref(), Some("bob.example.com"));

        // a quote is stripped
        let c = parse("mod lock \"off topic\"").unwrap();
        assert_eq!(c.subject, None);
        assert_eq!(c.reason.as_deref(), Some("off topic"));

        // the tier is the action's argument, not a subject
        let c = parse("mod tier curated \"great\" ").unwrap();
        assert_eq!(c.verb, Verb::Mod(ModVerb::Tier(Tier::Curated)));
        assert_eq!(c.subject, None);
        assert_eq!(c.reason.as_deref(), Some("great"));
    }

    /// The CLI's meaning of each verb: an override asserts the opposite value (`social.md`
    /// §4.2), and only `unhide`, `unban`, `unrestrict` remove the account's own record.
    #[test]
    fn mod_verbs_mean_what_the_cli_means() {
        let intent = |line: &str| {
            let c = parse(line).unwrap();
            let Verb::Mod(v) = &c.verb else {
                panic!("{line}")
            };
            mod_intent(
                v,
                "at://f",
                c.subject.as_deref().unwrap_or("did:plc:x"),
                c.reason.clone(),
                &c.opts,
            )
            .unwrap()
        };
        let action = |line: &str| match intent(line) {
            ModActionIntent::Assert { action, .. } => action,
            r => panic!("{line}: {r:?}"),
        };
        assert_eq!(
            action("mod keep"),
            ModActionValue::Hide {
                hidden: false,
                spam: false,
                show_reason: true
            }
        );
        assert_eq!(action("mod unlock"), ModActionValue::Lock { locked: false });
        let comment = format!(
            "at://did:plc:x/{}/3k",
            lessdugin_lexicon::collection::nsid::COMMENT
        );
        assert_eq!(
            action(&format!("mod pin {comment}")),
            ModActionValue::Pin {
                scope: PinScope::Thread,
                pinned: true
            }
        );
        assert_eq!(
            action(&format!("mod unpin {comment}")),
            ModActionValue::Pin {
                scope: PinScope::Thread,
                pinned: false
            }
        );
        assert!(matches!(
            intent("mod unhide"),
            ModActionIntent::Remove { action_key, .. } if action_key == "hide"
        ));
        // restrict: a capability list, every capability without one; each verb names only its
        // own dimension and the engine keeps the other (m3r-2)
        assert_eq!(
            action("mod restrict vote,react did:plc:x"),
            ModActionValue::Restrict {
                capabilities: Some(vec![Capability::Vote, Capability::React]),
                nullify_votes: None
            }
        );
        assert_eq!(
            action("mod restrict --cap wiki did:plc:x"),
            ModActionValue::Restrict {
                capabilities: Some(vec![Capability::Wiki]),
                nullify_votes: None
            }
        );
        assert_eq!(
            action("mod restrict did:plc:x"),
            ModActionValue::Restrict {
                capabilities: Some(ALL_CAPABILITIES.to_vec()),
                nullify_votes: None
            }
        );
        for (line, nullify) in [
            ("mod nullify did:plc:x", true),
            ("mod unnullify did:plc:x", false),
        ] {
            assert_eq!(
                action(line),
                ModActionValue::Restrict {
                    capabilities: None,
                    nullify_votes: Some(nullify)
                },
                "{line}"
            );
        }
        let c = parse("mod restrict vote did:plc:x").unwrap();
        assert_eq!(c.subject.as_deref(), Some("did:plc:x"));
        // --until and --author
        match intent("mod hide --until 2026-11-01T00:00:00Z --author \"spam\"") {
            ModActionIntent::Assert {
                until_ms,
                scope,
                reason,
                ..
            } => {
                assert_eq!(until_ms, Some(1_793_491_200_000));
                assert_eq!(scope, ModScope::Author);
                assert_eq!(reason.as_deref(), Some("spam"));
            }
            r => panic!("{r:?}"),
        }
        assert!(parse("mod hide --until soon").is_err());
        assert_eq!(
            parse("mod hide --spam").unwrap().verb,
            Verb::Mod(ModVerb::Spam)
        );
    }

    #[test]
    fn bad_input_says_what_it_wanted() {
        assert!(parse("").unwrap_err().contains("vote up"));
        assert!(parse("frobnicate").unwrap_err().contains("Unknown command"));
        assert!(parse("vote sideways").unwrap_err().contains("vote takes"));
        assert!(parse("mod").unwrap_err().contains("mod needs an action"));
        assert!(
            parse("mod tier")
                .unwrap_err()
                .contains("personal, frontpage")
        );
        assert!(parse("react").unwrap_err().contains("react id"));
    }
    #[test]
    fn mods_lists_sets_and_removes() {
        assert_eq!(parse("mods").unwrap().verb, Verb::ModsList { role: None });
        assert_eq!(
            parse("mods admin").unwrap().verb,
            Verb::ModsList {
                role: Some("admin".into())
            }
        );
        assert_eq!(
            parse("mods add did:plc:x").unwrap().verb,
            Verb::ModsSet {
                subject: "did:plc:x".into(),
                role: ModRole::Moderator,
                remove: false
            }
        );
        assert_eq!(
            parse("mods add did:plc:x admin").unwrap().verb,
            Verb::ModsSet {
                subject: "did:plc:x".into(),
                role: ModRole::Admin,
                remove: false
            }
        );
        assert_eq!(
            parse("mods remove did:plc:x").unwrap().verb,
            Verb::ModsSet {
                subject: "did:plc:x".into(),
                role: ModRole::Moderator,
                remove: true
            }
        );
        assert!(parse("mods add").unwrap_err().contains("needs a DID"));
        assert!(parse("mods remove").unwrap_err().contains("needs a DID"));
        assert!(parse("mods add did:plc:x owner").is_err());
    }
}
