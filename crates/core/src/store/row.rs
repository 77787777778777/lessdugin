// SPDX-License-Identifier: AGPL-3.0-or-later
//! Row values: canonical DAG-CBOR maps written from serde structs (`sync.md` §10.5). Every row
//! type carries a version field `v` (currently 1); readers treat an undecodable or unknown-version
//! row as absent, which is always safe for the disposable `cache` store (I1).

use lessdugin_atp::Ipld;
use lessdugin_atp::ipld_core::serde::{from_ipld, to_ipld};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The current row version.
pub const ROW_VERSION: u32 = 1;

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

/// Encodes a row as canonical DAG-CBOR.
pub fn encode<T: Serialize>(row: &T) -> Result<Vec<u8>, RowError> {
    let ipld = to_ipld(row).map_err(|e| RowError::Serde(e.to_string()))?;
    lessdugin_atp::dagcbor::encode(&ipld).map_err(|e| RowError::Cbor(e.to_string()))
}

/// Decodes a row from canonical DAG-CBOR.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RowError> {
    let ipld: Ipld =
        lessdugin_atp::dagcbor::decode(bytes).map_err(|e| RowError::Cbor(e.to_string()))?;
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
}
