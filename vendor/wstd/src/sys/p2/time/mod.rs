//! Monotonic and system clocks for the wasip2 backend.
//!
//! This is the platform half of the [`crate::time`] facade. The facade owns the
//! portable `Duration`/`Instant`/`Timer` types and all of their arithmetic;
//! this module provides only the primitives that genuinely depend on the WASI
//! 0.2 clocks. See [`crate::sys`] for the full backend contract.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use wasip2::clocks::{monotonic_clock, wall_clock};

use crate::runtime::{Reactor, WaitFor};

/// A measurement of the monotonic clock, in nanoseconds.
///
/// The facade's `Instant` wraps this. Keeping it a plain integer lets the
/// facade own all time arithmetic without coupling to the backend.
pub type MonotonicInstant = monotonic_clock::Instant;

/// A span of monotonic-clock time, in nanoseconds.
pub type MonotonicDuration = monotonic_clock::Duration;

/// Return the current monotonic-clock instant.
pub fn now() -> MonotonicInstant {
    monotonic_clock::now()
}

/// A measurement of the system clock, useful for talking to external entities
/// like the file system or other processes. May be converted losslessly to a
/// more useful `std::time::SystemTime` to provide more methods.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct SystemTime(wall_clock::Datetime);

impl SystemTime {
    pub fn now() -> Self {
        Self(wall_clock::now())
    }
}

impl From<SystemTime> for std::time::SystemTime {
    fn from(st: SystemTime) -> Self {
        std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(st.0.seconds)
            + std::time::Duration::from_nanos(st.0.nanoseconds.into())
    }
}

/// A future that resolves once the monotonic clock reaches a deadline.
///
/// Created by [`sleep_until`]. This is the backend `Sleep` type named by the
/// facade's `Timer`/`Wait`; on p2 it is a thin wrapper over a reactor-scheduled
/// `monotonic-clock` pollable.
#[must_use = "futures do nothing unless polled or .awaited"]
#[derive(Debug)]
pub struct Sleep {
    wait_for: WaitFor,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.wait_for).poll(cx)
    }
}

/// Create a [`Sleep`] future that resolves when the monotonic clock reaches
/// `deadline`.
///
/// Must be called from within [`crate::runtime::block_on`].
pub fn sleep_until(deadline: MonotonicInstant) -> Sleep {
    let pollable = Reactor::current().schedule(monotonic_clock::subscribe_instant(deadline));
    Sleep {
        wait_for: pollable.wait_for(),
    }
}
