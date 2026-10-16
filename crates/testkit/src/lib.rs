// SPDX-License-Identifier: AGPL-3.0-or-later
//! Test support for lessdugin: the shared JSON test-vector format and runner, an in-memory
//! host, a fake PDS with fault injection, and helpers for synthetic forums.
//!
//! | Module | Contents |
//! |---|---|
//! | [`memhost`] | [`MemHost`]: in-memory `Host` with a programmable HTTP router, fake clock, deterministic randomness |
//! | [`resp`] | response builders for routes |
//! | [`identity`] | [`FakeIdentity`], DID documents, DoH answers, route installers |
//! | [`repo`] | [`TestRepo`]: signed commits, MST, proof CARs |
//! | [`pds`] | [`FakePds`]: `sync.getRecord`, `repo.getRecord`, `getLatestCommit`, `resolveHandle` |
//! | [`kv`] | the shared `KvStore` contract suite |

#![forbid(unsafe_code)]

pub mod identity;
pub mod kv;
pub mod memhost;
pub mod pds;
pub mod repo;
pub mod resp;
pub mod vectors;

pub use identity::FakeIdentity;
pub use memhost::MemHost;
pub use pds::FakePds;
pub use repo::TestRepo;
