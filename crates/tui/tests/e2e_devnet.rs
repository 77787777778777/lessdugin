// SPDX-License-Identifier: AGPL-3.0-or-later
//! End to end over the native host against the local devnet (`packages/e2e/devnet.mjs`, ADR
//! 0006): the parity scenario's TUI step (`docs/plan.md` §4.2, step 3, text subset). alice
//! creates an approval-mode forum, grants carol and posts; carol replies with the TUI through
//! `$EDITOR` (the headless driver); in parity mode the Thread screen's `ThreadView` is then
//! byte-identical on alice's and carol's devices.
//!
//! Runs only with `LESSDUGIN_E2E=1`. `LESSDUGIN_DEVNET_JSON=<file>` reuses a running devnet;
//! otherwise the test starts one and kills it at the end.
//!
//! This is also the recipe for the parity scenario: open an engine with
//! `NativeHost::open(paths.for_devnet(..), config)` and `parity_now_ms`, log in with
//! `login_app_password`, then [`lessdugin_tui::capture_thread`].

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

mod common;

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use lessdugin_core::dto::{
    DefaultTier, ForumDraft, MembershipMode, PostDraft, SyncOpts, ThreadRoot, WriteState,
};
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_native::auth::{AppPassword, SecretStore, login_app_password, resolve_login_target};
use lessdugin_native::config::{Config, DevnetProfile, NetworkProfile, Paths};
use lessdugin_native::engine::engine_config;
use lessdugin_native::host::NativeHost;
use lessdugin_tui::{CaptureOpts, Headless, HeadlessOpts, capture_thread};
use serde_json::Value;

struct Devnet {
    info: Value,
    child: Option<Child>,
}

impl Drop for Devnet {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Devnet {
    fn start() -> Option<Self> {
        if std::env::var("LESSDUGIN_E2E").ok().as_deref() != Some("1") {
            eprintln!("skipped: set LESSDUGIN_E2E=1 to run against the devnet");
            return None;
        }
        if let Ok(path) = std::env::var("LESSDUGIN_DEVNET_JSON") {
            let info = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            return Some(Self { info, child: None });
        }
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let json =
            std::env::temp_dir().join(format!("lessdugin-tui-devnet-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&json);
        let child = Command::new("node")
            .arg(format!("{root}/packages/e2e/devnet.mjs"))
            .arg("--json")
            .arg(&json)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("node");
        let mut devnet = Self {
            info: Value::Null,
            child: Some(child),
        };
        for _ in 0..600 {
            let parsed = std::fs::read_to_string(&json)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if let Some(v) = parsed {
                devnet.info = v;
                let _ = std::fs::remove_file(&json);
                return Some(devnet);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the devnet did not start within 60 s");
    }

    fn s(&self, path: &[&str]) -> String {
        let mut v = &self.info;
        for p in path {
            v = &v[*p];
        }
        v.as_str().unwrap_or_default().to_owned()
    }
}

/// One device: its own data directory, the devnet's engine in parity mode, logged in.
struct Device {
    engine: Engine<NativeHost>,
    did: String,
    _dir: tempfile::TempDir,
}

async fn device(devnet: &Devnet, name: &str, parity_now_ms: i64) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let profile = DevnetProfile {
        plc: url::Url::parse(&devnet.s(&["plc"])).unwrap(),
        pds: url::Url::parse(&devnet.s(&["pds"])).unwrap(),
        jetstream: None,
    };
    let config = Config {
        plc: profile.plc.clone(),
        network: NetworkProfile::Devnet(profile.clone()),
        ..Config::default()
    };
    let paths = Paths::under(dir.path()).for_devnet(&profile);
    let host = NativeHost::open(&paths, &config).unwrap();
    let engine = Engine::new(
        host,
        EngineConfig {
            parity_now_ms: Some(parity_now_ms),
            ..engine_config(&config)
        },
    );
    // a keyring service of this test's own, so a real keyring keeps nothing of the user's
    let store = SecretStore::new(&paths).with_keyring_service(format!(
        "{}-tui-e2e-{}",
        profile.keyring_service(),
        std::process::id()
    ));
    let target = resolve_login_target(&engine, &devnet.s(&["accounts", name, "handle"]))
        .await
        .unwrap();
    let pw = AppPassword::new(devnet.s(&["accounts", name, "appPassword"])).unwrap();
    let out = login_app_password(&engine, &store, &paths, &target, &pw)
        .await
        .unwrap();
    Device {
        did: out.account.did.clone(),
        engine,
        _dir: dir,
    }
}

async fn flush(d: &Device) {
    let r = d.engine.outbox_flush().await.unwrap();
    assert_eq!(
        (r.failed, r.conflicts),
        (0, 0),
        "{:?}",
        d.engine.writes().await
    );
}

async fn done_uri(d: &Device, id: &str) -> String {
    match d.engine.write_status(id).await.unwrap().unwrap().state {
        WriteState::Done { uri, .. } => uri,
        other => panic!("write {id} is {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn carol_replies_with_the_tui_and_the_views_match() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    // the scenario's start plus a day (`engine-api.md` §2.1)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let t = i64::try_from(now).unwrap() + 86_400_000;
    let alice = device(&devnet, "alice", t).await;
    let carol = device(&devnet, "carol", t).await;

    // step 1 (alice): an approval-mode forum; carol granted; a post
    let slug = format!("tui{}", std::process::id());
    let mut draft = ForumDraft::new(&slug, "TUI e2e");
    draft.membership = MembershipMode::Approval;
    draft.default_tier = DefaultTier::Frontpage;
    let w = alice.engine.forum_create(&draft).await.unwrap();
    flush(&alice).await;
    let forum = done_uri(&alice, &w.id).await;
    alice
        .engine
        .grant(&forum, std::slice::from_ref(&carol.did))
        .await
        .unwrap();
    let w = alice
        .engine
        .publish(&PostDraft::article(
            &forum,
            "Welcome",
            "Hello **everyone**.",
        ))
        .await
        .unwrap();
    flush(&alice).await;
    let post = done_uri(&alice, &w.id).await;

    // step 3 (carol, TUI): open the thread, `c`, write in $EDITOR, sent by the outbox
    let tmp = tempfile::tempdir().unwrap();
    let script = common::editor_script(tmp.path(), "Hello from the TUI.");
    let mut tui = Headless::new(
        carol.engine.clone(),
        HeadlessOpts {
            width: 100,
            height: 30,
            editor: Some(common::editor(tmp.path(), &script)),
            ..HeadlessOpts::default()
        },
    )
    .await
    .unwrap();
    tui.start(Some(forum.clone())).await;
    tui.open_thread(&post).await;
    tui.press("c").await;
    let screen = tui.render();
    assert!(screen.contains("Hello from the TUI."), "{screen}");
    let held = tui.thread_view().unwrap().clone();
    let reply = held
        .comments
        .iter()
        .find(|c| c.author.did == carol.did)
        .expect("carol's comment");
    assert!(!reply.badges.pending);

    // both devices synced: the screen holds the same view, byte for byte
    let opts = SyncOpts::default();
    alice.engine.sync_forum(&forum, &opts).await.unwrap();
    carol.engine.sync_forum(&forum, &opts).await.unwrap();
    let copts = CaptureOpts {
        width: 100,
        height: 30,
        sync: false,
    };
    let a = capture_thread(&alice.engine, &post, copts).await.unwrap();
    let c = capture_thread(&carol.engine, &post, copts).await.unwrap();
    let ThreadRoot::Post(p) = &a.view.post else {
        panic!("the post is missing")
    };
    assert_eq!(p.post.uri, post);
    assert_eq!(a.view.comments.len(), 1, "{}", a.json);
    assert_eq!(a.json, c.json);
    assert_eq!(a.document, c.document);
}
