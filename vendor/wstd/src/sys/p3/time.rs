use std::future::Future;
use std::pin::Pin;

use wasip3::clocks::{monotonic_clock, system_clock};

/// A measurement of a monotonically nondecreasing clock. Opaque and useful only
/// with Duration.
pub type MonotonicInstant = monotonic_clock::Mark;

/// A duration from the monotonic clock, in nanoseconds.
pub type MonotonicDuration = monotonic_clock::Duration;

/// Return the current monotonic clock instant.
pub fn now() -> MonotonicInstant {
    monotonic_clock::now()
}

/// A measurement of the system clock, useful for talking to external entities
/// like the file system or other processes. May be converted losslessly to a
/// more useful `std::time::SystemTime` to provide more methods.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct SystemTime(system_clock::Instant);

impl SystemTime {
    pub fn now() -> Self {
        Self(system_clock::now())
    }
}

impl From<SystemTime> for std::time::SystemTime {
    fn from(st: SystemTime) -> Self {
        // p3 system_clock::Instant has i64 seconds
        if st.0.seconds >= 0 {
            std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(st.0.seconds as u64)
                + std::time::Duration::from_nanos(st.0.nanoseconds.into())
        } else {
            std::time::SystemTime::UNIX_EPOCH
                - std::time::Duration::from_secs((-st.0.seconds) as u64)
                + std::time::Duration::from_nanos(st.0.nanoseconds.into())
        }
    }
}

/// A future that resolves once the monotonic clock reaches a deadline.
pub type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Create a [`Sleep`] future that resolves when the monotonic clock reaches
/// `deadline`.
pub fn sleep_until(deadline: MonotonicInstant) -> Sleep {
    Box::pin(monotonic_clock::wait_until(deadline))
}
