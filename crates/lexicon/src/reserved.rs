// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reserved names (`data-model.md` §1.5, §9). v1 readers ignore them (they are unknown fields or
//! collections); writers never write them (W10). [`reserved_fields_in`] lets a writer refuse a
//! value that carries one.

use serde_json::Value;

use crate::collection::Collection;
use crate::values::ActionKind;

/// Reserved collection names (short names under `NS.`), with their phase.
pub const RESERVED_COLLECTIONS: &[(&str, &str)] = &[
    ("report", "P2"),
    ("coauthor", "P2"),
    ("subscription", "P2"),
    ("turn", "P8+"),
    ("pollVote", "P8+"),
    ("collection", "P8+"),
    ("karmaSnapshot", "P7"),
    ("observation", "later"),
];

/// Group names that are never record names (§1.5).
pub const RESERVED_GROUPS: &[&str] = &["index", "temp"];

/// Reserved behaviour IDs (P2).
pub const RESERVED_BEHAVIOURS: &[&str] = &["ratelimit/1", "karma/2", "magic/2"];

/// `true` if `short` may never be (or is not yet) a record collection name.
pub fn is_reserved_collection_name(short: &str) -> bool {
    RESERVED_GROUPS.contains(&short) || RESERVED_COLLECTIONS.iter().any(|(n, _)| *n == short)
}

/// A reserved field: the collection, the JSON path inside the record and its phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservedField {
    /// The collection.
    pub collection: Collection,
    /// Path of object keys from the record root; `["action", "keep"]` means the field of the
    /// action object (for the given action kind).
    pub path: &'static [&'static str],
    /// For fields of a modAction action: the action they belong to.
    pub action: Option<ActionKind>,
    /// Phase.
    pub phase: &'static str,
}

/// Every reserved field of §9.
pub const RESERVED_FIELDS: &[ReservedField] = &[
    ReservedField {
        collection: Collection::Post,
        path: &["coauthors"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::Post,
        path: &["event"],
        action: None,
        phase: "P8+",
    },
    ReservedField {
        collection: Collection::Post,
        path: &["poll"],
        action: None,
        phase: "P8+",
    },
    ReservedField {
        collection: Collection::Comment,
        path: &["answer"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::Profile,
        path: &["moderationGuidelines"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::Forum,
        path: &["successor"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::Forum,
        path: &["aliases"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::Forum,
        path: &["sensitive"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::Forum,
        path: &["policy", "labelers"],
        action: None,
        phase: "P2",
    },
    ReservedField {
        collection: Collection::ModAction,
        path: &["action", "keep"],
        action: Some(ActionKind::Access),
        phase: "P2",
    },
    ReservedField {
        collection: Collection::ModAction,
        path: &["action", "aliasOf"],
        action: Some(ActionKind::Tag),
        phase: "P2",
    },
];

/// The JSON paths (`$.policy.labelers`) of every reserved field present in a record `value` of
/// `collection`. Writers refuse such values (W10). Reserved key forms (modAction `{fk}-{tid}`)
/// and the reserved `#lock` forum subject are checked by [`crate::validity`] instead.
pub fn reserved_fields_in(collection: Collection, value: &Value) -> Vec<String> {
    let action_kind = value
        .get("action")
        .and_then(|a| a.get("$type"))
        .and_then(Value::as_str)
        .and_then(ActionKind::from_type);
    RESERVED_FIELDS
        .iter()
        .filter(|f| f.collection == collection)
        .filter(|f| f.action.is_none() || f.action == action_kind)
        .filter(|f| {
            let mut cur = value;
            for k in f.path {
                match cur.get(k) {
                    Some(v) => cur = v,
                    None => return false,
                }
            }
            true
        })
        .map(|f| format!("$.{}", f.path.join(".")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names() {
        assert!(is_reserved_collection_name("report"));
        assert!(is_reserved_collection_name("index"));
        assert!(!is_reserved_collection_name("post"));
        for c in Collection::ALL {
            assert!(!is_reserved_collection_name(c.short_name()));
        }
    }

    #[test]
    fn fields() {
        let ns = crate::ns::NS;
        assert_eq!(
            reserved_fields_in(
                Collection::Forum,
                &json!({"policy": {"labelers": []}, "name": "x"})
            ),
            vec!["$.policy.labelers"]
        );
        assert!(reserved_fields_in(Collection::Post, &json!({"title": "x"})).is_empty());
        assert_eq!(
            reserved_fields_in(Collection::Post, &json!({"poll": {}, "coauthors": []})),
            vec!["$.coauthors", "$.poll"]
        );
        let access = json!({"action": {"$type": format!("{ns}.modAction#access"), "keep": true}});
        assert_eq!(
            reserved_fields_in(Collection::ModAction, &access),
            vec!["$.action.keep"]
        );
        let hide = json!({"action": {"$type": format!("{ns}.modAction#hide"), "keep": true}});
        assert!(reserved_fields_in(Collection::ModAction, &hide).is_empty());
    }
}
