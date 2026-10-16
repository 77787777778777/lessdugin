// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shared record-store scenario over the in-memory stores.

use futures::executor::block_on;
use lessdugin_core::host::Host;
use lessdugin_core::store::mem::MemKv;
use lessdugin_testkit::MemHost;
use lessdugin_testkit::store::record_store_conformance;

#[test]
fn record_store_over_memkv() {
    block_on(record_store_conformance(&MemKv::new()));
}

#[test]
fn record_store_over_memhost_cache() {
    let host = MemHost::new();
    block_on(record_store_conformance(host.cache()));
}
