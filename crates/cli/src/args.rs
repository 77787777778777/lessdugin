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

    /// Do not use the network: read the local cache only; writes stay queued.
    #[arg(long, global = true)]
    pub offline: bool,
}

/// Commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read a post and its comments (labelled c1…cN; no login needed).
    Read(ReadArgs),
    /// List a forum's posts, numbered (`read 3` reads the third).
    Ls(LsArgs),
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
        ("forum_discover", Reach::NotHere("M5 (engine-api.md §3.9)")),
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
        ("wiki_edit", Reach::NotHere("M5 (engine-api.md §3.9)")),
        ("seq_create", Reach::NotHere("M5 (engine-api.md §3.9)")),
        ("seq_edit", Reach::NotHere("M5 (engine-api.md §3.9)")),
        ("seq_add", Reach::NotHere("M5 (engine-api.md §3.9)")),
        ("seq_remove", Reach::NotHere("M5 (engine-api.md §3.9)")),
        ("seq_move", Reach::NotHere("M5 (engine-api.md §3.9)")),
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
            if depth > 0 && !word.starts_with('-') && cur.find_subcommand(word).is_none() {
                // a value
                continue;
            }
            match cur.find_subcommand(word) {
                Some(sub) => {
                    cur = sub;
                    depth += 1;
                }
                None => panic!("{line}: `{word}` is no subcommand of `{}`", cur.get_name()),
            }
        }
        assert!(depth > 0, "{line}: no command");
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
