// SPDX-License-Identifier: AGPL-3.0-or-later
//! Row values: canonical DAG-CBOR maps written from serde structs (`sync.md` §10.5). Every row
//! type carries a version field `v` (currently 1); readers treat an undecodable or unknown-version
//! row as absent, which is always safe for the disposable `cache` store (I1).
//!
//! Every row of both stores is written with [`lessdugin_atp::dagcbor::encode_with`] and read with
//! [`lessdugin_atp::dagcbor::decode_with`] under the one shared [`ROW_LIMITS`]. The encoder enforces
//! the same caps as the decoder, so a row that could not be read back fails when it is written,
//! never silently later.

use lessdugin_atp::Ipld;
use lessdugin_atp::datamodel::Limits;
use lessdugin_atp::ipld_core::serde::{from_ipld, to_ipld};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The current row version.
pub const ROW_VERSION: u32 = 1;

/// The largest value a [`crate::host::KvStore`] must accept (`sync.md` §12.4: 2 MiB).
pub const KV_VALUE_MAX_BYTES: usize = 2 * 1024 * 1024;

/// The caps every store row is encoded and decoded under.
///
/// Rows are shallow (a `RepoRow`'s `capped` map is the deepest, at 3), hold at most a few thousand
/// entries (`marks` ≤ 1,000) and embed at most one record (≤ about 1 MB of DAG-CBOR, kept as one
/// byte string), so these caps only ever stop corrupt or foreign data.
pub const ROW_LIMITS: Limits = Limits {
    max_depth: 16,
    max_container_len: 131_072,
    max_string_bytes: 1 << 20,
    max_bytes_len: KV_VALUE_MAX_BYTES,
    max_total_nodes: 262_144,
};

/// Row encoding/decoding errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    /// The value could not be converted to or from the IPLD data model.
    #[error("row serde: {0}")]
    Serde(String),
    /// The bytes are not canonical DAG-CBOR (or the value cannot be encoded).
    #[error("row cbor: {0}")]
    Cbor(String),
}

/// Encodes a row as canonical DAG-CBOR under [`ROW_LIMITS`].
pub fn encode<T: Serialize>(row: &T) -> Result<Vec<u8>, RowError> {
    let ipld = to_ipld(row).map_err(|e| RowError::Serde(e.to_string()))?;
    lessdugin_atp::dagcbor::encode_with(&ipld, &ROW_LIMITS)
        .map_err(|e| RowError::Cbor(e.to_string()))
}

/// Decodes a row from canonical DAG-CBOR under [`ROW_LIMITS`].
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RowError> {
    let ipld: Ipld = lessdugin_atp::dagcbor::decode_with(bytes, &ROW_LIMITS)
        .map_err(|e| RowError::Cbor(e.to_string()))?;
    from_ipld(ipld).map_err(|e| RowError::Serde(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        v: u32,
        name: String,
        #[serde(with = "serde_bytes")]
        doc: Vec<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opt: Option<String>,
        at_ms: i64,
        flag: bool,
    }

    #[test]
    fn roundtrip_and_canonical() {
        let r = Row {
            v: ROW_VERSION,
            name: "x".into(),
            doc: b"{\"id\":1}".to_vec(),
            opt: None,
            at_ms: -5,
            flag: true,
        };
        let bytes = encode(&r).unwrap();
        assert_eq!(decode::<Row>(&bytes).unwrap(), r);
        // canonical: re-encoding the decoded IPLD gives the same bytes
        let ipld = lessdugin_atp::dagcbor::decode(&bytes).unwrap();
        assert_eq!(lessdugin_atp::dagcbor::encode(&ipld).unwrap(), bytes);
        assert!(decode::<Row>(b"\xff").is_err());
        assert!(decode::<Row>(&encode(&"just a string").unwrap()).is_err());
    }

    #[test]
    fn limits_apply_on_write_and_read() {
        // A byte string above the row cap cannot be written…
        #[derive(Serialize)]
        struct Big {
            #[serde(with = "serde_bytes")]
            b: Vec<u8>,
        }
        assert!(matches!(
            encode(&Big {
                b: vec![0; KV_VALUE_MAX_BYTES + 1]
            }),
            Err(RowError::Cbor(_))
        ));
        // …a record-sized one can, and reads back.
        let ok = encode(&Big {
            b: vec![7; 1_000_100],
        })
        .unwrap();
        let back: Ipld = lessdugin_atp::dagcbor::decode_with(&ok, &ROW_LIMITS).unwrap();
        assert!(matches!(back, Ipld::Map(_)));
        // Nesting deeper than the cap is refused in both directions.
        let mut deep = Ipld::Null;
        for _ in 0..ROW_LIMITS.max_depth + 1 {
            deep = Ipld::List(vec![deep]);
        }
        assert!(encode(&deep).is_err());
        let bytes =
            lessdugin_atp::dagcbor::encode_with(&deep, &Limits::uncapped(ROW_LIMITS.max_depth + 1))
                .unwrap();
        assert!(decode::<Ipld>(&bytes).is_err());
    }
}
