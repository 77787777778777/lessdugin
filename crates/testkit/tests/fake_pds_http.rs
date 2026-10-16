// SPDX-License-Identifier: AGPL-3.0-or-later
//! `FakeNet` served over real HTTP (feature `http`), as CLI tests use it.

#![cfg(feature = "http")]

use lessdugin_atp::proof::{ProofCaps, verify_record_proof};
use lessdugin_atp::syntax::{Nsid, RecordKey};
use lessdugin_testkit::fake_pds::{FakeNet, Fault, SystemClock};
use serde_json::{Value, json};

const POST: &str = "online.7778777.lessdugin.post";

#[tokio::test]
async fn serves_pds_plc_and_relay_over_http() {
    let net = FakeNet::serve_http(SystemClock);
    let alice = net.add_account("alice.test");
    let base = alice.pds_base();
    assert!(base.starts_with("http://127.0.0.1:"), "{base}");
    assert!(net.plc().starts_with("http://127.0.0.1:"));
    assert_ne!(net.plc(), base);
    let c = reqwest::Client::new();

    // the DID document points at the listener
    let doc: Value = c
        .get(format!("{}/{}", net.plc(), alice.did()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(doc["service"][0]["serviceEndpoint"], base);

    // log in with the app password and write with the bearer token
    let r = c
        .post(format!("{base}/xrpc/com.atproto.server.createSession"))
        .json(&json!({"identifier": "alice.test", "password": alice.app_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    let session: Value = r.json().await.unwrap();
    let token = session["accessJwt"].as_str().unwrap();
    let r = c
        .post(format!("{base}/xrpc/com.atproto.repo.putRecord"))
        .bearer_auth(token)
        .json(&json!({"repo": alice.did().as_str(), "collection": POST, "rkey": "p1", "record": {"$type": POST, "n": 1}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let put: Value = r.json().await.unwrap();
    assert!(put["commit"]["rev"].is_string());

    // read it back and prove it
    let list: Value = c
        .get(format!(
            "{base}/xrpc/com.atproto.repo.listRecords?repo={}&collection={POST}",
            alice.did()
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["records"][0]["value"]["n"], 1);
    assert_eq!(list["cursor"], "p1");
    let car = c
        .get(format!(
            "{base}/xrpc/com.atproto.sync.getRecord?did={}&collection={POST}&rkey=p1",
            alice.did()
        ))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    #[allow(clippy::disallowed_methods)]
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    let out = verify_record_proof(
        &car,
        alice.did(),
        &Nsid::parse(POST).unwrap(),
        &RecordKey::parse("p1").unwrap(),
        &alice.public_key(),
        now_us,
        &ProofCaps::default(),
    )
    .unwrap();
    assert_eq!(out.record_cid().unwrap().to_string(), put["cid"]);

    // the relay
    let relay = &net.relays()[0];
    let repos: Value = c
        .get(format!(
            "{relay}/xrpc/com.atproto.sync.listReposByCollection?collection={POST}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(repos["repos"], json!([{"did": alice.did().as_str()}]));

    // a second PDS gets its own listener
    let pds2 = net.add_pds("second");
    assert_ne!(pds2.base(), base);
    let bob = pds2.add_account("bob.test");
    let r = c
        .get(format!(
            "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
            pds2.base(),
            bob.did()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // faults: truncation and disconnects fail the body; statuses and callers
    // (the client sees the dropped connection either before or while reading the body)
    let commit_url = format!(
        "{base}/xrpc/com.atproto.sync.getLatestCommit?did={}",
        alice.did()
    );
    for rule in [Fault::truncate(3).once(), Fault::disconnect().once()] {
        alice.inject(rule);
        match c.get(&commit_url).send().await {
            Ok(r) => assert!(r.bytes().await.is_err()),
            Err(e) => assert!(e.is_request() || e.is_body(), "{e}"),
        }
    }
    assert_eq!(c.get(&commit_url).send().await.unwrap().status(), 200);
    net.inject(Fault::status(503).for_caller("cli").once());
    let r = c
        .get(format!(
            "{base}/xrpc/com.atproto.sync.getLatestCommit?did={}",
            alice.did()
        ))
        .header("x-fake-caller", "cli")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    let log = net.requests();
    assert!(log.iter().any(|r| r.caller == "cli" && r.status == 503));
    assert!(
        log.iter()
            .any(|r| r.caller == "http" && r.auth.as_deref() == Some(alice.did().as_str()))
    );
}
