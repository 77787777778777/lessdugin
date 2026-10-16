// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`Bridge`]: jacquard's client traits implemented over [`Net`], so that every request of the
//! OAuth library (protected-resource and authorization-server metadata, PAR, token, refresh,
//! revocation) passes the SSRF guard, the byte caps and the lessdugin User-Agent, and follows no
//! redirects.
//!
//! jacquard's `OAuthResolver` also wants an `IdentityResolver`. lessdugin resolves identities
//! with `lessdugin-core` (DoH, PLC, devnet rules, caching) and checks the login binding there,
//! so the facade never calls jacquard's identity helpers (`resolve_oauth`, `verify_issuer`,
//! `exchange_code`); the implementation below refuses, so an accidental call fails loudly.

use std::sync::Arc;

use jacquard_common::BosStr;
use jacquard_common::http_client::HttpClient;
use jacquard_common::types::did::Did as JDid;
use jacquard_common::types::string::Handle as JHandle;
use jacquard_identity::resolver::{DidDocResponse, IdentityError, IdentityResolver, ResolverOptions};
use jacquard_oauth::dpop::DpopExt;
use jacquard_oauth::resolver::OAuthResolver;
use lessdugin_core::host::HostError;
use lessdugin_core::limits;

use crate::host::Net;

/// Cap on any OAuth response (metadata, PAR, token): small JSON documents.
pub(crate) const OAUTH_RESPONSE_MAX_BYTES: u64 = limits::OAUTH_METADATA_MAX_BYTES;

/// The guarded HTTP client, as jacquard sees it.
#[derive(Clone)]
pub(crate) struct Bridge {
    net: Net,
    options: Arc<ResolverOptions>,
}

impl Bridge {
    pub(crate) fn new(net: Net) -> Self {
        Self {
            net,
            options: Arc::new(ResolverOptions::default()),
        }
    }

    pub(crate) fn net(&self) -> &Net {
        &self.net
    }
}

/// A host error inside jacquard's error chains.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct BridgeError(pub HostError);

impl HttpClient for Bridge {
    type Error = BridgeError;

    async fn send_http(
        &self,
        request: http::Request<Vec<u8>>,
    ) -> Result<http::Response<Vec<u8>>, BridgeError> {
        self.net
            .send_http(
                request,
                OAUTH_RESPONSE_MAX_BYTES,
                limits::REQUEST_TIMEOUT_MS,
            )
            .await
            .map_err(BridgeError)
    }
}

fn refused() -> IdentityError {
    IdentityError::xrpc("identity resolution goes through lessdugin-core, not jacquard")
}

impl IdentityResolver for Bridge {
    fn options(&self) -> &ResolverOptions {
        &self.options
    }

    async fn resolve_handle<S: BosStr + Sync>(
        &self,
        _handle: &JHandle<S>,
    ) -> Result<JDid, IdentityError> {
        Err(refused())
    }

    async fn resolve_did_doc<S: BosStr + Sync>(
        &self,
        _did: &JDid<S>,
    ) -> Result<DidDocResponse, IdentityError> {
        Err(refused())
    }
}

impl OAuthResolver for Bridge {}

impl DpopExt for Bridge {}
