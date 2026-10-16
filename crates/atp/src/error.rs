// SPDX-License-Identifier: MIT OR Apache-2.0
//! Crate-level error: every module error converts into [`Error`] with `?`.

use crate::car::CarError;
use crate::cid::CidError;
use crate::commit::CommitError;
use crate::crypto::CryptoError;
use crate::datamodel::DataModelError;
use crate::did_doc::DidDocError;
use crate::mst::MstError;
use crate::proof::ProofError;
use crate::syntax::SyntaxError;

/// Any error from this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Syntax validation.
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    /// CID parsing or verification.
    #[error(transparent)]
    Cid(#[from] CidError),
    /// Data model / DAG-CBOR.
    #[error(transparent)]
    DataModel(#[from] DataModelError),
    /// Keys and signatures.
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// DID documents.
    #[error(transparent)]
    DidDoc(#[from] DidDocError),
    /// CAR files.
    #[error(transparent)]
    Car(#[from] CarError),
    /// MST.
    #[error(transparent)]
    Mst(#[from] MstError),
    /// Commits.
    #[error(transparent)]
    Commit(#[from] CommitError),
    /// Proofs.
    #[error(transparent)]
    Proof(#[from] ProofError),
}
