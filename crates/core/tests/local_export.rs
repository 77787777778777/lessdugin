// SPDX-License-Identifier: AGPL-3.0-or-later
//! `export_local` / `import_local` (`engine-api.md` §3.6, M4) and the `export` command's privacy
//! rule (`threat-model.md` PRIV-8, PRIV-10) over `MemHost`.

mod common;

use common::*;
use lessdugin_core::engine::AccountInfo;
use lessdugin_core::engine::{EXPORT_VERSION, ExportScope, LocalExport};
use lessdugin_core::host::{Host, KvOp, KvStore};
use lessdugin_testkit::identity::{DOH, PLC};
use lessdugin_testkit::{FakePds, MemHost};

const PDS: &str = "https://pds.example.com";

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

/// Writes one local-state row straight into the state store.
fn put(host: &MemHost, parts: &[&str], value: &[u8]) {
    let parts: Vec<lessdugin_core::store::key::Part> = parts.iter().map(|s| (*s).into()).collect();
    block_on(host.state().write(vec![KvOp::Put(
        lessdugin_core::store::key::encode(&parts),
        value.to_vec(),
    )]))
    .unwrap();
}

#[test]
fn export_of_an_empty_store_is_empty_and_versioned() {
    let (host, _pds, _alice, _bob) = setup();
    let e = engine(&host);
    let x = block_on(e.export_local(ExportScope::Mine)).unwrap();
    assert_eq!(x.version, EXPORT_VERSION);
    assert_eq!(x.scope, "mine");
    assert_eq!(x.dto.0, lessdugin_core::dto::DTO_VERSION);
    // no accounts logged in, so nothing account-scoped to export
    assert!(x.members.is_empty(), "{:?}", x.members);
}

#[test]
fn mine_covers_only_the_active_account_and_the_account_free_rows() {
    let (host, _pds, alice, bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    // alice is active; bob's rows belong to another account on the same device
    put(&host, &["loc", "follow", "alice-forum"], b"a");
    put(&host, &["loc", "follow", "bob-forum"], b"b");
    put(&host, &["loc", "ack", alice.did.as_str(), "vote"], b"1");
    put(&host, &["loc", "ack", bob.did.as_str(), "vote"], b"1");
    // account-free rows: these belong to the device, not to an account
    put(&host, &["loc", "karmaCadence"], b"daily");
    put(&host, &["loc", "bookmark", "at://x"], b"1");

    let mine = block_on(e.export_local(ExportScope::Mine)).unwrap();
    assert_eq!(mine.account.as_deref(), Some(alice.did.as_str()));
    let tags: Vec<String> = mine
        .members
        .iter()
        .map(|m| m.parts.get(1).cloned().unwrap_or_default())
        .collect();
    assert!(tags.contains(&"karmaCadence".to_owned()), "{tags:?}");
    assert!(tags.contains(&"bookmark".to_owned()), "{tags:?}");
    // alice's ack is in, bob's is not
    let acks: Vec<&Vec<String>> = mine
        .members
        .iter()
        .map(|m| &m.parts)
        .filter(|p| p.get(1).is_some_and(|t| t == "ack"))
        .collect();
    assert_eq!(acks.len(), 1, "{acks:?}");
    assert_eq!(acks[0].get(2).map(String::as_str), Some(alice.did.as_str()));

    // everything covers both accounts
    let all = block_on(e.export_local(ExportScope::Everything)).unwrap();
    assert!(all.account.is_none());
    assert!(all.members.len() > mine.members.len());
    assert_eq!(all.scope, "everything");
}

#[test]
fn an_export_is_deterministic() {
    let (host, _pds, alice, _bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    for i in 0..40 {
        put(&host, &["loc", "bookmark", &format!("at://post/{i}")], b"1");
    }
    let a = block_on(e.export_local(ExportScope::Mine)).unwrap();
    let b = block_on(e.export_local(ExportScope::Mine)).unwrap();
    // key order, and the exported_ms is the only field that may differ within a still clock
    let keys = |x: &LocalExport| x.members.iter().map(|m| m.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&a), keys(&b));
    assert_eq!(
        serde_json::to_string(&a.members).unwrap(),
        serde_json::to_string(&b.members).unwrap()
    );
}

#[test]
fn import_merges_and_leaves_unmentioned_keys_alone() {
    let (host, _pds, alice, _bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    put(&host, &["loc", "bookmark", "at://keep"], b"1");
    let x = block_on(e.export_local(ExportScope::Mine)).unwrap();

    // the device changes after the export
    put(&host, &["loc", "bookmark", "at://added-later"], b"2");
    block_on(
        host.state()
            .write(vec![KvOp::Delete(lessdugin_core::store::key::encode(&[
                ("loc").into(),
                ("bookmark").into(),
                ("at://keep").into(),
            ]))]),
    )
    .unwrap();

    let report = block_on(e.import_local(&x, ExportScope::Mine)).unwrap();
    assert_eq!(report.imported, x.members.len());
    assert_eq!(report.undecodable, 0);
    // the imported key is back, and the key the export never mentioned is still there
    let after = block_on(e.export_local(ExportScope::Mine)).unwrap();
    let keys: Vec<Vec<String>> = after.members.iter().map(|m| m.parts.clone()).collect();
    assert!(
        keys.iter()
            .any(|p| p.get(2).is_some_and(|s| s == "at://keep")),
        "{keys:?}"
    );
    assert!(
        keys.iter()
            .any(|p| p.get(2).is_some_and(|s| s == "at://added-later")),
        "a merge must not delete what the export does not mention: {keys:?}"
    );
}

#[test]
fn importing_a_mine_export_cannot_write_another_accounts_state() {
    let (host, _pds, alice, bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    // craft an export that carries bob's ack, as a hand-edited file could
    let mut x = block_on(e.export_local(ExportScope::Mine)).unwrap();
    put(&host, &["loc", "ack", bob.did.as_str(), "vote"], b"1");
    x.members = block_on(e.export_local(ExportScope::Everything))
        .unwrap()
        .members;
    let report = block_on(e.import_local(&x, ExportScope::Mine)).unwrap();
    assert!(report.out_of_scope > 0, "{report:?}");
    // and bob's row never came back through a Mine import
    let mine = block_on(e.export_local(ExportScope::Mine)).unwrap();
    assert!(
        !mine
            .members
            .iter()
            .any(|m| m.parts.get(2).is_some_and(|s| s == bob.did.as_str())),
        "{:?}",
        mine.members
    );
}

#[test]
fn a_bad_member_is_counted_not_fatal() {
    let (host, _pds, alice, _bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    put(&host, &["loc", "karmaCadence"], b"daily");
    let mut x = block_on(e.export_local(ExportScope::Mine)).unwrap();
    let good = x.members.len();
    x.members.push(lessdugin_core::engine::ExportMember {
        key: "!!!not-base64!!!".into(),
        parts: vec![],
        value: "x".into(),
    });
    let report = block_on(e.import_local(&x, ExportScope::Mine)).unwrap();
    assert_eq!(report.imported, good);
    assert_eq!(report.undecodable, 1);
}

#[test]
fn a_future_export_version_is_refused() {
    let (host, _pds, alice, _bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    let mut x = block_on(e.export_local(ExportScope::Mine)).unwrap();
    x.version = EXPORT_VERSION + 1;
    let err = block_on(e.import_local(&x, ExportScope::Mine)).unwrap_err();
    assert!(err.message.contains("version"), "{}", err.message);
}

#[test]
fn every_local_family_round_trips() {
    // The families `social.md` §16 and `synthesis-v1.md` §6.12 name, one row each.
    let (host, _pds, alice, _bob) = setup();
    let e = engine(&host);
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), PDS))).unwrap();
    let did = alice.did.as_str();
    put(&host, &["loc", "follow", "fk"], b"1");
    put(&host, &["loc", "alias", "fk"], b"a");
    put(&host, &["loc", "read", "at://post"], b"1");
    put(&host, &["loc", "bookmark", "at://post"], b"1");
    put(&host, &["loc", "mute", did], b"1");
    put(&host, &["loc", "sub", r#"{"forum":"fk"}"#], b"1");
    put(&host, &["loc", "karmaCadence"], b"daily");
    put(&host, &["loc", "ack", did, "vote"], b"1");
    put(&host, &["loc", "notif", did, "n1"], b"1");
    put(&host, &["loc", "nbase", did, "fk"], b"1");
    put(&host, &["loc", "fvisit", "fk"], b"1");
    let x = block_on(e.export_local(ExportScope::Mine)).unwrap();
    let before = x.members.len();
    assert!(before >= 11, "expected every family, got {before}");
    let report = block_on(e.import_local(&x, ExportScope::Mine)).unwrap();
    assert_eq!(report.imported + report.out_of_scope, before);
    let again = block_on(e.export_local(ExportScope::Mine)).unwrap();
    assert_eq!(
        serde_json::to_string(&x.members).unwrap(),
        serde_json::to_string(&again.members).unwrap()
    );
}
