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
//! | [`fake_pds`] | [`FakeNet`]: multi-repo PDS hosts with real repos, PLC, relays, DoH, writes, sessions, faults (feature `http`: real HTTP) |
//! | [`kv`] | the shared `KvStore` contract suite |
//! | [`store`] | the shared record-store scenario, run over any `KvStore` |
//! | [`node_devnet`] | the node devnet process for the `LESSDUGIN_E2E=1` tests (native only) |

#![forbid(unsafe_code)]

pub mod fake_pds;
pub mod identity;
pub mod kv;
pub mod memhost;
#[cfg(not(target_arch = "wasm32"))]
pub mod node_devnet;
pub mod pds;
pub mod repo;
pub mod resp;
pub mod store;
pub mod vectors;

pub use fake_pds::FakeNet;
pub use identity::FakeIdentity;
pub use memhost::MemHost;
pub use pds::FakePds;
pub use repo::TestRepo;
