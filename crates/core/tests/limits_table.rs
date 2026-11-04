// SPDX-License-Identifier: AGPL-3.0-or-later
//! `limits.md` §21 item 1: every constant `crates/core/src/limits.rs` defines for a row of
//! `docs/spec/limits.md` MUST equal the row's value. This test parses the document's tables and
//! compares each named row with the constant of that name (a range row `a–b` with its `_MIN`
//! and `_MAX` pair, §21 item 2). The registry below must list every `pub const` of `limits.rs`
//! (checked against the source), so a new constant cannot escape the comparison.
//!
//! The rows that `limits.rs` does not mirror (§22 "Open questions": it mirrors part of the
//! table; other crates define some of the rest) are not compared here.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_core::limits::*;

const LIMITS_MD: &str = include_str!("../../../docs/spec/limits.md");
const LIMITS_RS: &str = include_str!("../src/limits.rs");

/// Every `pub const` of `limits.rs`, by name, as an integer.
macro_rules! registry {
    ($($c:ident),* $(,)?) => {
        BTreeMap::from([$((stringify!($c), $c as i128)),*])
    };
}

fn constants() -> BTreeMap<&'static str, i128> {
    registry![
        DEFAULT_RESPONSE_MAX_BYTES,
        RECORD_JSON_MAX_BYTES,
        LIST_PAGE_MAX_BYTES,
        PROOF_CAR_MAX_BYTES,
        INDEXER_BUNDLE_MAX_BYTES,
        BLOB_AUTOLOAD_MAX_BYTES,
        BLOB_HARD_MAX_BYTES,
        BLOB_TEXT_MAX_BYTES,
        BODY_BLOB_RETRY_MS,
        BODY_BLOB_WAIT_MS,
        BLOB_SNIFF_PREFIX_BYTES,
        MEDIA_WASM_WINDOW_BYTES,
        IMAGE_UPLOAD_TARGET_BYTES,
        IMAGE_UPLOAD_413_RETRIES,
        PREPARED_MEDIA_TTL_MS,
        JETSTREAM_MESSAGE_MAX_BYTES,
        CONNECT_TIMEOUT_MS,
        REQUEST_TIMEOUT_MS,
        CAR_TIMEOUT_MS,
        BLOB_IDLE_TIMEOUT_MS,
        BLOB_TOTAL_TIMEOUT_MS,
        MAX_REDIRECTS,
        IDENTITY_REDIRECTS,
        FETCH_CAP_DEFAULT,
        FETCH_CAP_OPINION,
        FUTURE_SKEW_MS,
        INFLIGHT_NATIVE,
        INFLIGHT_BROWSER,
        INFLIGHT_PER_HOST,
        INFLIGHT_PLC,
        INFLIGHT_DOH,
        DID_DOC_MAX_BYTES,
        WELLKNOWN_DID_MAX_BYTES,
        DOH_RESPONSE_MAX_BYTES,
        OAUTH_METADATA_MAX_BYTES,
        SMALL_RESPONSE_MAX_BYTES,
        RECORD_RESPONSE_MAX_BYTES,
        RETRY_MAX,
        RETRY_BASE_MS,
        RETRY_CAP_MS,
        RETRY_AFTER_MAX_WAIT_MS,
        NESTING_MAX_RECORD,
        NESTING_MAX_DOCUMENT,
        RECORD_CBOR_MAX_BYTES,
        TOMBSTONE_MIN_RETENTION_MS,
        REPO_MARKS_MAX,
        SPILL_THRESHOLD_BYTES,
        WRITE_BODY_MAX_BYTES,
        OPINION_HOLD_MS,
        SWAP_RETRY_MAX,
        SWAP_RETRY_JITTER_MIN_MS,
        SWAP_RETRY_JITTER_MAX_MS,
        APPLYWRITES_MAX_OPS,
        WRITE_BUDGET_POINTS_PER_HOUR,
        WRITE_BUDGET_POINTS_PER_DAY,
        WRITE_COST_CREATE,
        WRITE_COST_UPDATE,
        WRITE_COST_DELETE,
        LOCK_ALLOWED_MAX,
        THREAD_DEPTH_MAX,
        RENDER_CACHE_ENTRIES,
        LIST_PAGE_DEFAULT,
        LIST_PAGE_MAX,
        MODEL_MAX_AGE_MS,
        MODEL_CACHE_ENTRIES,
        FOLLOW_SEED_PAGES,
        FORUM_REOPEN_MS,
        FORUM_OPEN_SYNC_BUDGET_MS,
        COLD_PAGE_HOLD_MS,
        FORUM_DECL_CACHED_BUDGET_MS,
        FORUM_DECL_FIRST_BUDGET_MS,
        LIVE_TAIL_IDLE_MS,
        LIVE_TAIL_BATCH,
        HANDLE_VERIFY_BATCH,
        HANDLE_VERIFY_QUEUE_MAX,
        TOMBSTONE_GC_EVERY_MS,
        DISCUSSION_COMMENTS,
    ]
}

/// Rows whose `limits.rs` constant has another name: (row, constant). One row may stand for
/// several constants (`IDENTITY_RESPONSE_MAX_BYTES` caps each kind of identity answer).
const ALIASES: [(&str, &str); 5] = [
    ("REDIRECTS_MAX", "MAX_REDIRECTS"),
    ("HANDLE_WELLKNOWN_MAX_BYTES", "WELLKNOWN_DID_MAX_BYTES"),
    ("IDENTITY_RESPONSE_MAX_BYTES", "DID_DOC_MAX_BYTES"),
    ("IDENTITY_RESPONSE_MAX_BYTES", "DOH_RESPONSE_MAX_BYTES"),
    ("IDENTITY_RESPONSE_MAX_BYTES", "OAUTH_METADATA_MAX_BYTES"),
];

/// The constants of `limits.rs` that no row of `limits.md` names yet (`limits.md` §22 "Open
/// questions" lists them), so nothing here compares them. Every other constant must be
/// compared with a row.
const UNLISTED: [&str; 7] = [
    // the REDIRECTS_MAX row's prose: identity requests follow at most 3
    "IDENTITY_REDIRECTS",
    // sync.md §6.5 (marks: at most 1,000 per repo)
    "REPO_MARKS_MAX",
    // transport internals
    "SMALL_RESPONSE_MAX_BYTES",
    "RECORD_RESPONSE_MAX_BYTES",
    "RETRY_AFTER_MAX_WAIT_MS",
    // sync.md §4.2 declaration-fetch budgets
    "FORUM_DECL_CACHED_BUDGET_MS",
    "FORUM_DECL_FIRST_BUDGET_MS",
];

/// A row's value: one integer, or a range `a–b`.
#[derive(Debug, PartialEq)]
enum Value {
    One(i128),
    Range(i128, i128),
}

/// `4,194,304 (4 MiB)` → 4194304; `0–5,000` → (0, 5000); `None` for anything else.
fn parse_value(cell: &str) -> Option<Value> {
    let v = cell.split(" (").next()?.trim();
    let int = |s: &str| s.trim().replace(',', "").parse::<i128>().ok();
    match v.split_once('–') {
        Some((a, b)) => Some(Value::Range(int(a)?, int(b)?)),
        None => int(v).map(Value::One),
    }
}

/// Every table row of the document whose first cell names a constant: (name, value cell, line).
fn rows() -> Vec<(&'static str, &'static str, usize)> {
    LIMITS_MD
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let mut cells = line.strip_prefix("| `")?.splitn(3, '|');
            let name = cells.next()?.trim().strip_suffix('`')?;
            let value = cells.next()?.trim();
            name.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                .then_some((name, value, i + 1))
        })
        .collect()
}

/// The `_MIN`/`_MAX` constant names of a range row: `NAME_MIN`, or before a unit suffix
/// (`SWAP_RETRY_JITTER_MS` → `SWAP_RETRY_JITTER_MIN_MS`).
fn range_names(name: &str) -> Vec<(String, String)> {
    let mut out = vec![(format!("{name}_MIN"), format!("{name}_MAX"))];
    for unit in ["_MS", "_BYTES"] {
        if let Some(stem) = name.strip_suffix(unit) {
            out.push((format!("{stem}_MIN{unit}"), format!("{stem}_MAX{unit}")));
        }
    }
    out
}

#[test]
fn the_registry_lists_every_constant_of_limits_rs() {
    let declared: Vec<&str> = LIMITS_RS
        .lines()
        .filter_map(|l| l.strip_prefix("pub const ")?.split(':').next())
        .collect();
    let registry = constants();
    let listed: Vec<&str> = registry.keys().copied().collect();
    let mut declared_sorted = declared.clone();
    declared_sorted.sort_unstable();
    assert_eq!(listed, declared_sorted, "update the registry of this test");
}

/// Compares every row with the constants that mirror it: the names of the constants compared,
/// and one line per disagreement.
fn compare() -> (BTreeSet<String>, Vec<String>) {
    let c = constants();
    let mut compared = BTreeSet::new();
    let mut wrong = Vec::new();
    for (name, cell, line) in rows() {
        let aliased: Vec<&str> = ALIASES
            .iter()
            .filter(|(row, _)| *row == name)
            .map(|(_, rs)| *rs)
            .collect();
        let names = if aliased.is_empty() {
            vec![name]
        } else {
            aliased
        };
        let mut mirrored = false;
        for rs in names {
            let Some(&have) = c.get(rs) else { continue };
            mirrored = true;
            compared.insert(rs.to_owned());
            match parse_value(cell) {
                Some(Value::One(want)) if want == have => {}
                other => wrong.push(format!(
                    "limits.md:{line} {name} = {cell:?} ({other:?}), limits.rs {rs} {have}"
                )),
            }
        }
        if mirrored {
            continue;
        }
        let Some((lo, hi)) = range_names(name)
            .into_iter()
            .find(|(lo, hi)| c.contains_key(lo.as_str()) && c.contains_key(hi.as_str()))
        else {
            continue;
        };
        let have = Value::Range(c[lo.as_str()], c[hi.as_str()]);
        if parse_value(cell).as_ref() != Some(&have) {
            wrong.push(format!(
                "limits.md:{line} {name} = {cell:?}, limits.rs {lo}/{hi} {have:?}"
            ));
        }
        compared.insert(lo);
        compared.insert(hi);
    }
    (compared, wrong)
}

#[test]
fn every_row_limits_rs_mirrors_has_the_rows_value() {
    let (compared, wrong) = compare();
    assert!(
        wrong.is_empty(),
        "limits.rs disagrees with limits.md:\n{}",
        wrong.join("\n")
    );
    // the comparison is not vacuous: most of limits.rs mirrors a row
    assert!(compared.len() >= 40, "only {} compared", compared.len());
}

/// m3m4-continuation-6: a constant that no row names escaped the comparison, and the spec could
/// drift from it unseen (`BLOB_TOTAL_TIMEOUT_MS` had no row, and the `BLOB_IDLE_TIMEOUT_MS`
/// row said blob downloads have no total timeout). Every constant is compared with a row, or
/// listed in [`UNLISTED`]; an unlisted one has no row, so the list hides no comparison.
#[test]
fn every_constant_of_limits_rs_is_compared_or_listed_as_without_a_row() {
    let (compared, _) = compare();
    let uncompared: Vec<&str> = constants()
        .into_keys()
        .filter(|k| !compared.contains(*k) && !UNLISTED.contains(k))
        .collect();
    assert!(
        uncompared.is_empty(),
        "limits.rs constants no limits.md row names: {uncompared:?} (add the row, an alias, or \
         list them in UNLISTED and in limits.md §22)"
    );
    let named: BTreeSet<&str> = rows().into_iter().map(|(n, _, _)| n).collect();
    for k in UNLISTED {
        assert!(
            !compared.contains(k),
            "{k} is compared: remove it from UNLISTED"
        );
        assert!(!named.contains(k), "{k} has a row: remove it from UNLISTED");
        assert!(
            constants().contains_key(k),
            "{k} is not a constant of limits.rs"
        );
    }
}

#[test]
fn the_value_parser_reads_the_tables_forms() {
    assert_eq!(
        parse_value("4,194,304 (4 MiB)"),
        Some(Value::One(4_194_304))
    );
    assert_eq!(parse_value("1,048,576 (2^20)"), Some(Value::One(1_048_576)));
    assert_eq!(parse_value("0–5,000"), Some(Value::Range(0, 5_000)));
    assert_eq!(parse_value("[10, 1]"), None);
    assert!(rows().iter().any(|(n, _, _)| *n == "FETCH_CAP_DEFAULT"));
}
