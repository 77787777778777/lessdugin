// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared fixtures for the engine integration tests (offline, over `MemHost`).

#![allow(dead_code)]

use std::future::Future;

use lessdugin_core::engine::{Engine, EngineConfig, Sources};
use lessdugin_testkit::identity::{DOH, PLC};
use lessdugin_testkit::repo::tid_at_ms;
use lessdugin_testkit::{FakeIdentity, FakePds, MemHost};
use serde_json::json;

pub const PDS: &str = "https://pds.example.com";
pub const POST: &str = "online.7778777.lessdugin.post";
pub const RKEY: &str = "3l2s5xxv2zaa2";
pub const SLINGSHOT: &str = "https://slingshot.microcosm.blue";

pub fn block_on<F: Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

pub fn engine(host: &MemHost) -> Engine<MemHost> {
    Engine::new(host.clone(), EngineConfig::default())
}

pub fn direct_engine(host: &MemHost) -> Engine<MemHost> {
    Engine::new(
        host.clone(),
        EngineConfig {
            sources: Sources::Direct,
            ..EngineConfig::default()
        },
    )
}

pub fn alice() -> FakeIdentity {
    FakeIdentity::plc("alice", "alice.example.com", PDS)
}

pub fn bob() -> FakeIdentity {
    FakeIdentity::plc("bob", "bob.example.com", PDS)
}

/// alice's identity (PLC doc + DoH TXT) and a PDS hosting her repo with one post, committed a
/// minute ago.
pub struct World {
    pub host: MemHost,
    pub alice: FakeIdentity,
    pub pds: FakePds,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        let host = MemHost::new();
        let alice = alice();
        alice.install(&host, PLC, DOH);
        let pds = FakePds::install(&host, PDS);
        let mut repo = alice.repo();
        repo.put(
            POST,
            RKEY,
            &json!({"$type": POST, "title": "hello", "n": 1}),
        );
        repo.put(
            POST,
            "3l2s5xxv2zaa3",
            &json!({"$type": POST, "title": "second"}),
        );
        pds.add_repo(repo, tid_at_ms(host.now() - 60_000));
        Self { host, alice, pds }
    }

    pub fn uri(&self, rkey: &str) -> String {
        format!("at://{}/{POST}/{rkey}", self.alice.did)
    }
}

pub fn assert_send<T: Send>(_: &T) {}
