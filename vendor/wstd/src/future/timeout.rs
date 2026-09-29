use crate::time::utils::timeout_err;

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;

pin_project! {
    /// A future that times out after a duration of time.
    ///
    /// This `struct` is created by the [`timeout`] method on [`FutureExt`]. See its
    /// documentation for more.
    ///
    /// [`timeout`]: crate::future::FutureExt::timeout
    /// [`FutureExt`]: crate::future::futureExt
    #[must_use = "futures do nothing unless polled or .awaited"]
    pub struct Timeout<F, D> {
        #[pin]
        future: F,
        #[pin]
        deadline: D,
        completed: bool,
    }
}

impl<F, D> Timeout<F, D> {
    pub(super) fn new(future: F, deadline: D) -> Self {
        Self {
            future,
            deadline,
            completed: false,
        }
    }
}

impl<F: Future, D: Future> Future for Timeout<F, D> {
    type Output = io::Result<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        assert!(!*this.completed, "future polled after completing");

        match this.future.poll(cx) {
            Poll::Ready(v) => {
                *this.completed = true;
                Poll::Ready(Ok(v))
            }
            Poll::Pending => match this.deadline.poll(cx) {
                Poll::Ready(_) => {
                    *this.completed = true;
                    Poll::Ready(Err(timeout_err("future timed out")))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
