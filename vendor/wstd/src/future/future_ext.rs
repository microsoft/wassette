use super::{Delay, Timeout};
use std::future::{Future, IntoFuture};

/// Extend `Future` with time-based operations.
pub trait FutureExt: Future {
    /// Return an error if a future does not complete within a given time span.
    ///
    /// Typically timeouts are, as the name implies, based on _time_. However
    /// this method can time out based on any future. This can be useful in
    /// combination with channels, as it allows (long-lived) futures to be
    /// cancelled based on some external event.
    ///
    /// When a timeout is returned, the future will be dropped and destructors
    /// will be run.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use wstd::prelude::*;
    /// use wstd::time::{Instant, Duration};
    /// use std::io;
    ///
    /// #[wstd::main]
    /// async fn main() {
    ///     let res = async { "meow" }
    ///         .delay(Duration::from_millis(100))  // longer delay
    ///         .timeout(Duration::from_millis(50)) // shorter timeout
    ///         .await;
    ///     assert_eq!(res.unwrap_err().kind(), io::ErrorKind::TimedOut); // error
    ///
    ///     let res = async { "meow" }
    ///         .delay(Duration::from_millis(50))    // shorter delay
    ///         .timeout(Duration::from_millis(100)) // longer timeout
    ///         .await;
    ///     assert_eq!(res.unwrap(), "meow"); // success
    /// }
    /// ```
    fn timeout<D>(self, deadline: D) -> Timeout<Self, D::IntoFuture>
    where
        Self: Sized,
        D: IntoFuture,
    {
        Timeout::new(self, deadline.into_future())
    }

    /// Delay resolving the future until the given deadline.
    ///
    /// The underlying future will not be polled until the deadline has expired. In addition
    /// to using a time source as a deadline, any future can be used as a
    /// deadline too. When used in combination with a multi-consumer channel,
    /// this method can be used to synchronize the start of multiple futures and streams.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use wstd::prelude::*;
    /// use wstd::time::{Instant, Duration};
    ///
    /// #[wstd::main]
    /// async fn main() {
    ///     let now = Instant::now();
    ///     let delay = Duration::from_millis(100);
    ///     let _ = async { "meow" }.delay(delay).await;
    ///     assert!(now.elapsed() >= delay);
    /// }
    /// ```
    fn delay<D>(self, deadline: D) -> Delay<Self, D::IntoFuture>
    where
        Self: Sized,
        D: IntoFuture,
    {
        Delay::new(self, deadline.into_future())
    }
}

impl<T> FutureExt for T where T: Future {}
