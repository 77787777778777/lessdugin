// SPDX-License-Identifier: AGPL-3.0-or-later
//! MemHost behaviour and the KvStore contract on MemKv.

use futures::executor::block_on;
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{Host, HostError, HttpRequest, Target, WebSocket, WsMessage};
use lessdugin_core::store::mem::MemKv;
use lessdugin_testkit::{MemHost, kv::kv_conformance, resp};

fn get(url: &str) -> HttpRequest {
    HttpRequest::get(Target::Anon(url::Url::parse(url).unwrap()))
}

#[test]
fn memkv_passes_the_kv_contract() {
    block_on(kv_conformance(&MemKv::new()));
    let host = MemHost::new();
    block_on(kv_conformance(host.cache()));
    block_on(kv_conformance(host.state()));
}

#[test]
fn routing_prefers_the_longest_then_latest_prefix() {
    let host = MemHost::new();
    host.respond_get("https://a.example/", resp::text(200, "short"));
    host.respond_get("https://a.example/x/", resp::text(200, "long"));
    host.respond_get("https://a.example/x/", resp::text(200, "long, newer"));
    host.respond("https://a.example/post", resp::text(201, "any method"));
    let body = |url: &str| String::from_utf8(block_on(host.http(get(url))).unwrap().body).unwrap();
    assert_eq!(body("https://a.example/y"), "short");
    assert_eq!(body("https://a.example/x/1"), "long, newer");
    // GET-only routes do not match POSTs.
    let post = HttpRequest::post_json(
        Target::Anon(url::Url::parse("https://a.example/x/1").unwrap()),
        b"{}".to_vec(),
    );
    assert_eq!(block_on(host.http(post)).unwrap().status, 404);
    let post = HttpRequest::post_json(
        Target::Anon(url::Url::parse("https://a.example/post").unwrap()),
        b"{}".to_vec(),
    );
    assert_eq!(block_on(host.http(post)).unwrap().status, 201);
    // Unrouted requests are 404 and recorded as unmatched.
    assert_eq!(
        block_on(host.http(get("https://b.example/")))
            .unwrap()
            .status,
        404
    );
    let unmatched = host.unmatched();
    assert_eq!(unmatched.len(), 2);
    assert_eq!(unmatched[1].url, "https://b.example/");
    assert_eq!(host.requests().len(), 5);
}

#[test]
fn caps_clock_and_randomness() {
    let host = MemHost::new();
    host.respond_get("https://a.example/", resp::text(200, "0123456789"));
    assert_eq!(
        block_on(host.http(get("https://a.example/").with_max_bytes(9))).unwrap_err(),
        HostError::TooLarge
    );
    assert!(block_on(host.http(get("https://a.example/").with_max_bytes(10))).is_ok());

    let t0 = host.now_ms();
    block_on(host.sleep(1_500));
    assert_eq!(host.now_ms(), t0 + 1_500);
    assert_eq!(host.sleeps(), vec![1_500]);
    host.advance_ms(10);
    assert_eq!(host.now_ms(), t0 + 1_510);

    let mut a = [0u8; 13];
    let mut b = [0u8; 13];
    host.set_seed(42);
    host.fill_random(&mut a);
    host.set_seed(42);
    host.fill_random(&mut b);
    assert_eq!(a, b);
    host.fill_random(&mut b);
    assert_ne!(a, b);

    let seq = MemHost::new();
    seq.respond_seq(
        "https://s.example/",
        vec![
            Ok(resp::status(500)),
            Err(HostError::Timeout),
            Ok(resp::status(200)),
        ],
    );
    assert_eq!(
        block_on(seq.http(get("https://s.example/")))
            .unwrap()
            .status,
        500
    );
    assert_eq!(
        block_on(seq.http(get("https://s.example/"))).unwrap_err(),
        HostError::Timeout
    );
    assert_eq!(
        block_on(seq.http(get("https://s.example/")))
            .unwrap()
            .status,
        200
    );
    assert_eq!(
        block_on(seq.http(get("https://s.example/")))
            .unwrap()
            .status,
        200,
        "the last answer repeats"
    );
}

#[test]
fn blobs_are_hash_checked() {
    let host = MemHost::new();
    let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
    let pds = url::Url::parse("https://pds.example.com").unwrap();
    let data = b"hello blob".to_vec();
    let cid = lessdugin_atp::cid::for_raw(&data).to_string();
    host.respond_get(
        "https://pds.example.com/xrpc/com.atproto.sync.getBlob",
        resp::bytes(200, "image/png", data.clone()),
    );
    let h = block_on(host.fetch_blob(pds.clone(), did.clone(), cid.clone(), 1_000)).unwrap();
    assert_eq!(h.size, data.len() as u64);
    assert_eq!(host.blob(&cid), Some(data.clone()));
    let req = &host.requests()[0];
    assert!(req.url.contains("did=did%3Aplc%3Aewvi7nxzyoun6zhxrhs64oiz"));
    // A stored blob needs no request.
    block_on(host.fetch_blob(pds.clone(), did.clone(), cid.clone(), 1_000)).unwrap();
    assert_eq!(host.requests().len(), 1);
    // Mismatch, size cap, wrong codec, error status.
    let other = lessdugin_atp::cid::for_raw(b"other").to_string();
    assert_eq!(
        block_on(host.fetch_blob(pds.clone(), did.clone(), other.clone(), 1_000)).unwrap_err(),
        HostError::CidMismatch
    );
    assert_eq!(host.blob(&other), None);
    let small = lessdugin_atp::cid::for_raw(b"x").to_string();
    assert_eq!(
        block_on(host.fetch_blob(pds.clone(), did.clone(), small, 3)).unwrap_err(),
        HostError::TooLarge
    );
    let dag = lessdugin_atp::cid::for_dag_cbor(b"\xa0").to_string();
    assert!(matches!(
        block_on(host.fetch_blob(pds.clone(), did.clone(), dag, 1_000)),
        Err(HostError::Unsupported(_))
    ));
    host.respond_get(
        "https://pds.example.com/xrpc/com.atproto.sync.getBlob",
        resp::xrpc_error(400, "BlobNotFound", "no"),
    );
    let missing = lessdugin_atp::cid::for_raw(b"missing").to_string();
    assert_eq!(
        block_on(host.fetch_blob(pds, did, missing, 1_000)).unwrap_err(),
        HostError::HttpStatus(400, Some("BlobNotFound".into()))
    );
}

#[test]
fn websockets_are_scripted() {
    let host = MemHost::new();
    host.script_websocket(
        "wss://jet.example/",
        vec![WsMessage::Text("a".into()), WsMessage::Binary(vec![0; 10])],
    );
    let mut ws =
        block_on(host.websocket(url::Url::parse("wss://jet.example/subscribe").unwrap(), 5))
            .unwrap();
    assert_eq!(block_on(ws.next()), Some(Ok(WsMessage::Text("a".into()))));
    assert_eq!(block_on(ws.next()), Some(Err(HostError::TooLarge)));
    assert_eq!(block_on(ws.next()), None);
    block_on(ws.send_text("hello".into())).unwrap();
    assert_eq!(host.websocket_sent(), vec!["hello".to_owned()]);
    assert!(block_on(host.websocket(url::Url::parse("wss://other.example/").unwrap(), 5)).is_err());
    assert!(matches!(
        block_on(host.resolve_txt_system("x".into())),
        Err(HostError::Unsupported(_))
    ));
    host.set_system_txt("x", Ok(None));
    assert_eq!(block_on(host.resolve_txt_system("x".into())), Ok(None));
}
