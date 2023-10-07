/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

//! "Page out" logic as an attempt to reduce RSS / Working Set usage.

use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

#[cfg(unix)]
use minibytes::Bytes;
use minibytes::WeakBytes;

/// See `crate::config::set_page_out_threshold`.
pub(crate) static THRESHOLD: AtomicI64 = AtomicI64::new(DEFAULT_THRESHOLD);

/// Remaining byte count to read without `page_out()`.
static AVAILABLE: AtomicI64 = AtomicI64::new(DEFAULT_THRESHOLD);

/// Tracked buffers. Also serve as a lock for `page_out()`.
static BUFFERS: Mutex<Vec<WeakBytes>> = Mutex::new(Vec::new());

/// By default, trigger `page_out()` after reading 2GB `Log` entries.
const DEFAULT_THRESHOLD: i64 = 1i64 << 31;

/// Adjust the `AVAILABLE`.
/// If it becomes negative when `THRESHOLD` is positive, trigger `page_out`.
pub(crate) fn adjust_available(delta: i64) {
    let old_available = AVAILABLE.fetch_add(delta as _, Ordering::AcqRel);
    if old_available + delta < 0 {
        let threshold = THRESHOLD.load(Ordering::Acquire);
        if threshold > 0 {
            let mut buffers = BUFFERS.lock().unwrap();
            AVAILABLE.store(threshold, Ordering::Release);
            tracing::info!("running page_out()");
            page_out(&mut buffers);
        }
    }
}

#[cfg(unix)]
pub(crate) fn track_mmap_buffer(bytes: &Bytes) {
    let threshold = THRESHOLD.load(Ordering::Acquire);
    if threshold > 0 {
        let mut buffers = BUFFERS.lock().unwrap();
        if let Some(weak) = bytes.downgrade() {
            buffers.push(weak);
        }
    }
}

#[cfg(unix)]
fn page_out(buffers: &mut Vec<WeakBytes>) {
    let mut new_buffers = Vec::new();
    for weak in buffers.drain(..) {
        let bytes = match Bytes::upgrade(&weak) {
            None => continue,
            Some(bytes) => bytes,
        };
        let slice: &[u8] = bytes.as_ref();
        #[cfg(unix)]
        let ret = unsafe {
            libc::madvise(
                slice.as_ptr() as *const libc::c_void as *mut libc::c_void,
                slice.len() as _,
                libc::MADV_DONTNEED,
            )
        };
        tracing::debug!(
            "madvise({} bytes, MADV_DONTNEED) returned {}",
            slice.len(),
            ret
        );
        new_buffers.push(weak);
    }
    *buffers = new_buffers;
}

#[cfg(windows)]
fn page_out(buffers: &mut Vec<WeakBytes>) {
    use winapi::um::processthreadsapi::GetCurrentProcess;
    use winapi::um::psapi::EmptyWorkingSet;

    unsafe {
        let handle = GetCurrentProcess();
        let ret = EmptyWorkingSet(handle);
        tracing::debug!("EmptyWorkingSet returned {}", ret);
    }

    buffers.clear();
}
