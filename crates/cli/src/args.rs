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
    /// Drafts kept in the data directory: ls, edit, rm, publish.
    Draft(DraftArgs),
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
