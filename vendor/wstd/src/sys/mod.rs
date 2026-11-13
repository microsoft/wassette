//! Platform-specific backends.
//!
//! Each supported target provides an implementation under `src/sys/`, selected
//! here by a single `cfg-if`. The crate-root modules (`crate::time`,
//! `crate::io`, ...) are target-agnostic facades, free of `#[cfg]`, written
//! against the `crate::sys::*` items the selected backend provides. There is no
//! shared `trait`: backends are duck-typed in the `std::sys` style, and the
//! `const _` assertions below check the shapes the facades depend on.
//!
//! Backend modules: `time`, `io`, `net`, `http`, `rand`, `runtime`. The reified
//! pollable types (`AsyncPollable`, `WaitFor`) are p2-only and intentionally
//! left out of the common contract; once a second backend lands they become a
//! localized escape hatch rather than facade `#[cfg]`s.

cfg_if::cfg_if! {
    if #[cfg(wstd_p3)] {
        mod p3;
        use p3 as backend;
    } else if #[cfg(wstd_p2)] {
        mod p2;
        use p2 as backend;
    } else {
        compile_error!("unsupported target: wstd requires a WASI target with either the `wasip2` or `wasip3` feature");
    }
}

pub use backend::*;

// Check the selected backend provides the shapes the facades rely on, so drift
// fails here instead of deep inside a facade.
const _: fn() = || {
    fn assert_async_read<T: crate::io::AsyncRead>() {}
    fn assert_async_write<T: crate::io::AsyncWrite>() {}
    fn assert_sleep_future<T: core::future::Future<Output = ()>>() {}

    assert_async_read::<crate::sys::io::AsyncInputStream>();
    assert_async_write::<crate::sys::io::AsyncOutputStream>();
    assert_sleep_future::<crate::sys::time::Sleep>();

    let _: fn() -> crate::sys::time::MonotonicInstant = crate::sys::time::now;
    let _: fn(crate::sys::time::MonotonicInstant) -> crate::sys::time::Sleep =
        crate::sys::time::sleep_until;
};
