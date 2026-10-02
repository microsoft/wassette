//! Async event loop support.
//!
//! The way to use this is to call [`block_on()`]. Inside the future, [`Reactor::current`]
//! will give an instance of the [`Reactor`] running the event loop, which can be
//! to [`AsyncPollable::wait_for`] instances of
//! [`wasip2::Pollable`](https://docs.rs/wasi/latest/wasi/io/poll/struct.Pollable.html).
//! This will automatically wait for the futures to resolve, and call the
//! necessary wakers to work.

pub use ::async_task::Task;

#[cfg(wstd_p3)]
#[doc(hidden)]
pub use crate::sys::runtime::{__MainReturn, __finish_main};
#[cfg(wstd_p2)]
pub use crate::sys::runtime::{AsyncPollable, WaitFor};
pub use crate::sys::runtime::{Reactor, block_on};

/// Spawn a `Future` as a `Task` on the current `Reactor`.
///
/// Panics if called from outside `block_on`.
pub fn spawn<F, T>(fut: F) -> Task<T>
where
    F: std::future::Future<Output = T> + 'static,
    T: 'static,
{
    Reactor::current().spawn(fut)
}
