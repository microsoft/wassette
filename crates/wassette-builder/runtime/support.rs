// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

// A single inline runtime needs one task slot, not the upstream weak C symbol
// used to combine several independently linked versions of wit-bindgen.
#[unsafe(no_mangle)]
extern "C" fn wasip3_task_set(ptr: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
    use core::sync::atomic::{AtomicPtr, Ordering};
    static TASK: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(core::ptr::null_mut());
    TASK.swap(ptr, Ordering::Relaxed)
}
