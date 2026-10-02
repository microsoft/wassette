//! Types and Traits for working with asynchronous tasks.

use crate::time::{Duration, Instant, Timer, Wait};

/// Sleeps for the specified amount of time.
pub fn sleep(dur: Duration) -> Wait {
    Timer::after(dur).wait()
}

/// Sleeps until the specified instant.
pub fn sleep_until(deadline: Instant) -> Wait {
    Timer::at(deadline).wait()
}
