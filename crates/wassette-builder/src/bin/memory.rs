// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(2 * 1024 * 1024 * 1024);

pub struct TransformAllocator;

fn reserve(bytes: usize) -> bool {
    LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        live.checked_add(bytes)
            .filter(|total| *total <= LIMIT.load(Ordering::Relaxed))
    })
    .is_ok()
}

pub fn allow_vm_memory() {
    // Account for the guest backing and initrd as well as SDK allocations;
    // even a misbehaving guest cannot grow Rust-owned SDK buffers indefinitely.
    LIMIT.store(4 * 1024 * 1024 * 1024, Ordering::Relaxed);
}

pub fn limit_transforms() {
    LIMIT.store(2 * 1024 * 1024 * 1024, Ordering::Relaxed);
}

// The temporary ceiling covers Rust allocations in request decoding, WIT
// parsing and bindgen on every supported OS (Darwin does not enforce RLIMIT_AS).
// Failed reservations use the standard allocator failure/abort path, so the
// supervisor reaps a failed job instead of continuing with partial bindings.
unsafe impl GlobalAlloc for TransformAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let ptr = unsafe { System.alloc(layout) };
        if ptr.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if ptr.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            System.dealloc(ptr, layout);
        }
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let extra = size.saturating_sub(layout.size());
        if !reserve(extra) {
            return std::ptr::null_mut();
        }
        let result = unsafe { System.realloc(ptr, layout, size) };
        if result.is_null() {
            LIVE.fetch_sub(extra, Ordering::Relaxed);
        } else if size < layout.size() {
            LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
        }
        result
    }
}
