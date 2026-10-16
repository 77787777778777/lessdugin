// SPDX-License-Identifier: AGPL-3.0-or-later
//! `FakeNet` over `MemHost`: proofs verify with `lessdugin_atp::proof` and the engine,
//! `listRecords` cursor semantics (ADR 0004), write and swap semantics (ADR 0005), sessions,
//! blobs, faults, account states, equivocation, relays, PLC and DoH.

use futures::executor::block_on;
use lessdugin_atp::Cid;
use lessdugin_atp::proof::{ProofCaps, ProofError, verify_record_proof};
use lessdugin_atp::syntax::{Did, Nsid, RecordKey, Tid};
use lessdugin_core::dto::VerifyOutcome;
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_core::host::{Host, HostError, HttpRequest, HttpResponse, Target};
use lessdugin_core::xrpc::{CallOpts, Service, Xrpc, XrpcCallError};
use lessdugin_testkit::MemHost;
use lessdugin_testkit::fake_pds::{
    AccountHandle, AccountStatus, CursorMode, DEFAULT_PDS, FakeNet, Fault, ForkSelector, OpAction,
    RateLimit, RepoEventKind, tiny_png,
};
use serde_json::{Value, json};

const POST: &str = "online.7778777.lessdugin.post";
const OPINION: &str = "online.7778777.lessdugin.opinion";
const REVISION: &str = "online.7778777.lessdugin.revision";

struct W {
    host: MemHost,
    net: FakeNet,
}

fn world() -> W {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    W { host, net }
}

fn url(s: &str) -> url::Url {
    url::Url::parse(s).unwrap()
}

fn json_of(r: &HttpResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

impl W {
    fn send(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        block_on(self.host.http(req))
    }

    fn get(&self, u: &str) -> (u16, Value) {
        let r = self.send(HttpRequest::get(Target::Anon(url(u)))).unwrap();
        (r.status, json_of(&r))
    }

    fn xrpc_get(&self, acct: &AccountHandle, nsid: &str, qs: &[(&str, &str)]) -> (u16, Value) {
        let q = lessdugin_atp::xrpc::query_string(qs);
        self.get(&format!("{}/xrpc/{nsid}?{q}", acct.pds_base()))
    }

    fn post(&self, base: &str, nsid: &str, body: &Value, token: Option<&str>) -> (u16, Value) {
        let mut req = HttpRequest::post_json(
            Target::Anon(url(&format!("{base}/xrpc/{nsid}"))),
            serde_json::to_vec(body).unwrap(),
        );
        if let Some(t) = token {
            req = req.with_header("authorization", &format!("Bearer {t}"));
        }
        let r = self.send(req).unwrap();
        (r.status, json_of(&r))
    }

    /// A write through `Target::UserPds` (authenticated by the host).
    fn write(&self, acct: &AccountHandle, nsid: &str, body: &Value) -> (u16, Value) {
        let req = HttpRequest::post_json(
            Target::UserPds {
                account: acct.did().clone(),
                path_and_query: format!("/xrpc/{nsid}"),
            },
            serde_json::to_vec(body).unwrap(),
        );
        let r = self.send(req).unwrap();
        (r.status, json_of(&r))
    }

    fn head(&self, acct: &AccountHandle) -> (String, String) {
        let (s, v) = self.xrpc_get(
            acct,
            "com.atproto.sync.getLatestCommit",
            &[("did", acct.did().as_str())],
        );
        assert_eq!(s, 200, "{v}");
        (
            v["cid"].as_str().unwrap().to_owned(),
            v["rev"].as_str().unwrap().to_owned(),
        )
    }

    fn proof(
        &self,
        acct: &AccountHandle,
        coll: &str,
        rkey: &str,
    ) -> Result<Option<Cid>, ProofError> {
        let q = lessdugin_atp::xrpc::query_string(&[
            ("did", acct.did().as_str()),
            ("collection", coll),
            ("rkey", rkey),
        ]);
        let r = self
            .send(HttpRequest::get(Target::Anon(url(&format!(
                "{}/xrpc/com.atproto.sync.getRecord?{q}",
                acct.pds_base()
            )))))
            .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.header("content-type"), Some("application/vnd.ipld.car"));
        verify_record_proof(
            &r.body,
            acct.did(),
            &Nsid::parse(coll).unwrap(),
            &RecordKey::parse(rkey).unwrap(),
            &acct.public_key(),
            self.host.now() * 1000,
            &ProofCaps::default(),
        )
        .map(|o| o.record_cid().copied())
    }

    fn list(&self, acct: &AccountHandle, coll: &str, extra: &[(&str, &str)]) -> (u16, Value) {
        let mut qs = vec![("repo", acct.did().as_str()), ("collection", coll)];
        qs.extend_from_slice(extra);
        self.xrpc_get(acct, "com.atproto.repo.listRecords", &qs)
    }
}

fn rkeys(v: &Value) -> Vec<String> {
    v["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["uri"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn post(n: i64) -> Value {
    json!({"$type": POST, "title": format!("post {n}"), "n": n})
}

// ---------------------------------------------------------------------------------------------
// Proofs

#[test]
fn proofs_verify_with_atp_for_presence_and_absence() {
    let w = world();
    let alice = w.net.add_account("alice.example.com");
    alice.batch(|b| {
        for i in 0..150 {
            b.put(POST, &format!("aaaaaaaaaaaa-3l2s5xxv2z{i:03}"), &post(i));
        }
    });
    alice.put(
        "online.7778777.lessdugin.profile",
        "self",
        &json!({"$type": "online.7778777.lessdugin.profile"}),
    );
    for i in [0, 75, 149] {
        let rkey = format!("aaaaaaaaaaaa-3l2s5xxv2z{i:03}");
        let cid = w.proof(&alice, POST, &rkey).unwrap();
        assert_eq!(cid, alice.cid(POST, &rkey));
    }
    for rkey in ["aaaaaaaaaaaa-0", "aaaaaaaaaaaa-3l2s5xxv2z000x", "zzzz"] {
        assert_eq!(w.proof(&alice, POST, rkey).unwrap(), None);
    }
    // an empty repo proves absence too
    let bob = w.net.add_account("bob.example.com");
    assert_eq!(w.proof(&bob, POST, "x").unwrap(), None);
    // the commit served by getLatestCommit is the proof's commit, and its rev is from the clock
    let (cid, rev) = w.head(&alice);
    assert_eq!(cid, alice.head().0.to_string());
    let rev = Tid::parse(&rev).unwrap();
    assert_eq!(rev.timestamp_micros() / 1000, w.host.now() as u64);
    // a commit signed by the wrong key fails with a bad signature
    alice.resign_with(&lessdugin_testkit::identity::fake_key("mallory"));
    let err = w
        .proof(&alice, POST, "aaaaaaaaaaaa-3l2s5xxv2z000")
        .unwrap_err();
    assert!(
        matches!(
            err,
            ProofError::Commit(lessdugin_atp::commit::CommitError::Signature(_))
        ),
        "{err:?}"
    );
    // the next write is signed by the account key again
    alice.put(POST, "aaaaaaaaaaaa-3l2s5xxv2zzzz", &post(9));
    assert!(
        w.proof(&alice, POST, "aaaaaaaaaaaa-3l2s5xxv2zzzz")
            .unwrap()
            .is_some()
    );
}

#[test]
fn the_engine_verifies_records_end_to_end() {
    let w = world();
    let alice = w.net.add_account("alice.example.com");
    alice.put(POST, "3l2s5xxv2zaa2", &post(1));
    w.host.advance_ms(60_000);
    let e = Engine::new(w.host.clone(), EngineConfig::default());
    let r = block_on(e.verify(&alice.uri(POST, "3l2s5xxv2zaa2"), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Proven, "{:?}", r.checks);
    assert_eq!(r.rev.as_deref(), Some(alice.head().1.as_str()));
    let r = block_on(e.verify(&alice.uri(POST, "3l2s5xxv2zaa3"), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Absent, "{:?}", r.checks);
    // the handle resolves through the fake DoH and verifies against the PLC document
    let id = block_on(e.resolve_identity("alice.example.com")).unwrap();
    assert_eq!(id.identity.did, *alice.did(), "{id:?}");
    assert_eq!(
        id.identity.handle_state,
        lessdugin_core::dto::HandleState::Verified
    );
    assert!(w.net.count("plc") >= 1 && w.net.count("doh") >= 1);
    // after a key rotation the old commit no longer verifies; the next commit does
    alice.rotate_key("alice-2");
    let e = Engine::new(w.host.clone(), EngineConfig::default());
    let r = block_on(e.verify(&alice.uri(POST, "3l2s5xxv2zaa2"), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    alice.commit_now();
    let r = block_on(e.verify(&alice.uri(POST, "3l2s5xxv2zaa2"), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Proven, "{:?}", r.checks);
    assert!(w.host.unmatched().is_empty(), "{:?}", w.host.unmatched());
}

// ---------------------------------------------------------------------------------------------
// listRecords (ADR 0004)

const ADR4_KEYS: [&str; 10] = [
    "aaaaaaaaaaa7-3mxygf7onkc2b",
    "aaaaaaaaaaaa",
    "aaaaaaaaaaaa-3mxygf7onkd2b",
    "aaaaaaaaaaaa-3mxygf7onke2b",
    "aaaaaaaaaaaa-3mxygf7onkf2b-3mxygf7onkg2b",
    "aaaaaaaaaaaa-~-zzzzzzzzzzzzzzzz",
    "aaaaaaaaaaaa.x",
    "aaaaaaaaaaab-3mxygf7onkh2b",
    "bbbbbbbbbbbb-3mxygf7onki2b",
    "bbbbbbbbbbbb-3mxygf7onkj2b",
];

fn adr4(w: &W) -> AccountHandle {
    let a = w.net.add_account("alice.example.com");
    a.batch(|b| {
        for (i, k) in ADR4_KEYS.iter().enumerate() {
            b.put(POST, k, &post(i as i64));
        }
        b.put(
            "online.7778777.lessdugin.postx",
            "aaaaaaaaaaaa-x",
            &json!({"$type": "online.7778777.lessdugin.postx"}),
        );
    });
    a
}

#[test]
fn list_records_matches_the_reference_cursor_behaviour() {
    let w = world();
    let a = adr4(&w);
    let desc: Vec<String> = ADR4_KEYS.iter().rev().map(|s| s.to_string()).collect();
    // default: bytewise descending, cursor = last key even on a short page
    let (s, v) = w.list(&a, POST, &[]);
    assert_eq!(s, 200);
    assert_eq!(rkeys(&v), desc);
    assert_eq!(v["cursor"], "aaaaaaaaaaa7-3mxygf7onkc2b");
    let r0 = &v["records"][0];
    assert_eq!(r0["uri"], a.uri(POST, "bbbbbbbbbbbb-3mxygf7onkj2b"));
    assert_eq!(
        r0["cid"],
        a.cid(POST, "bbbbbbbbbbbb-3mxygf7onkj2b")
            .unwrap()
            .to_string()
    );
    assert_eq!(r0["value"], post(9));
    // the follow-up page is empty and has no cursor
    let (_, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaa7-3mxygf7onkc2b")]);
    assert_eq!(v, json!({"records": []}));
    // reverse is exactly the reverse
    let (_, v) = w.list(&a, POST, &[("reverse", "true")]);
    let mut asc = desc.clone();
    asc.reverse();
    assert_eq!(rkeys(&v), asc);
    // the range trick: descending from "{fk}." isolates the {fk}- range
    let (_, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa.")]);
    assert_eq!(
        rkeys(&v),
        [
            "aaaaaaaaaaaa-~-zzzzzzzzzzzzzzzz",
            "aaaaaaaaaaaa-3mxygf7onkf2b-3mxygf7onkg2b",
            "aaaaaaaaaaaa-3mxygf7onke2b",
            "aaaaaaaaaaaa-3mxygf7onkd2b",
            "aaaaaaaaaaaa",
            "aaaaaaaaaaa7-3mxygf7onkc2b",
        ]
    );
    // "/" is a plain string bound too, and illegal rkey characters are not validated
    let (_, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa/")]);
    assert_eq!(rkeys(&v)[0], "aaaaaaaaaaaa.x");
    let (s, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa/#?")]);
    assert_eq!(s, 200);
    assert_eq!(rkeys(&v)[0], "aaaaaaaaaaaa.x");
    // paging with limit=2 from "{fk}.": nothing missed or duplicated
    let mut cursor = "aaaaaaaaaaaa.".to_owned();
    let mut pages = Vec::new();
    loop {
        let (_, v) = w.list(&a, POST, &[("limit", "2"), ("cursor", &cursor)]);
        let ks = rkeys(&v);
        if ks.is_empty() {
            assert!(v.get("cursor").is_none());
            break;
        }
        assert_eq!(v["cursor"].as_str().unwrap(), ks.last().unwrap());
        cursor = v["cursor"].as_str().unwrap().to_owned();
        pages.push(ks);
    }
    assert_eq!(
        pages[0],
        [
            "aaaaaaaaaaaa-~-zzzzzzzzzzzzzzzz",
            "aaaaaaaaaaaa-3mxygf7onkf2b-3mxygf7onkg2b"
        ]
    );
    assert_eq!(
        pages[1],
        ["aaaaaaaaaaaa-3mxygf7onke2b", "aaaaaaaaaaaa-3mxygf7onkd2b"]
    );
    assert_eq!(pages[2], ["aaaaaaaaaaaa", "aaaaaaaaaaa7-3mxygf7onkc2b"]);
    // ascending from "{fk}-" or "{fk}" skips the bare fk
    for c in ["aaaaaaaaaaaa-", "aaaaaaaaaaaa"] {
        let (_, v) = w.list(
            &a,
            POST,
            &[("reverse", "true"), ("cursor", c), ("limit", "1")],
        );
        assert_eq!(rkeys(&v), ["aaaaaaaaaaaa-3mxygf7onkd2b"]);
    }
    // a range with no keys lists from the top: the first key is already outside
    let (_, v) = w.list(&a, POST, &[("cursor", "cccccccccccc.")]);
    assert_eq!(rkeys(&v)[0], "bbbbbbbbbbbb-3mxygf7onkj2b");
    // empty cursor = none; default limit 50; limit bounds
    let (_, v) = w.list(&a, POST, &[("cursor", "")]);
    assert_eq!(rkeys(&v), desc);
    let (s, v) = w.list(&a, POST, &[("limit", "101")]);
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidRequest")));
    assert_eq!(
        v["message"],
        "Invalid com.atproto.repo.listRecords params: integer too big (maximum 100, got 101)"
    );
    let (s, _) = w.list(&a, POST, &[("limit", "0")]);
    assert_eq!(s, 400);
    let (s, v) = w.list(&a, "online.7778777.lessdugin.nothing", &[]);
    assert_eq!((s, v), (200, json!({"records": []})));
    // handles work as the repo parameter; uris use the DID
    let (_, v) = w.xrpc_get(
        &a,
        "com.atproto.repo.listRecords",
        &[
            ("repo", "alice.example.com"),
            ("collection", POST),
            ("limit", "1"),
        ],
    );
    assert_eq!(
        v["records"][0]["uri"],
        a.uri(POST, "bbbbbbbbbbbb-3mxygf7onkj2b")
    );
    let mut many = Vec::new();
    a.batch(|b| {
        for i in 0..120 {
            let k = format!("cccccccccccc-{i:03}");
            b.put(POST, &k, &post(i));
            many.push(k);
        }
    });
    let (_, v) = w.list(&a, POST, &[]);
    assert_eq!(rkeys(&v).len(), 50);
    // describeRepo lists the collections
    let (_, v) = w.xrpc_get(
        &a,
        "com.atproto.repo.describeRepo",
        &[("repo", a.did().as_str())],
    );
    assert_eq!(
        v["collections"],
        json!([POST, "online.7778777.lessdugin.postx"])
    );
    assert_eq!(v["handle"], "alice.example.com");
    assert_eq!(v["didDoc"]["id"], a.did().as_str());
}

#[test]
fn cursor_modes_break_the_range_trick_detectably() {
    let w = world();
    let a = adr4(&w);
    let pds = a.pds();

    // Opaque: cursors are tokens, unknown cursors list from the top, tokens page correctly
    pds.set_cursor_mode(CursorMode::Opaque);
    let (_, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa."), ("limit", "3")]);
    let ks = rkeys(&v);
    assert_eq!(ks[0], "bbbbbbbbbbbb-3mxygf7onkj2b");
    let c = v["cursor"].as_str().unwrap().to_owned();
    assert_ne!(&c, ks.last().unwrap());
    let mut all = ks;
    let mut cursor = c;
    loop {
        let (_, v) = w.list(&a, POST, &[("cursor", &cursor), ("limit", "3")]);
        let ks = rkeys(&v);
        if ks.is_empty() {
            break;
        }
        all.extend(ks);
        cursor = v["cursor"].as_str().unwrap().to_owned();
    }
    assert_eq!(all.len(), ADR4_KEYS.len());
    // a token from another direction or collection is unknown
    let (_, v) = w.list(
        &a,
        POST,
        &[("cursor", &cursor), ("reverse", "true"), ("limit", "1")],
    );
    assert_eq!(rkeys(&v), ["aaaaaaaaaaa7-3mxygf7onkc2b"]);

    pds.set_cursor_mode(CursorMode::OpaqueStrict);
    let (s, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa.")]);
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidRequest")));

    // IgnoresBound: "{fk}." is ignored (keys ≥ "{fk}." come first); server cursors still page
    pds.set_cursor_mode(CursorMode::IgnoresBound);
    let (_, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa."), ("limit", "2")]);
    assert_eq!(
        rkeys(&v),
        ["bbbbbbbbbbbb-3mxygf7onkj2b", "bbbbbbbbbbbb-3mxygf7onki2b"]
    );
    let (_, v) = w.list(
        &a,
        POST,
        &[("cursor", v["cursor"].as_str().unwrap()), ("limit", "2")],
    );
    assert_eq!(rkeys(&v), ["aaaaaaaaaaab-3mxygf7onkh2b", "aaaaaaaaaaaa.x"]);

    // Stuck: the cursor never advances
    pds.set_cursor_mode(CursorMode::Stuck);
    let (_, v1) = w.list(&a, POST, &[("limit", "2")]);
    let c1 = v1["cursor"].as_str().unwrap().to_owned();
    let (_, v2) = w.list(&a, POST, &[("limit", "2"), ("cursor", &c1)]);
    assert_eq!(v2["cursor"].as_str().unwrap(), c1);

    // a per-repo override wins over the PDS mode
    a.set_cursor_mode(Some(CursorMode::Bound));
    let (_, v) = w.list(&a, POST, &[("cursor", "aaaaaaaaaaaa."), ("limit", "1")]);
    assert_eq!(rkeys(&v), ["aaaaaaaaaaaa-~-zzzzzzzzzzzzzzzz"]);
    a.set_cursor_mode(None);

    // a page cap shortens pages (a non-empty page after a short one)
    pds.set_cursor_mode(CursorMode::Bound);
    pds.set_max_page(Some(3));
    let (_, v) = w.list(&a, POST, &[("limit", "100")]);
    assert_eq!(rkeys(&v).len(), 3);
    let (_, v) = w.list(
        &a,
        POST,
        &[("limit", "100"), ("cursor", v["cursor"].as_str().unwrap())],
    );
    assert_eq!(rkeys(&v).len(), 3);
}

// ---------------------------------------------------------------------------------------------
// Writes (ADR 0005)

#[test]
fn create_record_semantics() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let rkey = "aaaaaaaaaaaa-3mxygf7onkd2b-aaaaaaaaaaaaaaaa";
    let body = json!({"repo": a.did().as_str(), "collection": OPINION, "rkey": rkey, "record": {"$type": OPINION, "v": 1}});
    let (s, v) = w.write(&a, "com.atproto.repo.createRecord", &body);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["uri"], a.uri(OPINION, rkey));
    assert_eq!(v["validationStatus"], "unknown");
    let (head_cid, head_rev) = w.head(&a);
    assert_eq!(v["commit"], json!({"cid": head_cid, "rev": head_rev}));
    assert_eq!(v["cid"], a.cid(OPINION, rkey).unwrap().to_string());
    // again (with another value): 500, nothing written, head unchanged
    let mut again = body.clone();
    again["record"]["v"] = json!(2);
    let (s, v) = w.write(&a, "com.atproto.repo.createRecord", &again);
    assert_eq!((s, v["error"].as_str()), (500, Some("InternalServerError")));
    assert_eq!(a.get(OPINION, rkey).unwrap()["v"], 1);
    assert_eq!(w.head(&a), (head_cid.clone(), head_rev.clone()));
    // no rkey: a TID from the clock, above the head rev
    let (s, v) = w.write(
        &a,
        "com.atproto.repo.createRecord",
        &json!({"repo": a.did().as_str(), "collection": POST, "record": post(1)}),
    );
    assert_eq!(s, 200);
    let k = v["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    assert!(Tid::parse(k).is_ok());
    assert!(v["commit"]["rev"].as_str().unwrap() > head_rev.as_str());
    // $type injected when missing; mismatch, floats, validate=true and bad keys are 400s
    let (s, _) = w.write(&a, "com.atproto.repo.createRecord", &json!({"repo": a.did().as_str(), "collection": POST, "rkey": "notype", "record": {"x": 1}}));
    assert_eq!(s, 200);
    assert_eq!(a.get(POST, "notype").unwrap()["$type"], POST);
    for (record, rkey, validate) in [
        (
            json!({"$type": "online.7778777.lessdugin.comment"}),
            "a",
            None,
        ),
        (json!({"$type": POST, "score": 1.5}), "b", None),
        (json!({"$type": POST}), "c", Some(true)),
        (json!({"$type": POST}), "bad/rkey", None),
    ] {
        let mut b =
            json!({"repo": a.did().as_str(), "collection": POST, "rkey": rkey, "record": record});
        if let Some(v) = validate {
            b["validate"] = json!(v);
        }
        let (s, v) = w.write(&a, "com.atproto.repo.createRecord", &b);
        assert_eq!(
            (s, v["error"].as_str()),
            (400, Some("InvalidRequest")),
            "{b} → {v}"
        );
    }
    // validate=false drops validationStatus
    let (s, v) = w.write(&a, "com.atproto.repo.createRecord", &json!({"repo": a.did().as_str(), "collection": POST, "rkey": "nv", "validate": false, "record": post(2)}));
    assert_eq!(s, 200);
    assert!(v.get("validationStatus").is_none());
    // a stale swapCommit
    let (s, v) = w.write(&a, "com.atproto.repo.createRecord", &json!({"repo": a.did().as_str(), "collection": POST, "rkey": "sc", "swapCommit": head_cid, "record": post(3)}));
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidSwap")));
    assert_eq!(v["message"], format!("Commit was at {}", w.head(&a).0));
}

#[test]
fn put_and_delete_swap_semantics() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let did = a.did().as_str().to_owned();
    let put = |rkey: &str, v: i64, swap: Option<Value>| {
        let mut b = json!({"repo": did, "collection": OPINION, "rkey": rkey, "record": {"$type": OPINION, "v": v}});
        if let Some(s) = swap {
            b["swapRecord"] = s;
        }
        w.write(&a, "com.atproto.repo.putRecord", &b)
    };
    // swapRecord: null on an absent key creates
    let (s, v1) = put("k", 1, Some(Value::Null));
    assert_eq!(s, 200);
    let c1 = v1["cid"].as_str().unwrap().to_owned();
    // swapRecord: null on an existing key: InvalidSwap naming the current CID
    let (s, v) = put("k", 2, Some(Value::Null));
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidSwap")));
    assert_eq!(v["message"], format!("Record was at {c1}"));
    // swap = current CID updates
    let (s, v2) = put("k", 2, Some(json!(c1)));
    assert_eq!(s, 200);
    let c2 = v2["cid"].as_str().unwrap().to_owned();
    assert!(v2["commit"].is_object());
    // swap = stale CID
    let (s, v) = put("k", 3, Some(json!(c1)));
    assert_eq!(
        (s, v["message"].as_str()),
        (400, Some(format!("Record was at {c2}").as_str()))
    );
    // swap = some CID, key absent
    let (s, v) = put("absent", 3, Some(json!(c1)));
    assert_eq!(
        (s, v["message"].as_str()),
        (400, Some("Record was at null"))
    );
    // identical value (no swap, or swap = current): no commit, head unchanged
    let head = w.head(&a);
    for swap in [None, Some(json!(c2))] {
        let (s, v) = put("k", 2, swap);
        assert_eq!(s, 200);
        assert_eq!(v["cid"], c2);
        assert!(v.get("commit").is_none(), "{v}");
        assert_eq!(v["validationStatus"], "unknown");
    }
    assert_eq!(w.head(&a), head);
    // delete: wrong swap, right swap, absent (with and without a swap)
    let del = |rkey: &str, swap: Option<&str>| {
        let mut b = json!({"repo": did, "collection": OPINION, "rkey": rkey});
        if let Some(s) = swap {
            b["swapRecord"] = json!(s);
        }
        w.write(&a, "com.atproto.repo.deleteRecord", &b)
    };
    let (s, v) = del("k", Some(&c1));
    assert_eq!(
        (s, v["message"].as_str()),
        (400, Some(format!("Record was at {c2}").as_str()))
    );
    let (s, v) = del("k", Some(&c2));
    assert_eq!(s, 200);
    assert_eq!(v["commit"]["rev"], w.head(&a).1);
    let head = w.head(&a);
    for swap in [None, Some(c2.as_str())] {
        let (s, v) = del("k", swap);
        assert_eq!((s, v), (200, json!({})));
    }
    assert_eq!(w.head(&a), head);
    // the commit events report the operations
    let ops: Vec<(OpAction, String)> = w
        .net
        .events()
        .into_iter()
        .filter(|e| e.did == *a.did())
        .filter_map(|e| match e.kind {
            RepoEventKind::Commit { ops, .. } => Some(ops),
            _ => None,
        })
        .flatten()
        .map(|o| (o.action, o.rkey))
        .collect();
    assert_eq!(
        ops,
        [
            (OpAction::Create, "k".to_owned()),
            (OpAction::Update, "k".to_owned()),
            (OpAction::Delete, "k".to_owned())
        ]
    );
}

#[test]
fn apply_writes_is_atomic_and_strict() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let did = a.did().as_str().to_owned();
    let create = |rkey: &str, n: i64| json!({"$type": "com.atproto.repo.applyWrites#create", "collection": POST, "rkey": rkey, "value": post(n)});
    let update = |rkey: &str, n: i64| json!({"$type": "com.atproto.repo.applyWrites#update", "collection": POST, "rkey": rkey, "value": post(n)});
    let delete = |rkey: &str| json!({"$type": "com.atproto.repo.applyWrites#delete", "collection": POST, "rkey": rkey});
    let apply = |writes: Vec<Value>, swap: Option<&str>| {
        let mut b = json!({"repo": did, "writes": writes});
        if let Some(s) = swap {
            b["swapCommit"] = json!(s);
        }
        w.write(&a, "com.atproto.repo.applyWrites", &b)
    };
    let (head, _) = w.head(&a);
    let (s, v) = apply(vec![create("m", 0), create("m-1", 1)], Some(&head));
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["results"][0]["$type"],
        "com.atproto.repo.applyWrites#createResult"
    );
    assert_eq!(v["results"][1]["uri"], a.uri(POST, "m-1"));
    assert_eq!(v["results"][1]["validationStatus"], "unknown");
    // the same swapCommit again is stale
    let (s, v) = apply(vec![create("m-2", 2)], Some(&head));
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidSwap")));
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .starts_with("Commit was at bafyrei")
    );
    // create-existing, update-absent, delete-absent: 500 and nothing written
    let before = w.head(&a);
    for writes in [
        vec![create("m", 9), create("m-3", 3)],
        vec![create("m-4", 4), update("absent", 1)],
        vec![delete("absent")],
    ] {
        let (s, v) = apply(writes, None);
        assert_eq!((s, v["error"].as_str()), (500, Some("InternalServerError")));
    }
    assert_eq!(w.head(&a), before);
    assert!(a.get(POST, "m-3").is_none() && a.get(POST, "m-4").is_none());
    assert_eq!(a.get(POST, "m").unwrap()["n"], 0);
    // an identical update still makes a commit
    let (s, v) = apply(vec![update("m", 0)], None);
    assert_eq!(s, 200);
    assert_ne!(v["commit"]["cid"], before.0);
    // ops see earlier ops: update then delete of one key; create then update of a new key
    let (s, v) = apply(
        vec![update("m", 5), delete("m"), create("n", 1), update("n", 2)],
        None,
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        v["results"][1],
        json!({"$type": "com.atproto.repo.applyWrites#deleteResult"})
    );
    assert!(a.get(POST, "m").is_none());
    assert_eq!(a.get(POST, "n").unwrap()["n"], 2);
    // ≤ 200 ops
    let (s, _) = apply(
        (0..201).map(|i| create(&format!("x{i}"), i)).collect(),
        None,
    );
    assert_eq!(s, 400);
    let (s, _) = apply(
        (0..200).map(|i| create(&format!("x{i}"), i)).collect(),
        None,
    );
    assert_eq!(s, 200);
    assert_eq!(a.list(POST).len(), 202);
    // creates without rkey get TIDs
    let (s, v) = apply(
        vec![
            json!({"$type": "com.atproto.repo.applyWrites#create", "collection": REVISION, "value": {"$type": REVISION}}),
        ],
        None,
    );
    assert_eq!(s, 200);
    let k = v["results"][0]["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap();
    assert!(Tid::parse(k).is_ok());
}

#[test]
fn the_core_client_parses_write_responses() {
    use lessdugin_atp::syntax::AtIdentifier;
    use lessdugin_atp::xrpc::{ApplyWrites, PutRecord, WriteOp, WriteResult};
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let x = Xrpc::new(&w.host);
    let svc = Service::UserPds(a.did().clone());
    let out = block_on(x.procedure_endpoint(
        &svc,
        &PutRecord {
            repo: AtIdentifier::Did(a.did().clone()),
            collection: Nsid::parse(POST).unwrap(),
            rkey: RecordKey::parse("p").unwrap(),
            validate: None,
            record: post(1),
            swap_record: None,
            swap_commit: None,
        },
        &CallOpts::default(),
    ))
    .unwrap();
    assert_eq!(out.cid, a.cid(POST, "p").unwrap().to_string());
    assert!(out.commit.is_some());
    let out = block_on(x.procedure_endpoint(
        &svc,
        &ApplyWrites {
            repo: AtIdentifier::Did(a.did().clone()),
            validate: None,
            writes: vec![WriteOp::Delete {
                collection: Nsid::parse(POST).unwrap(),
                rkey: RecordKey::parse("p").unwrap(),
            }],
            swap_commit: Some(a.head().0.to_string()),
        },
        &CallOpts::default(),
    ))
    .unwrap();
    assert_eq!(out.results, vec![WriteResult::Delete {}]);
    // errors map to the core's typed errors
    let err = block_on(x.procedure_endpoint(
        &svc,
        &ApplyWrites {
            repo: AtIdentifier::Did(a.did().clone()),
            validate: None,
            writes: vec![],
            swap_commit: Some("bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm".into()),
        },
        &CallOpts::default(),
    ))
    .unwrap_err();
    assert!(matches!(err, XrpcCallError::InvalidSwap { .. }), "{err:?}");
    // a DID the network does not know: the host has no session
    let other = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
    let err = block_on(x.query::<Value>(
        &Service::UserPds(other),
        "com.atproto.server.getSession",
        &[],
        &CallOpts::default(),
    ))
    .unwrap_err();
    assert!(matches!(err, XrpcCallError::AuthRequired { .. }), "{err:?}");
}

// ---------------------------------------------------------------------------------------------
// Sessions

#[test]
fn sessions_and_bearer_auth() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let b = w.net.add_account("bob.example.com");
    let base = a.pds_base();
    // wrong password, unknown identifier
    let (s, v) = w.post(
        &base,
        "com.atproto.server.createSession",
        &json!({"identifier": "alice.example.com", "password": "nope"}),
        None,
    );
    assert_eq!(
        (s, v["error"].as_str()),
        (401, Some("AuthenticationRequired"))
    );
    let (s, _) = w.post(
        &base,
        "com.atproto.server.createSession",
        &json!({"identifier": "carol.example.com", "password": "nope"}),
        None,
    );
    assert_eq!(s, 401);
    // app password by handle (case-insensitive), DID or email
    for ident in ["Alice.Example.com", a.did().as_str(), "alice@mail.test"] {
        let (s, v) = w.post(
            &base,
            "com.atproto.server.createSession",
            &json!({"identifier": ident, "password": a.app_password()}),
            None,
        );
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["did"], a.did().as_str());
        assert_eq!(v["handle"], "alice.example.com");
        assert_eq!(v["active"], true);
    }
    assert!(lessdugin_core::engine::is_app_password_shape(
        &a.app_password()
    ));
    let (_, sess) = w.post(
        &base,
        "com.atproto.server.createSession",
        &json!({"identifier": "alice.example.com", "password": a.app_password()}),
        None,
    );
    let access = sess["accessJwt"].as_str().unwrap().to_owned();
    let refresh = sess["refreshJwt"].as_str().unwrap().to_owned();
    // JWT-shaped with exp = now + 2h
    let payload: Value = serde_json::from_slice(
        &data_encoding::BASE64URL_NOPAD
            .decode(access.split('.').nth(1).unwrap().as_bytes())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(payload["sub"], a.did().as_str());
    assert_eq!(
        payload["exp"].as_i64().unwrap(),
        (w.host.now() + 7_200_000) / 1000
    );
    // getSession with bearer
    let req = HttpRequest::get(Target::Anon(url(&format!(
        "{base}/xrpc/com.atproto.server.getSession"
    ))))
    .with_header("authorization", &format!("Bearer {access}"));
    let r = w.send(req).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(json_of(&r)["did"], a.did().as_str());
    // writes need a token for the body's repo
    let body =
        json!({"repo": a.did().as_str(), "collection": POST, "rkey": "x", "record": post(1)});
    let (s, v) = w.post(&base, "com.atproto.repo.putRecord", &body, None);
    assert_eq!((s, v["error"].as_str()), (401, Some("AuthMissing")));
    let (s, v) = w.post(&base, "com.atproto.repo.putRecord", &body, Some("garbage"));
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidToken")));
    let (s, v) = w.post(&base, "com.atproto.repo.putRecord", &body, Some(&refresh));
    assert_eq!((s, v["error"].as_str()), (400, Some("InvalidToken")));
    let (s, _) = w.post(
        &base,
        "com.atproto.repo.putRecord",
        &json!({"repo": b.did().as_str(), "collection": POST, "rkey": "x", "record": post(1)}),
        Some(&access),
    );
    assert_eq!(s, 401);
    let (s, v) = w.post(&base, "com.atproto.repo.putRecord", &body, Some(&access));
    assert_eq!(s, 200, "{v}");
    // a session from another PDS is not accepted here
    let other = w.net.add_pds("pds2.example.com");
    let (s, _) = w.post(
        other.base(),
        "com.atproto.repo.putRecord",
        &body,
        Some(&access),
    );
    assert_eq!(s, 400);
    // refresh rotates; the old refresh token is revoked
    let refresh_req = |tok: &str| {
        let req = HttpRequest::post_json(
            Target::Anon(url(&format!(
                "{base}/xrpc/com.atproto.server.refreshSession"
            ))),
            Vec::new(),
        )
        .with_header("authorization", &format!("Bearer {tok}"));
        let r = w.send(req).unwrap();
        (r.status, json_of(&r))
    };
    let (s, v) = refresh_req(&refresh);
    assert_eq!(s, 200);
    let refresh2 = v["refreshJwt"].as_str().unwrap().to_owned();
    assert_ne!(refresh2, refresh);
    let (s, v) = refresh_req(&refresh);
    assert_eq!((s, v["error"].as_str()), (400, Some("ExpiredToken")));
    // access tokens expire on the fake clock
    w.host.advance_ms(2 * 60 * 60 * 1000);
    let (s, v) = w.post(&base, "com.atproto.repo.putRecord", &body, Some(&access));
    assert_eq!((s, v["error"].as_str()), (400, Some("ExpiredToken")));
    let err = XrpcCallError::from_response(
        &HttpResponse {
            status: s,
            headers: vec![],
            body: serde_json::to_vec(&v).unwrap(),
        },
        0,
    );
    assert!(matches!(err, XrpcCallError::AuthRequired { .. }));
    // deleteSession revokes the refresh token
    let req = HttpRequest::post_json(
        Target::Anon(url(&format!(
            "{base}/xrpc/com.atproto.server.deleteSession"
        ))),
        Vec::new(),
    )
    .with_header("authorization", &format!("Bearer {refresh2}"));
    assert_eq!(w.send(req).unwrap().status, 200);
    assert_eq!(refresh_req(&refresh2).0, 400);
    // takedown blocks login
    b.takedown();
    let (s, v) = w.post(
        &base,
        "com.atproto.server.createSession",
        &json!({"identifier": "bob.example.com", "password": b.app_password()}),
        None,
    );
    assert_eq!((s, v["error"].as_str()), (401, Some("AccountTakedown")));
    // describeServer
    let (s, v) = w.get(&format!("{base}/xrpc/com.atproto.server.describeServer"));
    assert_eq!(s, 200);
    assert_eq!(v["did"], "did:web:pds.example.com");
}

// ---------------------------------------------------------------------------------------------
// Blobs

#[test]
fn blobs_follow_the_reference_lifecycle() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let tok = a.access_token();
    let base = a.pds_base();
    let upload = |bytes: Vec<u8>, mime: &str| {
        let req = HttpRequest::post_json(
            Target::Anon(url(&format!("{base}/xrpc/com.atproto.repo.uploadBlob"))),
            bytes,
        )
        .with_header("authorization", &format!("Bearer {tok}"));
        let mut req = req;
        req.headers.retain(|(k, _)| k != "content-type");
        let req = req.with_header("content-type", mime);
        let r = w.send(req).unwrap();
        assert_eq!(r.status, 200);
        json_of(&r)["blob"].clone()
    };
    let get_blob = |cid: &str| {
        let q = lessdugin_atp::xrpc::query_string(&[("did", a.did().as_str()), ("cid", cid)]);
        w.send(HttpRequest::get(Target::Anon(url(&format!(
            "{base}/xrpc/com.atproto.sync.getBlob?{q}"
        )))))
        .unwrap()
    };
    let blob_a = upload(tiny_png(1), "image/jpeg");
    assert_eq!(blob_a["mimeType"], "image/png");
    assert_eq!(blob_a["size"], tiny_png(1).len());
    let cid_a = blob_a["ref"]["$link"].as_str().unwrap().to_owned();
    assert_eq!(cid_a, lessdugin_atp::cid::for_raw(&tiny_png(1)).to_string());
    let blob_b = upload(tiny_png(2), "image/png");
    let blob_c = upload(tiny_png(3), "image/png");
    let text = upload(b"hello".to_vec(), "text/plain; charset=utf-8");
    assert_eq!(text["mimeType"], "text/plain");
    // temporary blobs are not served
    let r = get_blob(&cid_a);
    assert_eq!(
        (r.status, json_of(&r)["message"].as_str()),
        (400, Some("Blob not found"))
    );
    // reference A in a post and B only inside a revision snapshot
    let (s, v) = w.write(&a, "com.atproto.repo.putRecord", &json!({"repo": a.did().as_str(), "collection": POST, "rkey": "p", "record": {"$type": POST, "body": {"media": [{"blob": blob_a, "alt": "red"}]}}}));
    assert_eq!(s, 200, "{v}");
    let (s, _) = w.write(&a, "com.atproto.repo.putRecord", &json!({"repo": a.did().as_str(), "collection": REVISION, "rkey": "r", "record": {"$type": REVISION, "record": {"body": {"media": [{"blob": blob_b}]}}}}));
    assert_eq!(s, 200);
    let r = get_blob(&cid_a);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, tiny_png(1));
    assert_eq!(r.header("content-type"), Some("image/png"));
    assert_eq!(
        get_blob(blob_b["ref"]["$link"].as_str().unwrap()).status,
        200
    );
    // wrong mime, wrong size, unknown CID
    let mut wrong = blob_c.clone();
    wrong["mimeType"] = json!("image/jpeg");
    let mut big = blob_c.clone();
    big["size"] = json!(999);
    let mut unknown = blob_c.clone();
    unknown["ref"]["$link"] = json!(lessdugin_atp::cid::for_raw(b"never").to_string());
    for (blob, err) in [
        (wrong, "InvalidMimeType"),
        (big, "InvalidSize"),
        (unknown, "BlobNotFound"),
    ] {
        let (s, v) = w.write(&a, "com.atproto.repo.createRecord", &json!({"repo": a.did().as_str(), "collection": POST, "record": {"$type": POST, "img": blob}}));
        assert_eq!((s, v["error"].as_str()), (400, Some(err)), "{v}");
    }
    // the core host fetches and CID-checks a served blob; a missing one is 400 "Blob not found"
    let h = block_on(
        w.host
            .fetch_blob(url(&base), a.did().clone(), cid_a.clone(), 1 << 20),
    )
    .unwrap();
    assert_eq!(h.size, tiny_png(1).len() as u64);
    let err = block_on(w.host.fetch_blob(
        url(&base),
        a.did().clone(),
        blob_c["ref"]["$link"].as_str().unwrap().to_owned(),
        1 << 20,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        HostError::HttpStatus(400, Some("InvalidRequest".into()))
    );
    // deleting the last reference collects the blob at once; temporary blobs wait for gc
    let (s, _) = w.write(
        &a,
        "com.atproto.repo.deleteRecord",
        &json!({"repo": a.did().as_str(), "collection": POST, "rkey": "p"}),
    );
    assert_eq!(s, 200);
    assert_eq!(get_blob(&cid_a).status, 400);
    assert!(!a.has_blob(&lessdugin_atp::cid::parse(&cid_a).unwrap()));
    let cid_c = lessdugin_atp::cid::parse(blob_c["ref"]["$link"].as_str().unwrap()).unwrap();
    assert!(a.has_blob(&cid_c));
    assert_eq!(w.net.gc_blobs(), 2); // C and the text blob
    assert!(!a.has_blob(&cid_c));
    // listBlobs lists referenced blobs only
    let (_, v) = w.xrpc_get(
        &a,
        "com.atproto.sync.listBlobs",
        &[("did", a.did().as_str())],
    );
    assert_eq!(v["cids"], json!([blob_b["ref"]["$link"]]));
    // the upload limit
    a.pds().configure(|c| c.blob_upload_limit = 10);
    let req = HttpRequest::post_json(
        Target::Anon(url(&format!("{base}/xrpc/com.atproto.repo.uploadBlob"))),
        vec![0; 11],
    )
    .with_header("authorization", &format!("Bearer {tok}"));
    assert_eq!(w.send(req).unwrap().status, 413);
    // helper uploads behave the same
    let blob = a.upload_blob(tiny_png(9), "application/octet-stream");
    assert_eq!(blob["mimeType"], "image/png");
    a.put(POST, "q", &json!({"$type": POST, "img": blob}));
    assert!(a.serves_blob(&lessdugin_atp::cid::for_raw(&tiny_png(9))));
}

// ---------------------------------------------------------------------------------------------
// Faults, rate limits

#[test]
fn faults_apply_by_scope_and_count() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let b = w.net.add_account("bob.example.com");
    let id = w.net.inject(
        Fault::status(503)
            .on_repo(a.did().as_str())
            .on_nsid("com.atproto.sync.getLatestCommit")
            .times(2),
    );
    let commit = |acct: &AccountHandle| {
        w.xrpc_get(
            acct,
            "com.atproto.sync.getLatestCommit",
            &[("did", acct.did().as_str())],
        )
        .0
    };
    assert_eq!(commit(&b), 200);
    assert_eq!(commit(&a), 503);
    assert_eq!(w.net.fault_hits(id), Some(1));
    assert_eq!(commit(&a), 503);
    assert_eq!(commit(&a), 200);
    assert_eq!(w.net.fault_hits(id), None);
    // the core client retries 5xx GETs with backoff on the fake clock
    w.net.inject(Fault::status(502).once());
    let x = Xrpc::new(&w.host);
    let svc = Service::Anon(url(&a.pds_base()));
    let out: Value = block_on(x.query(
        &svc,
        "com.atproto.sync.getLatestCommit",
        &[("did", a.did().as_str())],
        &CallOpts::default(),
    ))
    .unwrap();
    assert!(out["rev"].is_string());
    // 429 with Retry-After
    w.net
        .inject(Fault::rate_limited(7).on_host(&a.pds_base()).once());
    let r = w
        .send(HttpRequest::get(Target::Anon(url(&format!(
            "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
            a.pds_base(),
            a.did()
        )))))
        .unwrap();
    assert_eq!(r.status, 429);
    assert_eq!(r.header("retry-after"), Some("7"));
    // latency advances the host clock; past the timeout it is a Timeout
    let t0 = w.host.now();
    a.inject(Fault::latency(1500).once());
    assert_eq!(commit(&a), 200);
    assert_eq!(w.host.now() - t0, 1500);
    a.inject(Fault::latency(30_000).once());
    let req = HttpRequest::get(Target::Anon(url(&format!(
        "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
        a.pds_base(),
        a.did()
    ))))
    .with_timeout_ms(1000);
    assert_eq!(w.send(req).unwrap_err(), HostError::Timeout);
    // truncation and disconnects
    a.inject(Fault::truncate(5).once());
    let r = w
        .send(HttpRequest::get(Target::Anon(url(&format!(
            "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
            a.pds_base(),
            a.did()
        )))))
        .unwrap();
    assert_eq!(r.body.len(), 5);
    a.inject(Fault::disconnect().once());
    let err = w
        .send(HttpRequest::get(Target::Anon(url(&format!(
            "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
            a.pds_base(),
            a.did()
        )))))
        .unwrap_err();
    assert!(matches!(err, HostError::Network(_)));
    // skip the first n, PLC faults, canned responses
    w.net.inject(
        Fault::error(400, "Nope", "no")
            .on_nsid("plc")
            .after(1)
            .once(),
    );
    let doc_url = format!("{}/{}", w.net.plc(), a.did());
    assert_eq!(w.get(&doc_url).0, 200);
    assert_eq!(w.get(&doc_url).1["error"], "Nope");
    assert_eq!(w.get(&doc_url).0, 200);
    // the log records faults
    let log = w.net.requests_for("com.atproto.sync.getLatestCommit");
    assert_eq!(log.iter().filter(|r| !r.faults.is_empty()).count(), 8);
    assert!(
        log.iter()
            .any(|r| r.status == 0 && r.faults == ["disconnect"])
    );
    w.net.clear_faults();
}

#[test]
fn per_host_rate_limits_send_ratelimit_headers() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let other = w.net.add_pds("pds2.example.com");
    let b = other.add_account("bob.example.com");
    a.pds().set_rate_limit(Some(RateLimit {
        limit: 3,
        window_ms: 300_000,
    }));
    let get = |acct: &AccountHandle| {
        w.send(HttpRequest::get(Target::Anon(url(&format!(
            "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
            acct.pds_base(),
            acct.did()
        )))))
        .unwrap()
    };
    let r = get(&a);
    assert_eq!(r.header("ratelimit-remaining"), Some("2"));
    assert_eq!(r.header("ratelimit-policy"), Some("3;w=300"));
    get(&a);
    get(&a);
    let r = get(&a);
    assert_eq!(r.status, 429);
    assert_eq!(json_of(&r)["error"], "RateLimitExceeded");
    let reset: i64 = r.header("ratelimit-reset").unwrap().parse().unwrap();
    assert_eq!(reset, (w.host.now() + 300_000) / 1000);
    // the budget is per host
    assert_eq!(get(&b).status, 200);
    assert!(get(&b).header("ratelimit-remaining").is_none());
    w.host.advance_ms(300_000);
    assert_eq!(get(&a).status, 200);
}

// ---------------------------------------------------------------------------------------------
// Account state, identity, relays

#[test]
fn account_states_and_moves() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    a.put(POST, "p", &post(1));
    let status = |acct: &AccountHandle| {
        w.xrpc_get(
            acct,
            "com.atproto.sync.getRepoStatus",
            &[("did", acct.did().as_str())],
        )
        .1
    };
    let commit = |acct: &AccountHandle| {
        w.xrpc_get(
            acct,
            "com.atproto.sync.getLatestCommit",
            &[("did", acct.did().as_str())],
        )
    };
    let s = status(&a);
    assert_eq!(s["active"], true);
    assert_eq!(s["rev"], a.head().1.to_string());
    for (f, name, st) in [
        (
            AccountHandle::deactivate as fn(&AccountHandle),
            "RepoDeactivated",
            "deactivated",
        ),
        (AccountHandle::takedown, "RepoTakendown", "takendown"),
        (AccountHandle::suspend, "RepoSuspended", "suspended"),
    ] {
        f(&a);
        assert_eq!(commit(&a).1["error"], name);
        let (s, v) = w.xrpc_get(
            &a,
            "com.atproto.repo.getRecord",
            &[
                ("repo", a.did().as_str()),
                ("collection", POST),
                ("rkey", "p"),
            ],
        );
        assert_eq!((s, v["error"].as_str()), (400, Some(name)));
        assert_eq!(
            status(&a),
            json!({"did": a.did().as_str(), "active": false, "status": st})
        );
        let err = XrpcCallError::from_response(
            &HttpResponse {
                status: 400,
                headers: vec![],
                body: serde_json::to_vec(&commit(&a).1).unwrap(),
            },
            0,
        );
        assert!(err.is_repo_status());
    }
    a.activate();
    assert_eq!(commit(&a).0, 200);
    // a move: the old PDS says deactivated, the new one serves the repo, the doc follows
    let pds2 = w.net.add_pds("https://pds2.example.com/");
    let old = a.pds();
    a.move_to(&pds2);
    assert_eq!(
        a.doc()["service"][0]["serviceEndpoint"],
        "https://pds2.example.com"
    );
    let (s, v) = w.get(&format!(
        "{}/xrpc/com.atproto.sync.getLatestCommit?did={}",
        old.base(),
        a.did()
    ));
    assert_eq!((s, v["error"].as_str()), (400, Some("RepoDeactivated")));
    assert_eq!(commit(&a).0, 200);
    assert_eq!(
        w.xrpc_get(
            &a,
            "com.atproto.repo.getRecord",
            &[
                ("repo", a.did().as_str()),
                ("collection", POST),
                ("rkey", "p")
            ]
        )
        .0,
        200
    );
    // deleted accounts are unknown; tombstoned PLC entries are 410
    a.delete_account();
    assert_eq!(commit(&a).1["error"], "RepoNotFound");
    assert_eq!(
        w.xrpc_get(
            &a,
            "com.atproto.sync.getRepoStatus",
            &[("did", a.did().as_str())]
        )
        .1["error"],
        "RepoNotFound"
    );
    assert_eq!(w.get(&format!("{}/{}", w.net.plc(), a.did())).0, 200);
    a.tombstone_plc();
    assert_eq!(w.get(&format!("{}/{}", w.net.plc(), a.did())).0, 410);
    assert_eq!(
        w.get(&format!("{}/did:plc:aaaaaaaaaaaaaaaaaaaaaaaa", w.net.plc()))
            .0,
        404
    );
    // account events
    let statuses: Vec<Option<String>> = w
        .net
        .events()
        .into_iter()
        .filter_map(|e| match e.kind {
            RepoEventKind::Account { status, .. } => Some(status),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        [
            None,
            Some("deactivated".into()),
            Some("takendown".into()),
            Some("suspended".into()),
            None,
            Some("deleted".into())
        ]
    );
}

#[test]
fn identity_documents_handles_and_doh() {
    let w = world();
    let a = w.net.add_account("Alice.Example.com");
    assert_eq!(a.handle(), "alice.example.com");
    let (s, doc) = w.get(&format!("{}/{}", w.net.plc(), a.did()));
    assert_eq!(s, 200);
    let parsed = lessdugin_atp::did_doc::DidDocument::from_json(&doc).unwrap();
    assert_eq!(parsed.signing_key().unwrap(), a.public_key());
    assert_eq!(parsed.pds_endpoint().unwrap(), DEFAULT_PDS);
    assert_eq!(parsed.handle().unwrap().as_str(), "alice.example.com");
    // resolveHandle at any PDS; DoH TXT at both default endpoints
    let pds2 = w.net.add_pds("pds2.example.com");
    let (_, v) = w.get(&format!(
        "{}/xrpc/com.atproto.identity.resolveHandle?handle=alice.example.com",
        pds2.base()
    ));
    assert_eq!(v["did"], a.did().as_str());
    let (s, v) = w.get(&format!(
        "{}/xrpc/com.atproto.identity.resolveHandle?handle=nobody.example.com",
        pds2.base()
    ));
    assert_eq!((s, v["error"].as_str()), (400, Some("HandleNotFound")));
    for doh in [
        lessdugin_testkit::identity::DOH,
        lessdugin_testkit::identity::DOH2,
    ] {
        let (_, v) = w.get(&format!("{doh}?name=_atproto.alice.example.com&type=TXT"));
        assert_eq!(v["Answer"][0]["data"], format!("\"did={}\"", a.did()));
        let (_, v) = w.get(&format!("{doh}?name=_atproto.nobody.example.com&type=TXT"));
        assert_eq!(v["Status"], 3);
    }
    // a handle change moves DNS and the document; the old handle stops resolving
    a.set_handle("alice2.example.com");
    let (_, v) = w.get(&format!(
        "{}?name=_atproto.alice.example.com&type=TXT",
        lessdugin_testkit::identity::DOH
    ));
    assert_eq!(v["Status"], 3);
    assert_eq!(a.doc()["alsoKnownAs"], json!(["at://alice2.example.com"]));
    assert_eq!(w.net.account("alice2.example.com").unwrap().did(), a.did());
    // a document override
    a.set_doc_override(Some(json!({"id": "did:plc:other"})));
    assert_eq!(
        w.get(&format!("{}/{}", w.net.plc(), a.did())).1,
        json!({"id": "did:plc:other"})
    );
    // describeServer, protected-resource metadata and health
    let (_, v) = w.get(&format!(
        "{DEFAULT_PDS}/.well-known/oauth-protected-resource"
    ));
    assert_eq!(v["authorization_servers"], json!([DEFAULT_PDS]));
    assert_eq!(w.get(&format!("{DEFAULT_PDS}/xrpc/_health")).0, 200);
    assert_eq!(
        w.get(&format!("{DEFAULT_PDS}/xrpc/com.example.unknown")).0,
        501
    );
}

#[test]
fn relays_list_repos_by_collection() {
    let w = world();
    let pds2 = w.net.add_pds("pds2.example.com");
    let mut dids = Vec::new();
    for i in 0..5 {
        let acct = if i % 2 == 0 {
            w.net.add_account(&format!("u{i}.example.com"))
        } else {
            pds2.add_account(&format!("u{i}.example.com"))
        };
        if i != 3 {
            acct.put(
                "online.7778777.lessdugin.membership",
                "aaaaaaaaaaaa",
                &json!({"$type": "online.7778777.lessdugin.membership"}),
            );
            dids.push(acct.did().to_string());
        }
        if i == 4 {
            acct.deactivate();
            dids.pop();
        }
    }
    dids.sort();
    for relay in w.net.relays() {
        let mut got = Vec::new();
        let mut cursor = String::new();
        loop {
            let (s, v) = w.get(&format!("{relay}/xrpc/com.atproto.sync.listReposByCollection?collection=online.7778777.lessdugin.membership&limit=2&cursor={cursor}"));
            assert_eq!(s, 200);
            got.extend(
                v["repos"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["did"].as_str().unwrap().to_owned()),
            );
            match v["cursor"].as_str() {
                Some(c) => cursor = c.to_owned(),
                None => break,
            }
        }
        assert_eq!(got, dids);
    }
}

#[test]
fn equivocation_serves_a_different_commit_to_chosen_callers() {
    let host_a = MemHost::new();
    let host_b = MemHost::new();
    let net = FakeNet::new(host_a.clone());
    net.install_as(&host_a, "a");
    net.install_as(&host_b, "b");
    let alice = net.add_account("alice.example.com");
    alice.put(POST, "p", &post(1));
    let (real_cid, rev) = alice.head();
    let (fork_cid, fork_rev) = alice.equivocate(ForkSelector::callers(["b"]), |w| {
        w.put(POST, "p", &post(2));
        w.put(POST, "q", &post(3));
    });
    assert_eq!(fork_rev, rev);
    assert_ne!(fork_cid, real_cid);
    let get = |h: &MemHost, path: &str| {
        let r = block_on(h.http(HttpRequest::get(Target::Anon(url(&format!(
            "{DEFAULT_PDS}{path}"
        ))))))
        .unwrap();
        json_of(&r)
    };
    let commit = format!("/xrpc/com.atproto.sync.getLatestCommit?did={}", alice.did());
    assert_eq!(get(&host_a, &commit)["cid"], real_cid.to_string());
    assert_eq!(get(&host_b, &commit)["cid"], fork_cid.to_string());
    assert_eq!(get(&host_b, &commit)["rev"], rev.to_string());
    let rec = format!(
        "/xrpc/com.atproto.repo.getRecord?repo={}&collection={POST}&rkey=p",
        alice.did()
    );
    assert_eq!(get(&host_a, &rec)["value"]["n"], 1);
    assert_eq!(get(&host_b, &rec)["value"]["n"], 2);
    // both commits are validly signed: the proofs verify for each caller
    for (h, n) in [(&host_a, None), (&host_b, Some(()))] {
        let r = block_on(h.http(HttpRequest::get(Target::Anon(url(&format!(
            "{DEFAULT_PDS}/xrpc/com.atproto.sync.getRecord?did={}&collection={POST}&rkey=q",
            alice.did()
        ))))))
        .unwrap();
        let out = verify_record_proof(
            &r.body,
            alice.did(),
            &Nsid::parse(POST).unwrap(),
            &RecordKey::parse("q").unwrap(),
            &alice.public_key(),
            host_a.now() * 1000,
            &ProofCaps::default(),
        )
        .unwrap();
        assert_eq!(out.is_present(), n.is_some());
    }
    assert!(net.requests().iter().any(|r| r.fork && r.caller == "b"));
    assert!(net.requests().iter().all(|r| !r.fork || r.caller == "b"));
    // alternate: every other request
    alice.equivocate(ForkSelector::Alternate, |w| {
        w.delete(POST, "p");
    });
    let cids: Vec<Value> = (0..4)
        .map(|_| get(&host_a, &commit)["cid"].clone())
        .collect();
    assert_eq!(cids[0], cids[2]);
    assert_ne!(cids[0], cids[1]);
    alice.end_equivocation();
    assert_eq!(get(&host_b, &commit)["cid"], real_cid.to_string());
}

#[test]
fn helper_writes_commit_lazily_and_emit_events() {
    let w = world();
    let a = w.net.add_account("alice.example.com");
    let after = w.net.events().last().unwrap().seq;
    a.batch(|b| {
        b.put(POST, "1", &post(1));
        b.put(POST, "2", &post(2));
        b.delete(POST, "2");
    });
    a.put(POST, "3", &post(3));
    assert!(w.net.events_since(after).is_empty());
    w.host.advance_ms(10);
    let (cid, rev) = a.head();
    let ev = w.net.events_since(after);
    assert_eq!(ev.len(), 1);
    match &ev[0].kind {
        RepoEventKind::Commit {
            rev: r,
            cid: c,
            ops,
        } => {
            assert_eq!((*r, *c), (rev, cid));
            let ks: Vec<(&str, OpAction)> =
                ops.iter().map(|o| (o.rkey.as_str(), o.action)).collect();
            assert_eq!(ks, [("1", OpAction::Create), ("3", OpAction::Create)]);
            assert_eq!(ops[0].record_json().unwrap(), post(1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rev.timestamp_micros(), (w.host.now() * 1000) as u64);
    // revs strictly increase even when the clock stands still
    let (_, r2) = a.commit_now();
    let (_, r3) = a.commit_now();
    assert!(r2 > rev && r3 > r2);
    // commit_at sets an explicit rev (rollback tests)
    let past = Tid::from_parts(1_000_000, 0).unwrap();
    a.commit_at(past);
    assert_eq!(w.head(&a).1, past.to_string());
    // statuses and other details are reported
    assert_eq!(w.net.accounts().len(), 1);
    assert_eq!(
        w.net.account(a.did().as_str()).unwrap().handle(),
        "alice.example.com"
    );
    assert_eq!(AccountStatus::Takendown.as_status(), Some("takendown"));
    assert!(format!("{:?}", w.net).contains("FakeNet"));
}
