// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forum directory (`sync.md` §4.7, `engine-api.md` §3.9 `forum_discover`): relay owners'
//! listings ∪ a consented indexer's `listForums`, each forum read from its owner's PDS, grouped
//! by owner (m5-acceptance-11).

mod common;

use common::block_on;
use lessdugin_atp::syntax::Datetime;
use lessdugin_core::discovery::sources::LIST_REPOS_BY_COLLECTION;
use lessdugin_core::dto::ForumSummary;
use lessdugin_core::engine::{Engine, EngineConfig, IndexerConsent, Sources};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::Forum;
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost, identity, resp};
use serde_json::{Value, json};
use url::Url;

const INDEX: &str = "https://index.example.com";

fn endpoint(method: &str) -> String {
    format!(
        "{INDEX}/xrpc/{}",
        lessdugin_core::indexer::contract::method_nsid(method)
    )
}

fn metadata() -> Value {
    json!({
        "asOf": {"seq": 1, "timeUs": 1_760_000_000_000_000i64, "lagMs": 0},
        "indexer": {"did": "did:web:index.example.com", "coreVersion": "test"}
    })
}

/// A consented indexer's `describeIndexer`, advertising `listForums`.
fn describe(host: &MemHost) {
    let mut describe = json!({
        "did": "did:web:index.example.com",
        "version": "1",
        "coreVersion": "test",
        "policies": [],
        "features": ["describeIndexer", "listForums"],
        "forums": [],
    });
    describe
        .as_object_mut()
        .unwrap()
        .extend(metadata().as_object().unwrap().clone());
    host.respond_get(&endpoint("describeIndexer"), resp::ok_json(&describe));
}

/// A consented indexer that advertises `listForums` and lists `uris`, two per page.
fn indexer(host: &MemHost, uris: Vec<String>) {
    describe(host);
    host.on_get(&endpoint("listForums"), move |req| {
        let url = match &req.target {
            lessdugin_core::host::Target::Anon(url) => url,
            lessdugin_core::host::Target::UserPds { .. } => panic!("indexer call with credentials"),
        };
        let at: usize = url
            .query_pairs()
            .find(|(k, _)| k == "cursor")
            .map_or(0, |(_, v)| v.parse().unwrap());
        let end = (at + 2).min(uris.len());
        let forums: Vec<Value> = uris[at..end]
            .iter()
            // the hinted text is never shown: only the uri's owner is used
            .map(|uri| json!({"ref": {"uri": uri, "value": {"name": "Hinted name"}}}))
            .collect();
        let mut page =
            json!({"forums": forums, "cursor": (end < uris.len()).then(|| end.to_string())});
        page.as_object_mut()
            .unwrap()
            .extend(metadata().as_object().unwrap().clone());
        Ok(resp::ok_json(&page))
    });
}

fn declare(owner: &AccountHandle, slug: &str, name: &str, at: &Datetime) -> String {
    let mut decl = Forum::create(slug, at.clone(), MembershipMode::Open);
    decl.name = name.to_owned();
    owner.put(nsid::FORUM, slug, &serde_json::to_value(decl).unwrap());
    owner.uri(nsid::FORUM, slug)
}

fn engine(host: &MemHost, net: &FakeNet, relays: bool, indexer: bool) -> Engine<MemHost> {
    Engine::new(
        host.clone(),
        EngineConfig {
            sources: Sources::Assisted,
            relays: if relays {
                net.relays()
                    .into_iter()
                    .map(|r| Url::parse(&r).unwrap())
                    .collect()
            } else {
                Vec::new()
            },
            indexers: if indexer {
                vec![IndexerConsent {
                    url: Url::parse(INDEX).unwrap(),
                }]
            } else {
                Vec::new()
            },
            ..EngineConfig::default()
        },
    )
}

fn listed(found: &[ForumSummary]) -> Vec<(String, String)> {
    found
        .iter()
        .map(|f| (f.owner.did.clone(), f.name.clone()))
        .collect()
}

/// With an indexer and no relay, the indexer's `listForums` names the owners; each forum shown
/// is the owner's PDS declaration (not the hinted text), and a hinted forum the owner's PDS does
/// not have is not shown.
#[test]
fn the_directory_uses_indexer_owner_hints_read_from_each_owners_pds() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    let meta = declare(&owner, "meta", "Meta", &at);
    let ghost = format!(
        "at://{}/{}/ghost",
        net.add_account("ghost.example.com").did(),
        nsid::FORUM
    );
    indexer(&host, vec![meta.clone(), ghost]);

    let without = engine(&host, &net, false, false);
    assert!(block_on(without.forum_discover("")).unwrap().is_empty());

    let with = engine(&host, &net, false, true);
    let found = block_on(with.forum_discover("")).unwrap();
    assert_eq!(
        listed(&found),
        [(owner.did().to_string(), "Meta".to_owned())]
    );
    assert_eq!(found[0].uri, meta);
    assert!(net.requests().iter().any(|r| {
        r.nsid.as_deref() == Some("com.atproto.repo.listRecords")
            && r.did.as_deref() == Some(owner.did().as_str())
    }));
    assert!(
        block_on(with.forum_discover("hinted")).unwrap().is_empty(),
        "the indexer's text is never matched or shown"
    );
}

/// Relay and indexer owners are merged without duplicates, and the result is grouped by owner
/// (verified handle, else DID), then sorted by name: forums of one owner are never interleaved
/// with another's, whatever their names (F9).
#[test]
fn the_directory_merges_relay_and_indexer_owners_grouped_by_owner() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let one = net.add_account("one.example.com");
    let two = net.add_account("two.example.com");
    let three = net.add_account("three.example.com");
    let mut owners = [one, two, three];
    owners.sort_by(|a, b| a.did().as_str().cmp(b.did().as_str()));
    let [first, second, third] = owners;
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    // names interleave across owners: a sort by name alone would mix the groups
    declare(&first, "zebra", "Zebra", &at);
    declare(&first, "apple", "Apple", &at);
    declare(&second, "mango", "Mango", &at);
    let berry = declare(&second, "berry", "Berry", &at);
    let kiwi = declare(&third, "kiwi", "Kiwi", &at);
    // the indexer names owners the relays name too: each forum is listed once
    indexer(&host, vec![kiwi, berry]);

    let engine = engine(&host, &net, true, true);
    let found = block_on(engine.forum_discover("")).unwrap();
    let did = |a: &AccountHandle| a.did().to_string();
    let mut forums = listed(&found);
    forums.sort();
    let mut expected = vec![
        (did(&first), "Apple".to_owned()),
        (did(&first), "Zebra".to_owned()),
        (did(&second), "Berry".to_owned()),
        (did(&second), "Mango".to_owned()),
        (did(&third), "Kiwi".to_owned()),
    ];
    expected.sort();
    assert_eq!(forums, expected, "every forum once");
    // grouped: owners with a verified handle first, by handle, then by DID; then by name
    let keys: Vec<_> = found
        .iter()
        .map(|f| {
            (
                f.owner.handle.is_none(),
                f.owner.handle.clone().unwrap_or_default(),
                f.owner.did.clone(),
                f.name.to_lowercase(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    let groups: Vec<&str> = found.iter().map(|f| f.owner.did.as_str()).collect();
    let mut runs = groups.clone();
    runs.dedup();
    assert_eq!(runs.len(), 3, "one run per owner: {groups:?}");
}

/// A consented indexer whose `listForums` fails.
fn failing_indexer(host: &MemHost) {
    describe(host);
    host.respond_get(
        &endpoint("listForums"),
        resp::xrpc_error(503, "Unavailable", "down"),
    );
}

/// Every relay of `net` fails its listing.
fn failing_relays(host: &MemHost, net: &FakeNet) {
    for relay in net.relays() {
        host.respond_get(
            &format!("{relay}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
            resp::xrpc_error(503, "Unavailable", "down"),
        );
    }
}

/// Either source may fail alone: the directory shows what the other answered, an empty list
/// included, and fails only when no configured source answered (sync.md §4.7).
#[test]
fn the_directory_fails_only_when_no_configured_source_answers() {
    // the relays answer with no forum owner, the indexer fails: an empty directory
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    failing_indexer(&host);
    let found = block_on(engine(&host, &net, true, true).forum_discover("")).unwrap();
    assert!(found.is_empty(), "{found:?}");
    // ... and with no relay configured, the failing indexer is the only source: an error
    assert!(
        block_on(engine(&host, &net, false, true).forum_discover("")).is_err(),
        "no configured source answered"
    );

    // the relays fail, the indexer answers: its owners' forums
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    let meta = declare(&owner, "meta", "Meta", &at);
    failing_relays(&host, &net);
    indexer(&host, vec![meta.clone()]);
    let found = block_on(engine(&host, &net, true, true).forum_discover("")).unwrap();
    assert_eq!(found.iter().map(|f| &f.uri).collect::<Vec<_>>(), [&meta]);
    // without the indexer, the failing relays are the only source: an error
    assert!(block_on(engine(&host, &net, true, false).forum_discover("")).is_err());

    // both fail: an error
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    failing_relays(&host, &net);
    failing_indexer(&host);
    assert!(block_on(engine(&host, &net, true, true).forum_discover("")).is_err());
}

/// The owner groups: owners with a verified handle first, by handle, then the others by DID.
#[test]
fn directory_groups_put_verified_handles_first_by_handle_then_dids() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let zulu = net.add_account("zulu.example.com");
    let alpha = net.add_account("alpha.example.com");
    let mike = net.add_account("mike.example.com");
    let kilo = net.add_account("kilo.example.com");
    // two handles that do not verify (no `_atproto` record, no well-known answer)
    for handle in ["mike.example.com", "kilo.example.com"] {
        for doh in [identity::DOH, identity::DOH2] {
            identity::install_nxdomain(&host, doh, handle);
        }
    }
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    for (i, owner) in [&zulu, &alpha, &mike, &kilo].into_iter().enumerate() {
        declare(owner, &format!("f{i}"), "Same name", &at);
    }
    let engine = engine(&host, &net, true, false);
    // the first listing queues the owners shown for handle verification (sync.md §3.3)
    block_on(engine.forum_discover("")).unwrap();
    block_on(engine.verify_handles(10));
    let found = block_on(engine.forum_discover("")).unwrap();
    let order: Vec<&str> = found.iter().map(|f| f.owner.did.as_str()).collect();
    let mut unverified = [mike.did().as_str(), kilo.did().as_str()];
    unverified.sort();
    let expected = [
        alpha.did().as_str(),
        zulu.did().as_str(),
        unverified[0],
        unverified[1],
    ];
    assert_eq!(order, expected);
    let handles: Vec<Option<&str>> = found.iter().map(|f| f.owner.handle.as_deref()).collect();
    assert_eq!(
        handles,
        [
            Some("alpha.example.com"),
            Some("zulu.example.com"),
            None,
            None
        ]
    );
    // the rule decides the order, not the DIDs: a sort by DID alone gives another one
    let mut by_did = expected;
    by_did.sort();
    assert_ne!(
        by_did, expected,
        "pick handles whose order differs from the DIDs'"
    );
}
