// SPDX-License-Identifier: AGPL-3.0-or-later
//! `media add|get|open` and the draft-local images of a post through the real CLI on the
//! FakePds network (`support/fakenet.rs`): the core's pipeline prepares the files, publishing
//! rewrites their paths to `cid:` URLs (`lfm-1.md` §12) and uploads them, an image without alt
//! text is warned about (`data-model.md` §4), and a reader saves an item as `<cid>.<ext>`.

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, User, s, text};
use lessdugin_lexicon::collection::nsid;
use serde_json::Value;

/// A PNG signature and header: the pipeline sniffs it, and there is nothing to strip.
const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00";
const PDF: &[u8] = b"%PDF-1.4\n% a report\n%%EOF\n";

fn raw_cid(b: &[u8]) -> String {
    lessdugin_atp::cid::for_raw(b).to_string()
}

/// alice's forum.
fn forum() -> (Net, User) {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    (net, alice)
}

/// The only post record of `u`.
fn the_post(u: &User) -> Value {
    let posts = u.account.list(nsid::POST);
    assert_eq!(posts.len(), 1, "{posts:?}");
    u.account.get(nsid::POST, &posts[0].0).unwrap()
}

#[test]
fn draft_local_images_become_cid_media_and_save_as_cid_files() {
    let (net, alice) = forum();
    std::fs::create_dir(alice.path("img")).unwrap();
    std::fs::write(alice.path("img/chart.png"), PNG).unwrap();
    std::fs::write(alice.path("report.pdf"), PDF).unwrap();
    let f = alice.file(
        "p.md",
        "---\ntitle: Charts\n---\nLook: ![a chart](img/chart.png)\n\n\
         The report: ![](./report.pdf)\n\n`![not an image](./missing.png)`\n",
    );
    // relative paths are next to the post's file, whatever the working directory
    let out = alice
        .cmd()
        .current_dir(std::env::temp_dir())
        .args(["--json", "post", s(&f), "--yes"])
        .write_stdin("")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stderr = text(&out.stderr);
    // the image without alt text is warned about, the described one is not
    assert!(
        stderr.contains("./report.pdf") && stderr.contains("no alt text"),
        "{stderr}"
    );
    assert!(!stderr.contains("img/chart.png"), "{stderr}");
    let t: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let post = t["state"]["uri"].as_str().unwrap().to_owned();

    // the record names the blobs by CID; the local paths are not published
    let rec = the_post(&alice);
    let body = &rec["body"];
    let md = body["text"].as_str().unwrap();
    let (png, pdf) = (raw_cid(PNG), raw_cid(PDF));
    assert!(md.contains(&format!("![a chart](cid:{png})")), "{md}");
    assert!(md.contains(&format!("![](cid:{pdf})")), "{md}");
    assert!(!md.contains("img/chart.png") && !md.contains("./report.pdf"), "{md}");
    // a path in code is text, not an image
    assert!(md.contains("`![not an image](./missing.png)`"), "{md}");
    let media = body["media"].as_array().unwrap();
    assert_eq!(media.len(), 2, "{body}");
    assert_eq!(media[0]["blob"]["ref"]["$link"], png.as_str(), "{body}");
    assert_eq!(media[0]["blob"]["mimeType"], "image/png", "{body}");
    assert_eq!(media[0]["alt"], "a chart", "{body}");
    assert_eq!(media[1]["blob"]["ref"]["$link"], pdf.as_str(), "{body}");
    assert_eq!(media[1]["blob"]["mimeType"], "application/pdf", "{body}");
    assert!(media[1].get("alt").is_none(), "{body}");

    // a reader sees the numbered items, and saves them as <cid>.<ext> (CID-checked)
    let dave = net.reader(&alice);
    let read = dave.ok(&["read", &post, "--no-pager"]);
    assert!(read.contains("[image 1: a chart]"), "{read}");
    let into = dave.path("downloads");
    std::fs::create_dir(&into).unwrap();
    let g = dave.json(&["media", "get", &post, "--index", "1", "-o", s(&into)]);
    let want = into.join(format!("{png}.png"));
    assert_eq!(g["path"], want.display().to_string(), "{g}");
    assert_eq!(g["mime"], "image/png", "{g}");
    assert_eq!(std::fs::read(&want).unwrap(), PNG);
    // without -o the file is saved here, named by its CID and its sniffed type
    let here = dave.path("here");
    std::fs::create_dir(&here).unwrap();
    let out = dave
        .cmd()
        .current_dir(&here)
        .args(["media", "get", &post, "--index", "2"])
        .write_stdin("")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let names: Vec<String> = std::fs::read_dir(&here)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [format!("{pdf}.pdf")]);
    assert_eq!(std::fs::read(here.join(format!("{pdf}.pdf"))).unwrap(), PDF);
    // an item the body does not have
    let e = dave.fails(4, &["media", "get", &post, "--index", "3"]);
    assert_eq!(e["code"], "not_found", "{e}");
}

#[test]
fn media_add_prepares_a_file_that_a_post_names_by_cid() {
    let (_net, alice) = forum();
    let file = alice.path("dot.png");
    std::fs::write(&file, PNG).unwrap();
    let m = alice.json(&["media", "add", s(&file), "--alt", "a dot"]);
    let cid = raw_cid(PNG);
    assert_eq!(m["dto"], 1, "{m}");
    assert_eq!(m["cid"], cid.as_str(), "{m}");
    assert_eq!(m["mime"], "image/png", "{m}");
    assert_eq!(m["sizeBytes"], PNG.len(), "{m}");
    // the human output is the line to put in a post; no alt text is warned about
    let out = alice.run(&["media", "add", s(&file)]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains(&format!("![](cid:{cid})")),
        "{}",
        text(&out.stdout)
    );
    assert!(text(&out.stderr).contains("no alt text"), "{}", text(&out.stderr));

    let f = alice.file("p.md", &format!("---\ntitle: Dot\n---\n![a dot](cid:{cid})\n"));
    let p = alice.json(&["post", s(&f), "--yes"]);
    assert_eq!(p["state"]["kind"], "done", "{p}");
    let rec = the_post(&alice);
    let media = rec["body"]["media"].as_array().unwrap();
    assert_eq!(media.len(), 1, "{rec}");
    assert_eq!(media[0]["blob"]["ref"]["$link"], cid.as_str(), "{rec}");
    assert_eq!(media[0]["alt"], "a dot", "{rec}");
}

#[test]
fn a_missing_local_image_publishes_nothing() {
    let (_net, alice) = forum();
    let f = alice.file("p.md", "---\ntitle: Gone\n---\n![a chart](./gone.png)\n");
    let e = alice.fails(4, &["post", s(&f), "--yes"]);
    assert!(
        e["message"].as_str().unwrap().contains("gone.png"),
        "{e}"
    );
    assert!(alice.account.list(nsid::POST).is_empty());
}

#[test]
fn media_open_hands_the_saved_file_to_the_viewer() {
    let (net, alice) = forum();
    std::fs::write(alice.path("chart.png"), PNG).unwrap();
    let f = alice.file("p.md", "---\ntitle: Open\n---\n![a chart](chart.png)\n");
    let p = alice.json(&["post", s(&f), "--yes"]);
    let post = p["state"]["uri"].as_str().unwrap().to_owned();
    let dave = net.reader(&alice);
    // a "viewer" that records what it was asked to open
    let opened = dave.path("opened.txt");
    let viewer = dave.path("viewer.sh");
    std::fs::write(
        &viewer,
        format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", opened.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&viewer, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = dave
        .cmd()
        .env("BROWSER", &viewer)
        .args(["--json", "media", "open", &post, "--index", "1"])
        .write_stdin("")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let path = v["path"].as_str().unwrap().to_owned();
    assert!(path.ends_with(&format!("{}.png", raw_cid(PNG))), "{v}");
    assert_eq!(std::fs::read(&path).unwrap(), PNG);
    // the viewer runs in the background
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !opened.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let arg = std::fs::read_to_string(&opened).unwrap_or_default();
    assert!(arg.ends_with(&format!("{}.png", raw_cid(PNG))), "{arg:?}");
}
