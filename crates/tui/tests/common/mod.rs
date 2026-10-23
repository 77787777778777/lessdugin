// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared fixtures for the TUI tests: three users on three devices over one fake network
//! (`MemHost` + `FakeNet`), as in `crates/core/tests/engine_m2.rs`. carol drives the TUI.

#![allow(dead_code)]

use std::future::Future;
use std::path::{Path, PathBuf};

use lessdugin_core::dto::{
    BodyDraft, CommentDraft, DefaultTier, ForumDraft, PostDraft, SyncOpts, WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig};
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost};
use lessdugin_tui::{EditorConfig, Headless, HeadlessOpts};

pub const MINUTE: i64 = 60_000;

pub fn block_on<F: Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

pub struct User {
    pub host: MemHost,
    pub engine: Engine<MemHost>,
    pub acct: AccountHandle,
}

impl User {
    fn new(net: &FakeNet, host: MemHost, handle: &str, config: EngineConfig) -> Self {
        let acct = net.add_account(handle);
        let engine = Engine::new(host.clone(), config);
        block_on(engine.add_account(AccountInfo::app_password(
            acct.did().as_str(),
            acct.pds_base(),
        )))
        .unwrap();
        Self { host, engine, acct }
    }

    pub fn did(&self) -> String {
        self.acct.did().to_string()
    }

    /// Runs the outbox until idle; every write must be done.
    pub fn flush(&self) {
        let r = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!((r.failed, r.conflicts), (0, 0), "{}", self.acct.handle());
    }

    /// The uri a done write created.
    pub fn done(&self, t: &WriteTicket) -> String {
        match block_on(self.engine.write_status(&t.id))
            .unwrap()
            .unwrap()
            .state
        {
            WriteState::Done { uri, .. } => uri,
            other => panic!("write {} is {other:?}", t.id),
        }
    }

    pub fn sync(&self, forum: &str) {
        let r = block_on(self.engine.sync_forum(forum, &SyncOpts::default())).unwrap();
        assert!(!r.partial, "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
    }
}

pub struct World {
    pub alice: User,
    pub bob: User,
    pub carol: User,
}

impl World {
    pub fn new() -> Self {
        Self::with_config(EngineConfig::default())
    }

    pub fn with_config(config: EngineConfig) -> Self {
        let ha = MemHost::new();
        let net = FakeNet::on(&ha);
        let _ = net.default_pds();
        let hb = MemHost::new();
        hb.set_seed(2);
        net.install_as(&hb, "bob");
        let hc = MemHost::new();
        hc.set_seed(3);
        net.install_as(&hc, "carol");
        Self {
            alice: User::new(&net, ha, "alice.example.com", config.clone()),
            bob: User::new(&net, hb, "bob.example.com", config.clone()),
            carol: User::new(&net, hc, "carol.example.com", config),
        }
    }

    /// Moves every device's clock (the network runs on alice's).
    pub fn tick(&self, ms: i64) {
        for u in [&self.alice, &self.bob, &self.carol] {
            u.host.advance_ms(ms);
        }
    }
}

pub struct Scenario {
    pub w: World,
    pub forum: String,
    pub welcome: String,
    pub bob_post: String,
    pub bob_c1: String,
}

pub const WELCOME_MD: &str = "Hello **everyone**, and welcome to the forum.\n\n\
## Rules\n\n\
- Be kind.\n\
- Stay on topic; see the [guide](https://example.com/guide).\n\n\
```\nlessdugin tui\n```\n";

/// alice's approval-mode forum (bob and carol granted) with her welcome post; bob's post and
/// his comment on the welcome. carol follows the forum and has synced it.
pub fn scenario() -> Scenario {
    scenario_with(EngineConfig::default())
}

pub fn scenario_with(config: EngineConfig) -> Scenario {
    let w = World::with_config(config);
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);
    let mut draft = ForumDraft::new("meta", "Meta");
    draft.description = Some("Talk about the forum.".into());
    draft.about = Some(BodyDraft::text("# Rules\n\nBe kind."));
    draft.default_tier = DefaultTier::Frontpage;
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    block_on(alice.engine.grant(&forum, &[bob.did(), carol.did()])).unwrap();
    alice.flush();
    w.tick(MINUTE);
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "Welcome", WELCOME_MD)),
    )
    .unwrap();
    alice.flush();
    let welcome = alice.done(&t);

    w.tick(MINUTE);
    block_on(bob.engine.forum_open(&forum)).unwrap();
    block_on(bob.engine.join(&forum, Some("hi"))).unwrap();
    bob.flush();
    bob.sync(&forum);
    w.tick(MINUTE);
    let t = block_on(bob.engine.publish(&PostDraft::article(
        &forum,
        "Hello from bob",
        "My first post.",
    )))
    .unwrap();
    bob.flush();
    let bob_post = bob.done(&t);
    w.tick(MINUTE);
    let t = block_on(bob.engine.comment(&CommentDraft::new(
        &welcome,
        None,
        "Thanks for the forum!\n\nIt looks *great*.",
    )))
    .unwrap();
    bob.flush();
    let bob_c1 = bob.done(&t);

    w.tick(MINUTE);
    block_on(carol.engine.forum_follow(&forum, None)).unwrap();
    carol.sync(&forum);
    Scenario {
        w,
        forum,
        welcome,
        bob_post,
        bob_c1,
    }
}

/// An executable script that writes `text` above the template (an `$EDITOR` stand-in).
pub fn editor_script(dir: &Path, text: &str) -> PathBuf {
    let text_file = dir.join("reply.txt");
    std::fs::write(&text_file, text).unwrap();
    let script = dir.join("fake-editor.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nset -e\n{{ cat '{}'; printf '\\n'; cat \"$1\"; }} > \"$1.new\"\nmv \"$1.new\" \"$1\"\n",
            text_file.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

/// The editor configuration with `$EDITOR` set to `script`.
pub fn editor(dir: &Path, script: &Path) -> EditorConfig {
    let s = script.display().to_string();
    EditorConfig::from_vars(dir.join("templates"), move |k| {
        (k == "EDITOR").then(|| s.clone())
    })
}

/// carol's TUI at `width` × `height`.
pub fn tui(s: &Scenario, width: u16, height: u16) -> Headless<MemHost> {
    block_on(Headless::new(
        s.w.carol.engine.clone(),
        HeadlessOpts {
            width,
            height,
            ..HeadlessOpts::default()
        },
    ))
    .unwrap()
}
