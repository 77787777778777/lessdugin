// SPDX-License-Identifier: AGPL-3.0-or-later
//! `fsutil::read_regular_nofollow` on a FIFO: refused, without blocking the open. It is its own
//! test binary because the library forbids `unsafe` (so `mkfifo(3)`), and spawning `mkfifo(1)`
//! from the library's tests lets the child, between fork and exec, hold another test's flock.

#![cfg(unix)]

use std::ffi::CString;
use std::io;

use lessdugin_native::fsutil::read_regular_nofollow;

#[test]
fn a_fifo_is_refused_without_blocking_the_open() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo.png");
    let c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a NUL-terminated path that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert_eq!(
        read_regular_nofollow(&fifo, 10).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}
