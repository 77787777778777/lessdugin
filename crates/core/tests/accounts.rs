// SPDX-License-Identifier: AGPL-3.0-or-later
//! Accounts and the login binding (`sync.md` §3.6) over `MemHost`.

mod common;

use common::*;
use lessdugin_core::dto::AuthKind;
use lessdugin_core::engine::{AccountInfo, ErrorCode, PinStatus};
use lessdugin_core::host::{Dir, KeyRange, KvOp, KvStore};
use lessdugin_core::key;
use lessdugin_testkit::identity::{DOH, PLC, did_doc, install_doc};
use lessdugin_testkit::{FakePds, MemHost};

fn setup() -> (
    MemHost,
    FakePds,
    lessdugin_testkit::FakeIdentity,
    lessdugin_testkit::FakeIdentity,
) {
    let host = MemHost::new();
    let pds = FakePds::install(&host, PDS);
    let (a, b) = (alice(), bob());
    a.install(&host, PLC, DOH);
    b.install(&host, PLC, DOH);
    (host, pds, a, b)
}

#[test]
fn app_password_account_lifecycle() {
    let (host, _pds, alice, bob) = setup();
    let e = engine(&host);
    assert!(block_on(e.accounts()).unwrap().is_empty());
    assert!(block_on(e.active_account()).unwrap().is_none());

    let a = block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    assert_eq!(a.did, alice.did.as_str());
    assert_eq!(a.handle.as_deref(), Some("alice.example.com"));
    assert_eq!(a.pds, PDS);
    assert_eq!(a.auth_kind, AuthKind::AppPassword);
    assert!(a.active);
    // The PRM document is not consulted for app passwords.
    assert!(host.requests_to(&format!("{PDS}/.well-known")).is_empty());

    host.advance_ms(1_000);
    let b = block_on(e.add_account(AccountInfo::app_password(
        bob.did.as_str(),
        format!("{PDS}/"),
    )))
    .unwrap();
    assert!(b.active, "the newest login becomes active");
    let all = block_on(e.accounts()).unwrap();
    assert_eq!(
        all.iter()
            .map(|x| (x.did.as_str(), x.active))
            .collect::<Vec<_>>(),
        vec![(alice.did.as_str(), false), (bob.did.as_str(), true),]
    );

    block_on(e.set_active_account(alice.did.as_str())).unwrap();
    assert_eq!(
        block_on(e.active_account()).unwrap().unwrap().did,
        alice.did.as_str()
    );
    assert_eq!(
        block_on(e.set_active_account("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    // Re-login keeps the original order.
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    let order: Vec<String> = block_on(e.accounts())
        .unwrap()
        .into_iter()
        .map(|x| x.did)
        .collect();
    assert_eq!(order, vec![alice.did.to_string(), bob.did.to_string()]);

    block_on(e.remove_account(alice.did.as_str(), false)).unwrap();
    assert!(
        block_on(e.active_account()).unwrap().is_none(),
        "removing the active account clears it"
    );
    assert_eq!(block_on(e.accounts()).unwrap().len(), 1);
    assert_eq!(
        block_on(e.remove_account(alice.did.as_str(), false))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        block_on(e.account(bob.did.as_str())).unwrap().unwrap().did,
        bob.did.as_str()
    );

    // Accounts live in the durable state store, not the cache.
    let rows = block_on(host.state_kv().scan(
        KeyRange::prefix(&key!("loc", "account")),
        Dir::Forward,
        10,
    ))
    .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn purge_removes_the_accounts_local_state() {
    let (host, _pds, alice, bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    block_on(e.add_account(AccountInfo::app_password(bob.did.as_str(), PDS))).unwrap();
    let seed = |did: &str| {
        vec![
            KvOp::Put(key!("out", did, 1i64), b"w1".to_vec()),
            KvOp::Put(key!("out", did, 2i64), b"w2".to_vec()),
            KvOp::Put(key!("out.budget", did), b"b".to_vec()),
            KvOp::Put(key!("notif", did, 5i64, "n1"), b"n".to_vec()),
        ]
    };
    block_on(
        host.state_kv()
            .write([seed(alice.did.as_str()), seed(bob.did.as_str())].concat()),
    )
    .unwrap();
    block_on(e.remove_account(alice.did.as_str(), true)).unwrap();
    let left = block_on(
        host.state_kv()
            .scan(KeyRange::prefix(&key!("out")), Dir::Forward, 100),
    )
    .unwrap();
    assert_eq!(left.len(), 2, "only bob's outbox remains");
    let budget = block_on(host.state_kv().get_many(vec![
        key!("out.budget", alice.did.as_str()),
        key!("out.budget", bob.did.as_str()),
    ]))
    .unwrap();
    assert_eq!(budget, vec![None, Some(b"b".to_vec())]);
    let notif = block_on(
        host.state_kv()
            .scan(KeyRange::prefix(&key!("notif")), Dir::Forward, 100),
    )
    .unwrap();
    assert_eq!(notif.len(), 1);
}

#[test]
fn oauth_binding_pins_the_issuer() {
    let (host, pds, alice, _) = setup();
    let e = engine(&host);
    let entryway = "https://entryway.example.com";
    pds.set_authorization_servers(vec![entryway.to_owned(), PDS.to_owned()]);

    let b = block_on(e.bind_login(&AccountInfo::oauth(alice.did.as_str(), PDS, entryway))).unwrap();
    assert_eq!(b.pin, PinStatus::New);
    assert_eq!(b.issuer.as_deref(), Some(entryway));
    let prm = host.requests_to(&format!("{PDS}/.well-known/oauth-protected-resource"));
    assert_eq!(prm.len(), 1);
    assert_eq!(prm[0].redirects, lessdugin_core::host::Redirects::None);
    assert_eq!(prm[0].max_bytes, 65_536);

    let v = block_on(e.add_account(AccountInfo::oauth(alice.did.as_str(), PDS, entryway))).unwrap();
    assert_eq!(v.auth_kind, AuthKind::Oauth);
    assert_eq!(
        block_on(e.bind_login(&AccountInfo::oauth(alice.did.as_str(), PDS, entryway)))
            .unwrap()
            .pin,
        PinStatus::Same
    );

    // A different (but listed) issuer needs explicit confirmation.
    let err =
        block_on(e.add_account(AccountInfo::oauth(alice.did.as_str(), PDS, PDS))).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    let d = err.detail.unwrap();
    assert_eq!(d["kind"], "issuerChanged");
    assert_eq!(d["pinned"], entryway);
    assert_eq!(d["issuer"], PDS);
    let b = block_on(
        e.bind_login(&AccountInfo::oauth(alice.did.as_str(), PDS, PDS).confirming_issuer_change()),
    )
    .unwrap();
    assert_eq!(b.pin, PinStatus::Changed);
    assert_eq!(b.previous_issuer.as_deref(), Some(entryway));
    assert_eq!(
        block_on(e.bind_login(&AccountInfo::oauth(alice.did.as_str(), PDS, PDS)))
            .unwrap()
            .pin,
        PinStatus::Same
    );
}

#[test]
fn the_issuer_must_be_listed_by_the_pds() {
    let (host, _pds, alice, _) = setup();
    let e = engine(&host);
    let err = block_on(e.add_account(AccountInfo::oauth(
        alice.did.as_str(),
        PDS,
        "https://evil.example.com",
    )))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Blocked);
    assert_eq!(err.detail.unwrap()["kind"], "issuerNotListed");
    assert!(
        block_on(e.accounts()).unwrap().is_empty(),
        "nothing is stored on failure"
    );
    // Issuers are compared exactly.
    let err = block_on(e.add_account(AccountInfo::oauth(
        alice.did.as_str(),
        PDS,
        format!("{PDS}/"),
    )))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Blocked);
    // OAuth without an issuer is invalid input.
    let mut info = AccountInfo::oauth(alice.did.as_str(), PDS, PDS);
    info.issuer = None;
    assert_eq!(
        block_on(e.add_account(info)).unwrap_err().code,
        ErrorCode::InvalidInput
    );
}

#[test]
fn the_session_must_match_the_identity() {
    let (host, _pds, alice, bob) = setup();
    let e = engine(&host);
    // The session's PDS must be the document's PDS.
    let err = block_on(e.add_account(AccountInfo::app_password(
        alice.did.as_str(),
        "https://other-pds.example.com",
    )))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(err.detail.unwrap()["kind"], "pdsMismatch");
    // The session must be for the account the user asked for.
    let err = block_on(e.add_account(
        AccountInfo::app_password(bob.did.as_str(), PDS).with_intended(alice.did.as_str()),
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(err.detail.unwrap()["kind"], "accountMismatch");
    // A DID whose document has no PDS cannot log in.
    let carol = lessdugin_testkit::FakeIdentity::plc("carol", "carol.example.com", PDS);
    let mut doc = did_doc(carol.did.as_str(), None, &carol.multikey(), PDS);
    doc["service"] = serde_json::json!([]);
    install_doc(&host, PLC, &carol.did, &doc);
    assert_eq!(
        block_on(e.add_account(AccountInfo::app_password(carol.did.as_str(), PDS)))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    // Bad input.
    assert_eq!(
        block_on(e.add_account(AccountInfo::app_password("alice", PDS)))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), "ftp://x")))
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert!(block_on(e.accounts()).unwrap().is_empty());
    // The binding always uses a freshly fetched document.
    host.clear_requests();
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    assert_eq!(host.requests_to(&format!("{PLC}/{}", alice.did)).len(), 1);
}

#[test]
fn account_info_wire_shape() {
    let info = AccountInfo::oauth("did:plc:ewvi7nxzyoun6zhxrhs64oiz", PDS, PDS)
        .with_intended("did:plc:ewvi7nxzyoun6zhxrhs64oiz");
    let v = serde_json::to_value(&info).unwrap();
    assert_eq!(v["auth"], "oauth");
    assert_eq!(v["issuer"], PDS);
    assert_eq!(v["confirmIssuerChange"], false);
    let back: AccountInfo = serde_json::from_value(serde_json::json!({"did": "did:plc:ewvi7nxzyoun6zhxrhs64oiz", "pds": PDS, "auth": "appPassword"})).unwrap();
    assert_eq!(
        back,
        AccountInfo::app_password("did:plc:ewvi7nxzyoun6zhxrhs64oiz", PDS)
    );
}

#[test]
fn a_login_checks_its_binding_once() {
    let (host, pds, alice, bob) = setup();
    let e = engine(&host);
    pds.set_authorization_servers(vec![PDS.to_owned()]);
    let info = AccountInfo::oauth(alice.did.as_str(), PDS, PDS);
    host.clear_requests();
    let b = block_on(e.bind_login(&info)).unwrap();
    let v = block_on(e.record_account(&info, &b)).unwrap();
    assert!(v.active);
    assert_eq!(v.auth_kind, AuthKind::Oauth);
    // One fresh document and one protected-resource fetch for the whole login.
    assert_eq!(host.requests_to(&format!("{PLC}/{}", alice.did)).len(), 1);
    assert_eq!(
        host.requests_to(&format!("{PDS}/.well-known/oauth-protected-resource"))
            .len(),
        1
    );
    // A binding cannot be recorded for another session.
    let other = AccountInfo::app_password(bob.did.as_str(), PDS);
    assert_eq!(
        block_on(e.record_account(&other, &b)).unwrap_err().code,
        ErrorCode::Internal
    );
    let as_app_password = AccountInfo::app_password(alice.did.as_str(), PDS);
    assert!(block_on(e.record_account(&as_app_password, &b)).is_err());
    let other_issuer = AccountInfo::oauth(alice.did.as_str(), PDS, "https://as.example.com");
    assert!(block_on(e.record_account(&other_issuer, &b)).is_err());
}

#[test]
fn refresh_account_follows_the_identity() {
    let (host, _pds, alice, _) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();

    // The handle moves to another domain: the recorded handle follows (after the cache TTL).
    let moved = lessdugin_testkit::FakeIdentity {
        handle: Some("alice.example.org".into()),
        ..alice.clone()
    };
    install_doc(&host, PLC, &alice.did, &moved.doc());
    lessdugin_testkit::identity::install_txt(
        &host,
        DOH,
        "alice.example.org",
        &[&format!("did={}", alice.did)],
    );
    host.advance_ms(lessdugin_core::identity::DID_DOC_TTL_MS);
    let v = block_on(e.refresh_account(alice.did.as_str())).unwrap();
    assert_eq!(v.handle.as_deref(), Some("alice.example.org"));
    assert!(v.active);
    assert_eq!(
        block_on(e.account(alice.did.as_str()))
            .unwrap()
            .unwrap()
            .handle
            .as_deref(),
        Some("alice.example.org"),
        "the record was updated"
    );

    // The old handle now belongs to someone else: alice's document no longer claims a handle
    // that resolves back, so no handle is recorded.
    let unclaimed = lessdugin_testkit::FakeIdentity {
        handle: Some("alice.example.com".into()),
        ..alice.clone()
    };
    install_doc(&host, PLC, &alice.did, &unclaimed.doc());
    lessdugin_testkit::identity::install_txt(
        &host,
        DOH,
        "alice.example.com",
        &["did=did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"],
    );
    host.advance_ms(lessdugin_core::identity::DID_DOC_TTL_MS);
    let v = block_on(e.refresh_account(alice.did.as_str())).unwrap();
    assert_eq!(v.handle, None);

    // The PDS changed (migration): needs-relogin, never silent re-targeting (AUTH-8).
    let migrated = lessdugin_testkit::FakeIdentity {
        pds: "https://new-pds.example.com".into(),
        ..alice.clone()
    };
    install_doc(&host, PLC, &alice.did, &migrated.doc());
    host.advance_ms(lessdugin_core::identity::DID_DOC_TTL_MS);
    let err = block_on(e.refresh_account(alice.did.as_str())).unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthRequired);
    let d = err.detail.unwrap();
    assert_eq!(d["kind"], "needsRelogin");
    assert_eq!(d["boundPds"], PDS);
    assert_eq!(d["docPds"], "https://new-pds.example.com");
    assert_eq!(
        block_on(e.account(alice.did.as_str()))
            .unwrap()
            .unwrap()
            .pds,
        PDS,
        "the bound PDS is never changed silently"
    );

    // An unreachable PLC directory is an error, and changes nothing.
    host.fail(
        &format!("{PLC}/{}", alice.did),
        lessdugin_core::host::HostError::Network("down".into()),
    );
    host.advance_ms(lessdugin_core::identity::DID_DOC_TTL_MS);
    assert_eq!(
        block_on(e.refresh_account(alice.did.as_str()))
            .unwrap_err()
            .code,
        ErrorCode::Network
    );
    assert_eq!(
        block_on(e.refresh_account("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}
