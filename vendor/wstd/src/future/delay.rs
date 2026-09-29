use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use pin_project_lite::pin_project;

pin_project! {
    /// Suspends a future until the specified deadline.
    ///
    /// This `struct` is created by the [`delay`] method on [`FutureExt`]. See its
    /// documentation for more.
    ///
    /// [`delay`]: crate::future::FutureExt::delay
    /// [`FutureExt`]: crate::future::futureExt
    #[must_use = "futures do nothing unless polled or .awaited"]
    pub struct Delay<F, D> {
        #[pin]
        future: F,
        #[pin]
        deadline: D,
        state: State,
    }
}

/// The internal state
#[derive(Debug)]
enum State {
    Started,
    PollFuture,
    Completed,
}

impl<F, D> Delay<F, D> {
    pub(super) fn new(future: F, deadline: D) -> Self {
        Self {
            future,
            deadline,
            state: State::Started,
        }
    }
}

impl<F: Future, D: Future> Future for Delay<F, D> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            match this.state {
                State::Started => {
                    ready!(this.deadline.as_mut().poll(cx));
                    *this.state = State::PollFuture;
                }
                State::PollFuture => {
                    let value = ready!(this.future.as_mut().poll(cx));
                    *this.state = State::Completed;
                    return Poll::Ready(value);
                }
                State::Completed => panic!("future polled after completing"),
            }
        }
    }
}
