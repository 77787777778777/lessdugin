// SPDX-License-Identifier: AGPL-3.0-or-later
//! Command-line arguments (`docs/design/synthesis-v1.md` §6.10).
//!
//! There is deliberately no option that takes a password: app passwords come from a no-echo
//! prompt, `--password-stdin` or `LESSDUGIN_APP_PASSWORD` (`threat-model.md` AUTH-3). A hidden
//! `--password` and a hidden catch-all positional of `login` exist only to refuse them with an
//! explanation, without echoing the value.
//!
//! `--dev` has no environment variable: devnet mode is switched on only by the command line or
//! by a `[network]` profile in `config.toml` (and only in builds that have it, see
//! `ctx::DEVNET_AVAILABLE`).

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use url::Url;

mod content;
pub use content::*;

mod social;
pub use social::*;

mod local;
pub use local::*;

mod organisation;
pub use organisation::*;

/// A serverless markdown and media forum on atproto.
#[derive(Debug, Parser)]
#[command(
    name = "lessdugin",
    version,
    about,
    long_about = None,
    after_help = "Exit codes: 0 ok, 1 error, 2 usage, 3 login required, 4 not found, \
                  5 conflict, 6 partial."
)]
pub struct Cli {
    /// Options for every command.
    #[command(flatten)]
    pub global: GlobalArgs,
    /// The command.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// `--color`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    /// Colour on a terminal, unless `NO_COLOR` is set or `TERM=dumb`.
    #[default]
    Auto,
    /// Always colour.
    Always,
    /// Plain output.
    Never,
}

/// Global options.
#[derive(Clone, Debug, Args)]
pub struct GlobalArgs {
    /// Act as this logged-in account (handle or DID) instead of the active one.
    #[arg(long, global = true, value_name = "ACCOUNT", env = "LESSDUGIN_ACCOUNT")]
    pub account: Option<String>,

    /// Print JSON (dto/1) instead of text; errors go to stderr as JSON.
    #[arg(long, global = true)]
    pub json: bool,

    /// Keep the configuration, data and cache under this directory.
    #[arg(long, global = true, value_name = "DIR", env = "LESSDUGIN_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// When to use colour.
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    /// Use a local devnet (PLC + PDS, `.test` handles; never enabled implicitly, development
    /// builds only).
    #[arg(long, global = true)]
    pub dev: bool,

    /// Devnet PLC directory (with --dev).
    #[arg(long, global = true, value_name = "URL", env = "LESSDUGIN_PLC_URL")]
    pub plc_url: Option<Url>,

    /// Devnet PDS (with --dev).
    #[arg(long, global = true, value_name = "URL", env = "LESSDUGIN_PDS_URL")]
    pub pds_url: Option<Url>,

    /// More log output on stderr (-v debug, -vv trace); RUST_LOG overrides.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Only errors on stderr.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// The forum to act in, instead of the current one (`forum use`).
    #[arg(long, global = true, value_name = "FORUM", env = "LESSDUGIN_FORUM")]
    pub forum: Option<String>,

    /// Indexers for this invocation: a consenting HTTPS URL, `forum`, or `none`.
    #[arg(long, global = true, value_name = "URL|none|forum")]
    pub indexer: Option<String>,

    /// Do not use the network: read the local cache only; writes stay queued.
    #[arg(long, global = true)]
    pub offline: bool,
}

/// Commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read a post and its comments (labelled c1…cN; `next` and `prev` step along its
    /// sequence; no login needed).
    Read(ReadArgs),
    /// List a forum's posts, numbered (`read 3` reads the third).
    Ls(LsArgs),
    /// Discover public forums, optionally matching a name.
    Discover(DiscoverArgs),
    /// List the listed posts that link to an item.
    Linked(RefArgs),
    /// Search local titles and rendered bodies; nothing is sent to a search service.
    Search(SearchArgs),
    /// Show a cached user page.
    User(UserArgs),
    /// List or read a forum's tag pages.
    Tag(TagArgs),
    /// Edit a tag/wiki page (owner and moderators).
    Wiki(WikiArgs),
    /// Read or edit sequence libraries.
    Sequence(SequenceArgs),
    /// Publish a post from a markdown file, or write it in $EDITOR.
    Post(PostArgs),
    /// Comment on a post, or reply to a comment.
    Comment(CommentArgs),
    /// Edit your post or comment (the old version is kept as a revision).
    Edit(EditArgs),
    /// Delete your post or comment, with its revisions.
    Delete(DeleteArgs),
    /// Retract your comment (it stays visible, marked as retracted).
    Retract(RefArgs),
    /// List the versions of a post or comment.
    History(RefArgs),
    /// Delete the version history of one of your posts or comments (cannot be undone).
    Redact(RedactArgs),
    /// Sync a forum (or every followed forum) and send queued writes.
    Sync(SyncArgs),
    /// Join a forum: follow it, write your membership and make it the current forum.
    Join(JoinArgs),
    /// Leave a forum: delete your membership (the forum stays followed).
    Leave(LeaveArgs),
    /// Forums: ls, show, follow, unfollow, use, create.
    Forum(ForumArgs),
    /// List a forum's members.
    Members(MembersArgs),
    /// Grant membership to applicants (approval-mode forums; owner and admins).
    Approve(ApproveArgs),
    /// Dismiss applicants on this device, and remove your grant of them if you made one.
    Reject(RejectArgs),
    /// Vote on a post or comment (public record).
    Vote(VoteArgs),
    /// Cast an agreement vote (public record).
    Agree(AgreeArgs),
    /// React to a post or comment, optionally against or on a quote (public record).
    React(ReactArgs),
    /// List the react palette.
    Reacts,
    /// Your notifications (computed locally; --mark-read marks them read).
    Notifications(NotificationsArgs),
    /// Mute authors on this device: their items are shown collapsed (local, not public).
    Mute(MuteArgs),
    /// Unmute authors.
    Unmute(MuteArgs),
    /// List the authors you muted.
    Mutes,
    /// Bookmarks, kept on this device: add, rm, ls.
    Bookmark(BookmarkArgs),
    /// Be notified of new comments on a post, or new posts in a forum, by a user or with a tag
    /// (kept on this device).
    Subscribe(SubscribeArgs),
    /// Remove a subscription.
    Unsubscribe(SubscribeArgs),
    /// List your subscriptions.
    Subs,
    /// Export your posts and comments, or a forum, as JSON or markdown (--local: this device's
    /// local state, which --import merges back).
    Export(ExportArgs),
    /// Recent Discussion: the posts with the newest comments.
    Activity(ActivityArgs),
    /// Moderators: add, rm (a removal adopts their assertions first).
    Mods(ModsArgs),
    /// Moderate: hide, ban, restrict, tier, pin, lock, queue, log, adopt, …
    Mod(ModArgs),
    /// Drafts kept in the data directory: ls, edit, rm, publish.
    Draft(DraftArgs),
    /// Media: add a file for a post, get or open a post's attachment.
    Media(MediaArgs),
    /// Queued writes: ls, flush, retry, undo.
    Outbox(OutboxArgs),
    /// Log in (OAuth in the browser by default; --app-password for bots and headless use).
    Login(LoginArgs),
    /// Log out: revoke the session and delete it from the keyring.
    Logout(LogoutArgs),
    /// Show the account in use and check its session with its PDS.
    Whoami,
    /// List logged-in accounts, or choose the active one.
    Accounts(AccountsArgs),
    /// Resolve a handle or DID, checking the handle in both directions.
    Resolve(ResolveArgs),
    /// Verify a record against its repository's signed commit (proofs/1).
    Verify(VerifyArgs),
    /// Check the configuration, keyring, network and the account's PDS.
    Doctor(DoctorArgs),
    /// This device's settings: get [key], set <key> [value].
    Config(ConfigArgs),
    /// Print a shell completion script.
    Completions(CompletionsArgs),
    /// Serve the web app on 127.0.0.1 with a bridge to your logged-in accounts (the browser
    /// holds no secret; each printed link works once).
    #[cfg(feature = "web")]
    Web(crate::web::WebArgs),
    /// Open the terminal UI (bare `lessdugin` does too, on a terminal with an account).
    #[cfg(feature = "tui")]
    Tui(TuiArgs),
}

/// `discover`.
#[derive(Debug, Args)]
pub struct DiscoverArgs {
    /// Match forum names (all discoverable forums when omitted).
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,
}

/// `tui`.
#[cfg(feature = "tui")]
#[derive(Debug, Default, Args)]
pub struct TuiArgs {
    /// A forum to open (as `ls` takes it); without one, the forums you follow.
    #[arg(value_name = "FORUM")]
    pub forum: Option<String>,
}

/// `--oauth-client`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OAuthClientArg {
    /// A loopback client (`http://localhost?…`), with the hosted metadata as fallback.
    #[default]
    Loopback,
    /// The hosted native client metadata.
    Hosted,
}

/// `login`.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// The handle or DID to log in as.
    #[arg(value_name = "HANDLE|DID")]
    pub actor: Option<String>,

    /// Log in with an app password (prompted without echo, or from --password-stdin or
    /// LESSDUGIN_APP_PASSWORD) instead of OAuth.
    #[arg(long, conflicts_with_all = ["no_browser", "oauth_client"])]
    pub app_password: bool,

    /// Read the app password from the first line of standard input (implies --app-password).
    #[arg(long, conflicts_with_all = ["no_browser", "oauth_client"])]
    pub password_stdin: bool,

    /// Print the authorization URL instead of opening a browser, then paste back the address
    /// of the page that fails to load (works over SSH).
    #[arg(long)]
    pub no_browser: bool,

    /// Which OAuth client metadata to use.
    #[arg(long, value_enum)]
    pub oauth_client: Option<OAuthClientArg>,

    /// The hosted native client metadata URL (default: config.toml's `oauth_client_id`, else
    /// the production metadata): a preview or self-hosted origin serving it for itself.
    #[arg(long, value_name = "URL", conflicts_with_all = ["app_password", "password_stdin"])]
    pub oauth_client_id: Option<Url>,

    /// Refused: passwords are never read from the command line.
    #[arg(long = "password", short = 'p', hide = true, num_args = 0..=1, value_name = "X")]
    pub password_argv: Option<Option<String>>,

    /// Refused: anything after the handle (most likely a password) is not echoed back.
    #[arg(hide = true, value_name = "EXTRA", num_args = 0..)]
    pub extra: Vec<String>,
}

/// `logout`.
#[derive(Debug, Args)]
pub struct LogoutArgs {
    /// Also delete the account's local state (outbox, write budget, notification state).
    #[arg(long)]
    pub purge: bool,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// `accounts`.
#[derive(Debug, Args)]
pub struct AccountsArgs {
    /// What to do (default: list).
    #[command(subcommand)]
    pub action: Option<AccountsAction>,
}

/// `accounts use`.
#[derive(Debug, Subcommand)]
pub enum AccountsAction {
    /// Make a logged-in account the active one.
    Use {
        /// Handle or DID.
        account: String,
    },
}

/// `resolve`.
#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// A handle, DID, `@handle` or `at://` identifier.
    #[arg(value_name = "HANDLE|DID")]
    pub actor: String,
}

/// `verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// The record, `at://<did|handle>/<collection>/<rkey>`.
    #[arg(value_name = "AT-URI")]
    pub uri: String,
    /// Also check references and blobs.
    #[arg(long)]
    pub deep: bool,
}

/// `doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Check this PDS instead of the account's.
    #[arg(long, value_name = "URL")]
    pub pds: Option<Url>,
}

/// `completions`.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// The shell.
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

/// How the CLI reaches each operation of `engine-api.md` §3.2 (forums and membership), §3.5
/// (writes) and §3.6 (local state): `docs/plan.md` §M3, "every MVP action is reachable in all
/// three clients". The TUI keeps the same table (`lessdugin_tui::command::REACH`); the test
/// holds this one to the spec's tables, and each command line it names to the parser, so an
/// operation the spec adds fails here until the CLI reaches it (or says why it does not).
#[cfg(test)]
mod reach {
    use std::collections::BTreeSet;

    use clap::CommandFactory;

    use super::Cli;

    /// How the CLI reaches an engine operation.
    enum Reach {
        /// This command line (its words are checked against the parser).
        Command(&'static str),
        /// No command of its own: a command calls it on the way, as the text says.
        Automatic(&'static str),
        /// Not in the CLI, for the reason given (a later milestone, or another client's job).
        NotHere(&'static str),
    }

    const REACH: &[(&str, Reach)] = &[
        // §3.2
        ("forum_open", Reach::Command("forum show")),
        ("forum_discover", Reach::Command("discover rust")),
        (
            "forum_follow",
            Reach::Command("forum follow alice.example.com/meta"),
        ),
        ("forum_unfollow", Reach::Command("forum unfollow")),
        ("followed", Reach::Command("forum ls")),
        ("forum_create", Reach::Command("forum create meta")),
        ("forum_update", Reach::Command("forum edit --name Meta")),
        ("join", Reach::Command("join alice.example.com/meta")),
        ("leave", Reach::Command("leave")),
        ("members", Reach::Command("members --pending")),
        (
            "grant",
            Reach::Automatic("`approve` grants (and forgets a rejection); `mod approve` too"),
        ),
        (
            "revoke",
            Reach::Command("members revoke did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
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
            Reach::Automatic("`members --pending` leaves out the applicants rejected here"),
        ),
        (
            "mods_set",
            Reach::Command("mods add did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        // §3.5
        ("publish", Reach::Command("post --title Hello")),
        ("comment", Reach::Command("comment 3 -m Hi")),
        ("edit", Reach::Command("edit 3 -m Changed")),
        (
            "edit_preflight",
            Reach::Automatic("`edit` asks first when the edit suspends a keep, tier or pin"),
        ),
        ("delete", Reach::Command("delete 3 --yes")),
        ("retract", Reach::Command("retract c1")),
        ("opinion", Reach::Command("vote 3 up")),
        ("mod_action", Reach::Command("mod hide 3")),
        (
            "mod_adopt",
            Reach::Command("mod adopt did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        (
            "wiki_edit",
            Reach::Command("wiki edit rust --name Rust --file page.md"),
        ),
        (
            "seq_create",
            Reach::Command("sequence create sequence.json"),
        ),
        (
            "seq_edit",
            Reach::Command(
                "sequence edit at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1 sequence.json",
            ),
        ),
        (
            "seq_add",
            Reach::Command(
                "sequence add at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1 at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.post/p1 --chapter 0 --index 0",
            ),
        ),
        (
            "seq_remove",
            Reach::Command(
                "sequence remove at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1 at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.post/p1",
            ),
        ),
        (
            "seq_move",
            Reach::Command(
                "sequence move at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1 --chapter 0 --index 0 --to-chapter 0 --to-index 1",
            ),
        ),
        ("redact_revision", Reach::Command("redact 3 --yes")),
        ("prepare_media", Reach::Command("media add chart.png")),
        ("undo", Reach::Command("outbox undo 1")),
        ("retry_write", Reach::Command("outbox retry 1")),
        (
            "write_status",
            Reach::Automatic("a writing command reports its write's final ticket"),
        ),
        ("writes", Reach::Command("outbox ls")),
        // §3.6
        (
            "mark_read",
            Reach::Automatic("`read` marks the thread read"),
        ),
        (
            "mark_notifications_read",
            Reach::Command("notifications --mark-read"),
        ),
        (
            "mark_all_notifications_read",
            Reach::Command("notifications --all --mark-read"),
        ),
        ("bookmark", Reach::Command("bookmark add 3")),
        ("subscribe", Reach::Command("subscribe forum")),
        (
            "mute",
            Reach::Command("mute did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        (
            "acknowledged_public_actions",
            Reach::Command("config get acknowledgedPublicActions"),
        ),
        (
            "public_action_acknowledged",
            Reach::Automatic("the first public action of a kind shows the interstitial"),
        ),
        (
            "acknowledge_public_action",
            Reach::Automatic("answering the interstitial (or --yes) records it"),
        ),
        (
            "karma_notifications",
            Reach::Command("config get karmaNotifications"),
        ),
        (
            "set_karma_notifications",
            Reach::Command("notifications --karma-cadence weekly"),
        ),
        (
            "save_draft",
            Reach::NotHere("the CLI's drafts are files of the data directory (`draft`)"),
        ),
        (
            "delete_draft",
            Reach::NotHere("the CLI's drafts are files of the data directory (`draft`)"),
        ),
        ("settings", Reach::Command("config get")),
        (
            "set_setting",
            Reach::Command("config set mediaAutoload off"),
        ),
        ("export_local", Reach::Command("export --local")),
        ("import_local", Reach::Command("export --import state.json")),
    ];

    /// How the CLI writes each value `opinion` and `mod_action` take: every `kind` of
    /// `OpinionIntent` and `ModActionValue` (`testdata/dto/dto-1.d.ts`, `engine-api.md` §3.5),
    /// since reaching the operation with one value (`vote 3 up`) does not reach the others (a
    /// tag vote, the moderator's `#tag`).
    const VARIANTS: &[(&str, &str)] = &[
        ("OpinionIntent.karma", "vote 3 up"),
        ("OpinionIntent.agreement", "agree 3 agree"),
        ("OpinionIntent.react", "react 3 heart --quote Thanks"),
        ("OpinionIntent.tag", "vote 3 up --tag rust"),
        ("ModActionValue.hide", "mod hide 3 --spam"),
        (
            "ModActionValue.access",
            "mod ban did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        (
            "ModActionValue.restrict",
            "mod restrict did:plc:aaaaaaaaaaaaaaaaaaaaaaaa --cap vote",
        ),
        ("ModActionValue.tier", "mod curate 3"),
        ("ModActionValue.pin", "mod pin 3 --scope forum"),
        ("ModActionValue.lock", "mod lock 3"),
        ("ModActionValue.tag", "mod tag rust --removed"),
    ];

    /// The `kind`s of the TypeScript union `name` in `testdata/dto/dto-1.d.ts`.
    fn dto_kinds(name: &str) -> BTreeSet<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/dto/dto-1.d.ts");
        let dts = std::fs::read_to_string(path).expect("testdata/dto/dto-1.d.ts");
        let head = format!("export type {name} = ");
        let line = dts
            .lines()
            .find(|l| l.starts_with(&head))
            .unwrap_or_else(|| panic!("no {name} in dto-1.d.ts"));
        line.split("kind: \"")
            .skip(1)
            .filter_map(|k| k.split('"').next())
            .map(|k| format!("{name}.{k}"))
            .collect()
    }

    /// The opinion or the asserted mod-action value a command line sends, as `Type.kind`.
    fn sent_kind(line: &str) -> String {
        use clap::Parser;
        let words = std::iter::once("lessdugin").chain(line.split_whitespace());
        let cli = Cli::try_parse_from(words).unwrap_or_else(|e| panic!("{line}: {e}"));
        let (name, value) = match cli.command {
            Some(super::Command::Vote(a)) => ("OpinionIntent", serde_json::to_value(a.intent())),
            Some(super::Command::Agree(a)) => ("OpinionIntent", serde_json::to_value(a.intent())),
            Some(super::Command::React(a)) => ("OpinionIntent", serde_json::to_value(a.intent())),
            Some(super::Command::Mod(m)) => (
                "ModActionValue",
                serde_json::to_value(
                    m.action
                        .asserted()
                        .unwrap_or_else(|| panic!("{line} asserts nothing")),
                ),
            ),
            other => panic!("{line}: neither an opinion nor a mod action: {other:?}"),
        };
        let value = value.unwrap();
        format!("{name}.{}", value["kind"].as_str().unwrap_or_default())
    }

    #[test]
    fn every_opinion_and_mod_action_value_has_a_way_in() {
        let mut want = dto_kinds("OpinionIntent");
        want.extend(dto_kinds("ModActionValue"));
        assert!(want.len() >= 11, "the dto unions were not read: {want:?}");
        let have: BTreeSet<String> = VARIANTS.iter().map(|(k, _)| (*k).to_owned()).collect();
        assert_eq!(VARIANTS.len(), have.len(), "a variant is listed twice");
        assert_eq!(have, want, "the variants the CLI reaches, against dto/1");
        for (kind, line) in VARIANTS {
            parses(line);
            assert_eq!(sent_kind(line), *kind, "{line}");
        }
    }

    /// How the CLI reaches each operation argument and field that `engine-api.md` §1 "After
    /// the freeze (M5)" adds to dto/1: a new optional request field or argument by the command
    /// line that sends it, a new optional output field by the command that shows it. A test
    /// holds this table to that paragraph, so a field the spec adds there fails until the CLI
    /// reaches it.
    const FIELDS: &[(&str, &str)] = &[
        ("PostView.tagRelevance", "read 3"),
        (
            "WikiDraft.base",
            "wiki edit rust --name Rust --file page.md --base at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.wikiRevision/w1",
        ),
        ("ModQueue.banners", "mod queue"),
        (
            "get_post.sequence",
            "read 3 --sequence at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1",
        ),
        (
            "SequenceView.continueReading",
            "sequence read at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.sequence/s1",
        ),
        ("EditOpts.dropLinkCard", "edit 3 --drop-link-card"),
    ];

    /// The new optional request fields and arguments (`request`) and output fields (`output`)
    /// that `engine-api.md` §1 "After the freeze (`milestone`)" names, as `Type.field` (an
    /// argument `op(a, b?)` as `op.b`).
    fn freeze_fields(milestone: &str) -> std::collections::BTreeMap<String, &'static str> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/spec/engine-api.md");
        let spec = std::fs::read_to_string(path).expect("docs/spec/engine-api.md");
        let head = format!("- **After the freeze ({milestone}).**");
        let start = spec.find(&head).unwrap_or_else(|| panic!("no {head:?}"));
        let rest = &spec[start + head.len()..];
        let para = &rest[..rest.find("\n- **").unwrap_or(rest.len())];
        let para = para.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut out = std::collections::BTreeMap::new();
        for (marker, kind) in [
            ("ew optional request field", "request"),
            ("ew optional request argument", "request"),
            ("ew optional output field", "output"),
        ] {
            for (i, _) in para.match_indices(marker) {
                let Some(code) = para[i + marker.len()..].split('`').nth(1) else {
                    continue;
                };
                match code.split_once('(') {
                    Some((op, args)) => {
                        for a in args.trim_end_matches(')').split(',') {
                            if let Some(a) = a.trim().strip_suffix('?') {
                                out.insert(format!("{op}.{a}"), kind);
                            }
                        }
                    }
                    None => {
                        out.insert(code.to_owned(), kind);
                    }
                }
            }
        }
        out
    }

    /// The fields of [`FIELDS`] that the command line `line` sends.
    fn sends(line: &str) -> Vec<&'static str> {
        use clap::Parser;
        let words = std::iter::once("lessdugin").chain(line.split_whitespace());
        let cli = Cli::try_parse_from(words).unwrap_or_else(|e| panic!("{line}: {e}"));
        let mut out = Vec::new();
        match cli.command {
            Some(super::Command::Wiki(super::WikiArgs {
                action: super::WikiAction::Edit { base: Some(_), .. },
            })) => out.push("WikiDraft.base"),
            Some(super::Command::Read(a)) if a.sequence.is_some() => out.push("get_post.sequence"),
            Some(super::Command::Edit(a)) if a.drop_link_card => out.push("EditOpts.dropLinkCard"),
            _ => {}
        }
        out
    }

    #[test]
    fn every_new_field_has_a_way_in() {
        let spec = freeze_fields("M5");
        let have: BTreeSet<String> = FIELDS.iter().map(|(f, _)| (*f).to_owned()).collect();
        assert_eq!(FIELDS.len(), have.len(), "a field is listed twice");
        let want: BTreeSet<String> = spec.keys().cloned().collect();
        assert!(want.len() >= 6, "the paragraph was not read: {spec:?}");
        assert_eq!(
            have, want,
            "the M5 fields the CLI reaches, against engine-api.md"
        );
        for (field, line) in FIELDS {
            parses(line);
            if spec[*field] == "request" {
                assert_eq!(sends(line), [*field], "{line}");
            }
        }
    }

    /// The tag vote and the moderator `#tag` carry what their flags say.
    #[test]
    fn tag_votes_and_tag_moderation_build_their_values() {
        use clap::Parser;
        use lessdugin_core::dto::{KarmaValue, ModActionValue, OpinionIntent};
        let parse = |line: &str| {
            Cli::try_parse_from(std::iter::once("lessdugin").chain(line.split_whitespace()))
                .unwrap_or_else(|e| panic!("{line}: {e}"))
                .command
        };
        let intent = |line: &str| match parse(line) {
            Some(super::Command::Vote(a)) => a.intent(),
            other => panic!("{line}: {other:?}"),
        };
        assert_eq!(
            intent("vote 3 down --tag #rust"),
            OpinionIntent::Tag {
                slug: "rust".into(),
                value: Some(KarmaValue::Down)
            }
        );
        assert_eq!(
            intent("vote 3 clear --tag rust"),
            OpinionIntent::Tag {
                slug: "rust".into(),
                value: None
            }
        );
        assert_eq!(
            intent("vote 3 strong-up"),
            OpinionIntent::Karma {
                value: Some(KarmaValue::StrongUp)
            }
        );
        let value = |line: &str| match parse(line) {
            Some(super::Command::Mod(m)) => m.action.asserted(),
            other => panic!("{line}: {other:?}"),
        };
        let head = "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.wikiRevision/w1";
        assert_eq!(
            value("mod tag tag:rust --removed"),
            Some(ModActionValue::Tag {
                removed: true,
                head: None
            })
        );
        assert_eq!(
            value(&format!("mod tag rust --head {head}")),
            Some(ModActionValue::Tag {
                removed: false,
                head: Some(head.into())
            })
        );
        assert_eq!(
            value(&format!("mod tag rust --removed --head {head}")),
            Some(ModActionValue::Tag {
                removed: true,
                head: Some(head.into())
            })
        );
        assert_eq!(
            value("mod tag rust --restore"),
            Some(ModActionValue::Tag {
                removed: false,
                head: None
            })
        );
        assert_eq!(value("mod untag rust"), None);
        assert_eq!(super::tag_subject("rust"), "tag:rust");
        assert_eq!(super::tag_subject("tag:rust"), "tag:rust");
        // a `#tag` record must say something; --removed and --restore contradict each other
        for bad in [
            "mod tag rust",
            "mod tag rust --removed --restore",
            "mod tag rust --author-scope --removed",
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("lessdugin").chain(bad.split_whitespace()))
                    .is_err(),
                "{bad}"
            );
        }
    }

    /// `engine-api.md` §3.6: headless, the first public action of a kind needs `--yes`, so
    /// every command that writes one takes it (and `-y`); so does every write that asks a
    /// question of its own (publishing, uploads of this machine's files, what an edit
    /// suspends, a deletion), so a script confirms each the same way. `redact` alone has no
    /// `-y`: deleting history is kept a long flag away from a typo (`RedactArgs`).
    #[test]
    fn every_write_that_asks_takes_yes() {
        use clap::Parser;
        const ASKS: &[&str] = &[
            // public actions (social.md §17)
            "vote 3 up",
            "vote 3 up --tag rust",
            "agree 3 agree",
            "react 3 heart",
            "join alice.example.com/meta",
            "approve did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
            "members revoke did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
            "mods add did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
            "mods rm did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
            "mod hide 3",
            "mod unhide 3",
            "mod tag rust --removed",
            "mod untag rust",
            "mod adopt did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
            // writes with a question of their own
            "post page.md",
            "comment 3 page.md",
            "edit 3 page.md",
            "delete 3",
            "redact 3",
            "draft publish abc123",
        ];
        for line in ASKS {
            let short: &[&str] = if line.starts_with("redact") {
                &[]
            } else {
                &["-y"]
            };
            for yes in std::iter::once("--yes").chain(short.iter().copied()) {
                let words = std::iter::once("lessdugin")
                    .chain(line.split_whitespace())
                    .chain([yes]);
                if let Err(e) = Cli::try_parse_from(words) {
                    panic!("`{line} {yes}`: {e}");
                }
            }
        }
    }

    /// Every command (`mod hide`, `post`; a command whose subcommand is optional counts
    /// itself), with whether it takes `--yes`: its own, or a global one of a parent.
    fn commands() -> Vec<(String, bool)> {
        fn walk(c: &clap::Command, path: &str, inherited: bool, out: &mut Vec<(String, bool)>) {
            let yes = c.get_arguments().find(|a| a.get_id() == "yes");
            let takes = inherited || yes.is_some();
            let subs: Vec<&clap::Command> = c
                .get_subcommands()
                .filter(|s| s.get_name() != "help")
                .collect();
            if !path.is_empty() && (subs.is_empty() || !c.is_subcommand_required_set()) {
                out.push((path.to_owned(), takes));
            }
            let global = inherited || yes.is_some_and(clap::Arg::is_global_set);
            for s in subs {
                let p = format!("{path} {}", s.get_name());
                walk(s, p.trim_start(), global, out);
            }
        }
        let mut out = Vec::new();
        walk(&Cli::command(), "", false, &mut out);
        out
    }

    /// The other side of `every_write_that_asks_takes_yes`: a command without `--yes` is listed
    /// here with the reason it never asks, so a new write command cannot ship without `--yes`
    /// (`engine-api.md` §3.6) unless it is classified. What is a public action follows
    /// `social.md` §17 as the TUI (`Effect::public_kind`) and the SPA (`requestAck`) gate it:
    /// content writes, leaving, and dismissing applicants are not.
    #[test]
    fn every_command_without_yes_asks_nothing() {
        const ASKS_NOTHING: &[&str] = &[
            // reads, and listings of this device's state
            "read",
            "ls",
            "discover",
            "linked",
            "search",
            "user",
            "tag",
            "history",
            "members",
            "reacts",
            "notifications",
            "mutes",
            "subs",
            "export",
            "activity",
            "sequence ls",
            "sequence read",
            "forum ls",
            "forum show",
            "bookmark",
            "bookmark ls",
            "draft",
            "draft ls",
            "media get",
            "media open",
            "outbox",
            "outbox ls",
            "accounts",
            "whoami",
            "resolve",
            "verify",
            "doctor",
            "config get",
            "completions",
            // local state, never written to a PDS (`engine-api.md` §3.6)
            "mute",
            "unmute",
            "bookmark add",
            "bookmark rm",
            "subscribe post",
            "subscribe forum",
            "subscribe user",
            "subscribe tag",
            "unsubscribe post",
            "unsubscribe forum",
            "unsubscribe user",
            "unsubscribe tag",
            "forum follow",
            "forum unfollow",
            "forum use",
            "draft edit",
            "draft rm",
            "media add",
            "accounts use",
            "config set",
            // writes that are no public action and ask no question: content (wiki, sequences,
            // a forum's declaration), a retraction, leaving, dismissing applicants (with the
            // removal of one's own grant), and what the outbox already holds
            "wiki edit",
            "sequence create",
            "sequence edit",
            "sequence add",
            "sequence remove",
            "sequence move",
            "retract",
            "leave",
            "reject",
            "forum create",
            "forum edit",
            "sync",
            "outbox flush",
            "outbox retry",
            "outbox undo",
            // a login's question (an unverified handle) has no --yes on purpose: headless, it
            // is refused and the DID is given instead (`cmd::login::confirm_target`)
            "login",
            "web",
            "tui",
        ];
        let all = commands();
        let names: BTreeSet<&str> = all.iter().map(|(c, _)| c.as_str()).collect();
        for c in ASKS_NOTHING {
            assert!(names.contains(c), "`{c}` is no command");
        }
        let listed: BTreeSet<&str> = ASKS_NOTHING.iter().copied().collect();
        assert_eq!(
            listed.len(),
            ASKS_NOTHING.len(),
            "a command is listed twice"
        );
        let wrong: Vec<String> = all
            .iter()
            .filter(|(c, takes)| *takes == listed.contains(c.as_str()))
            .map(|(c, takes)| {
                if *takes {
                    format!("`{c}` takes --yes but is listed as asking nothing")
                } else {
                    format!("`{c}` takes no --yes and is not listed as asking nothing")
                }
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// The operations of the spec's §3.2, §3.5 and §3.6 tables (the first cell of each row).
    fn spec_operations() -> BTreeSet<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/spec/engine-api.md");
        let spec = std::fs::read_to_string(path).expect("docs/spec/engine-api.md");
        let mut ops = BTreeSet::new();
        let mut on = false;
        for line in spec.lines() {
            if line.starts_with("### ") {
                on = ["### 3.2 ", "### 3.5 ", "### 3.6 "]
                    .iter()
                    .any(|h| line.starts_with(h));
                continue;
            }
            if !on || !line.starts_with("| `") {
                continue;
            }
            let first = line.split('|').nth(1).unwrap_or_default();
            for code in first.split('`').skip(1).step_by(2) {
                let name: String = code
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    ops.insert(name);
                }
            }
        }
        ops
    }

    /// `line` parses as far as its subcommands and flags go: every leading word is a
    /// subcommand of the one before, and every `--flag` an argument of the last one.
    fn parses(line: &str) {
        let root = Cli::command();
        let mut cur = &root;
        let mut depth = 0;
        let mut positional = false;
        for word in line.split_whitespace() {
            if let Some(flag) = word.strip_prefix("--") {
                let flag = flag.split('=').next().unwrap_or_default();
                let known = cur
                    .get_arguments()
                    .chain(root.get_arguments())
                    .any(|a| a.get_long() == Some(flag));
                assert!(
                    known,
                    "{line}: `{word}` is no option of `{}`",
                    cur.get_name()
                );
                continue;
            }
            if let Some(flag) = word.strip_prefix('-')
                && let Some(flag) = flag.chars().next()
            {
                let known = cur
                    .get_arguments()
                    .chain(root.get_arguments())
                    .any(|a| a.get_short() == Some(flag));
                assert!(
                    known,
                    "{line}: `{word}` is no option of `{}`",
                    cur.get_name()
                );
                continue;
            }
            if depth > 0
                && positional
                && !word.starts_with('-')
                && cur.find_subcommand(word).is_none()
            {
                // a value
                continue;
            }
            match cur.find_subcommand(word) {
                Some(sub) => {
                    cur = sub;
                    depth += 1;
                    positional = false;
                }
                None if depth > 0 && cur.has_subcommands() => {
                    panic!("{line}: `{word}` is no subcommand of `{}`", cur.get_name())
                }
                None => {
                    positional = true;
                }
            }
        }
        assert!(depth > 0, "{line}: no command");
    }

    #[test]
    fn discovery_and_backlink_commands_validate_arguments() {
        use clap::Parser;

        assert!(Cli::try_parse_from(["lessdugin", "discover"]).is_ok());
        assert!(Cli::try_parse_from(["lessdugin", "discover", "rust"]).is_ok());
        assert!(Cli::try_parse_from(["lessdugin", "discover", "rust", "extra"]).is_err());
        assert!(Cli::try_parse_from(["lessdugin", "linked"]).is_err());
        assert!(Cli::try_parse_from(["lessdugin", "linked", "at://did:plc:a/x.y/z"]).is_ok());
    }

    #[test]
    fn every_engine_operation_has_a_way_in() {
        let spec = spec_operations();
        assert!(spec.len() > 40, "the spec's tables were not read: {spec:?}");
        let reach: BTreeSet<String> = REACH.iter().map(|(op, _)| (*op).to_owned()).collect();
        assert_eq!(REACH.len(), reach.len(), "an operation is listed twice");
        let missing: Vec<&String> = spec.difference(&reach).collect();
        assert!(
            missing.is_empty(),
            "not reachable from the CLI: {missing:?}"
        );
        let stale: Vec<&String> = reach.difference(&spec).collect();
        assert!(stale.is_empty(), "not operations of the spec: {stale:?}");
        for (op, r) in REACH {
            match r {
                Reach::Command(line) => parses(line),
                Reach::Automatic(how) => assert!(!how.is_empty(), "{op}: say how"),
                // every operation an M3 client must offer is a command
                Reach::NotHere(why) => assert!(
                    why.starts_with("M5") || op.contains("draft"),
                    "{op} must be reachable: {why}"
                ),
            }
        }
    }
}
