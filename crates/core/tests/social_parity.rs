// SPDX-License-Identifier: AGPL-3.0-or-later
//! `social.md` §6.3 P9 and §1.3 rule 6: native and wasm builds of the fold produce
//! byte-identical serialised output, on random inputs.
//!
//! [`parity_digest`] folds 24 random forums (`social_gen::random_spec`, whose draws are 64-bit
//! on every target, so both builds draw the same inputs) under four queries each, serialises
//! every `ForumView` (the projection of every vector field, then the whole view's `Debug`),
//! and hashes it all into one sha256. The test compares it with [`GOLDEN`], the value recorded
//! by a native run; the same test on wasm32 under node then proves the wasm build equal:
//!
//! ```sh
//! cargo test -p lessdugin-core --test social_parity                    # native
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test -p lessdugin-core --target wasm32-unknown-unknown --test social_parity   # wasm32
//! ```
//!
//! (add `--features final-ns` to both for the final namespace). When the fold's outputs or the
//! generator change on purpose, the native run prints the new digest: record it here, then run
//! the wasm32 command; never record a digest from the wasm side alone.

mod social_gen;

use lessdugin_lexicon::ns::NS;
use social_gen::{
    Rng, Size, build, check_validity, first_post, fold, queries, random_spec, serialize,
};
// Natively this is a libtest test; on wasm32 the same body runs under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// The parity digest per namespace (recorded natively; last re-recorded at M3 for CO5 comparing
/// only a valid root, `social.md` §20.1 item 51, and the projected `versions` and `edited`).
const GOLDEN: &[(&str, &str)] = &[
    (
        "online.7778777.lessdugin.temp",
        "993638987e37175f4cac6654e4b038176ab6152e0a9bb8f278559d4e9927466a",
    ),
    (
        "online.7778777.lessdugin",
        "cffcb857391d012a6893eefad78d439b8cd129ebd68376e8ffe3645c26aef594",
    ),
];

const SIZE: Size = Size {
    posts: 8,
    comments: 18,
    wikis: 5,
    sequences: 2,
    mods: 14,
    ops: 80,
};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// sha256 over the sha256 of every serialised view, in order.
fn parity_digest() -> (String, usize) {
    let mut all = Vec::new();
    let mut views = 0;
    let mut rng = Rng(0x005e_ed0f_9a11_7e57);
    for _ in 0..24 {
        let f = build(&random_spec(&mut rng, SIZE));
        let input = f.input();
        for q in queries(first_post(&f).as_deref()).into_iter().step_by(3) {
            let v = fold(&input, &q);
            check_validity(&f, &input, &v);
            all.extend_from_slice(&lessdugin_atp::cid::sha256(serialize(&v).as_bytes()));
            views += 1;
        }
    }
    (hex(&lessdugin_atp::cid::sha256(&all)), views)
}

#[test]
fn p9_native_equals_wasm() {
    let (got, views) = parity_digest();
    let want = GOLDEN.iter().find(|(ns, _)| *ns == NS).map(|(_, d)| *d);
    assert_eq!(
        want,
        Some(got.as_str()),
        "parity digest of {views} views under {NS}: {got} (record it in GOLDEN only from a \
         native run, after an intended change of the fold or the generator)"
    );
}
