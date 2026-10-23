// SPDX-License-Identifier: AGPL-3.0-or-later
//! Followed forums follow the user across devices (`docs/plan.md` M2, `sync.md` §4.8): a forum
//! created (and so joined) on one device appears on a second device of the same account after
//! login, seeded from the account's own membership records, read anonymously from the PDS
//! bound at login (the native write allowlist, AUTH-4, refuses authenticated reads).

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, text};

#[test]
fn a_second_device_follows_the_forums_of_the_account() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let t = alice.json(&["forum", "create", "seedme", "--name", "Seed me"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");

    let device2 = net.second_device(&alice);
    let out = device2.run(&["--json", "forum", "ls"]);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let ls: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let forums = ls["forums"].as_array().unwrap();
    assert_eq!(forums.len(), 1, "{ls}");
    assert!(
        forums[0]["uri"].as_str().unwrap().ends_with("/seedme"),
        "{ls}"
    );
    assert!(
        !stderr.contains("WARN") && !stderr.contains("blocked by policy"),
        "seeding must not fail: {stderr}"
    );

    // The forum is the only followed one, so a post needs no --forum.
    let file = device2.file("p.md", "---\ntitle: From device 2\n---\nHello.\n");
    let out = device2.run(&["--json", "post", file.to_str().unwrap(), "--yes"]);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("WARN"), "{stderr}");
}
