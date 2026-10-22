// SPDX-License-Identifier: MIT OR Apache-2.0
//! `testdata/vectors/limits/graphemes.json`: grapheme counting for `maxGraphemes` is pinned
//! (`data-model.md` §2.2). Each vector's text is counted with the crate the validator uses, and its
//! boundary is checked through validity rule G2 on `post.title` (`s(300g/3000b)`).

mod common;

// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

use common::nsid;
use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::validity::{RuleId, check_uri};
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

const TITLE_MAX_GRAPHEMES: u64 = 300;
const TITLE_MAX_BYTES: u64 = 3000;

fn text_of(input: &Value) -> String {
    if let Some(t) = input["text"].as_str() {
        return t.to_owned();
    }
    let g = &input["gen"];
    let s = |k: &str| g[k].as_str().unwrap_or_else(|| panic!("gen.{k}"));
    let times = usize::try_from(g["times"].as_u64().expect("gen.times")).unwrap();
    format!(
        "{}{}{}",
        s("prefix"),
        s("repeat").repeat(times),
        s("suffix")
    )
}

fn post_with_title(title: &str) -> (AtUri, Value) {
    let forum = format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{}/rationality",
        nsid("forum")
    );
    let rkey = format!("{}-3l2s5xxv2ze2c", keys::fk(&forum).unwrap());
    let uri = AtUri::parse(&format!(
        "at://did:plc:abcdefghijklmnopqrstuvwx/{}/{rkey}",
        nsid("post")
    ))
    .unwrap();
    let v = json!({"$type": nsid("post"), "forum": forum, "title": title,
                   "createdAt": "2026-10-16T12:00:00.000Z"});
    (uri, v)
}

#[test]
fn unicode_version_is_pinned() {
    // a new Unicode version is a behaviour change (data-model.md §2.2)
    assert_eq!(unicode_segmentation::UNICODE_VERSION, (17, 0, 0));
}

#[test]
fn grapheme_vectors() {
    let doc: Value =
        serde_json::from_str(&common::text("testdata/vectors/limits/graphemes.json")).unwrap();
    assert_eq!(doc["behaviour"], "data-model");
    assert_eq!(doc["kind"], "graphemes");
    let vectors = doc["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 30, "{}", vectors.len());
    let mut names = std::collections::HashSet::new();
    let (mut at_limit, mut past_limit) = (0, 0);
    for vector in vectors {
        let name = vector["name"].as_str().unwrap();
        assert!(names.insert(name), "duplicate name {name}");
        let text = text_of(&vector["input"]);
        let exp = &vector["expected"];
        let graphemes = text.graphemes(true).count() as u64;
        let bytes = text.len() as u64;
        assert_eq!(
            Some(graphemes),
            exp["graphemes"].as_u64(),
            "{name}: graphemes"
        );
        assert_eq!(Some(bytes), exp["bytes"].as_u64(), "{name}: bytes");

        let (uri, v) = post_with_title(&text);
        let fits = graphemes <= TITLE_MAX_GRAPHEMES && bytes <= TITLE_MAX_BYTES;
        let validity = check_uri(&uri, &v);
        assert_eq!(
            !validity.fails(RuleId::G2),
            fits,
            "{name}: {:?}",
            validity.violations
        );
        if name.starts_with("title-") {
            if fits {
                at_limit += 1;
            } else {
                past_limit += 1;
            }
        }
    }
    // the boundary is exercised from both sides
    assert!(at_limit >= 5 && past_limit >= 5, "{at_limit} {past_limit}");
}
