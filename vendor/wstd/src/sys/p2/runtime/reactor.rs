use super::REACTOR;

use async_task::{Runnable, Task};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use slab::Slab;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use wasip2::io::poll::Pollable;

/// A key for a `Pollable`, which is an index into the `Slab<Pollable>` in `Reactor`.
#[repr(transparent)]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub(crate) struct EventKey(pub(crate) usize);

/// A Registration is a reference to the Reactor's owned Pollable. When the registration is
/// dropped, the reactor will drop the Pollable resource.
#[derive(Debug, PartialEq, Eq, Hash)]
struct Registration {
    key: EventKey,
}

impl Drop for Registration {
    fn drop(&mut self) {
        Reactor::current().deregister_event(self.key)
    }
}

/// An AsyncPollable is a reference counted Registration. It can be cloned, and used to create
/// as many WaitFor futures on a Pollable that the user needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AsyncPollable(Arc<Registration>);

impl AsyncPollable {
    /// Create an `AsyncPollable` from a Wasi `Pollable`. Schedules the `Pollable` with the current
    /// `Reactor`.
    pub fn new(pollable: Pollable) -> Self {
        Reactor::current().schedule(pollable)
    }
    /// Create a Future that waits for the Pollable's readiness.
    pub fn wait_for(&self) -> WaitFor {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        WaitFor {
            waitee: Waitee {
                pollable: self.clone(),
                unique,
            },
            needs_deregistration: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct Waitee {
    /// This needs to be a reference counted registration, because it may outlive the AsyncPollable
    /// &self that it was created from.
    pollable: AsyncPollable,
    unique: u64,
}

/// A Future that waits for the Pollable's readiness.
#[must_use = "futures do nothing unless polled or .awaited"]
#[derive(Debug)]
pub struct WaitFor {
    waitee: Waitee,
    needs_deregistration: bool,
}
impl Future for WaitFor {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let reactor = Reactor::current();
        if reactor.ready(&self.as_ref().waitee, cx.waker()) {
            Poll::Ready(())
        } else {
            self.as_mut().needs_deregistration = true;
            Poll::Pending
        }
    }
}
impl Drop for WaitFor {
    fn drop(&mut self) {
        if self.needs_deregistration {
            Reactor::current().deregister_waitee(&self.waitee)
        }
    }
}

/// Manage async system resources for WASI 0.2
#[derive(Debug, Clone)]
pub struct Reactor {
    inner: Arc<InnerReactor>,
}

/// The private, internal `Reactor` implementation - factored out so we can take
/// a lock of the whole.
#[derive(Debug)]
struct InnerReactor {
    pollables: Mutex<Slab<Pollable>>,
    wakers: Mutex<HashMap<Waitee, Waker>>,
    ready_list: Mutex<VecDeque<Runnable>>,
}

impl Reactor {
    /// Return a `Reactor` for the currently running `wstd::runtime::block_on`.
    ///
    /// # Panic
    /// This will panic if called outside of `wstd::runtime::block_on`.
    pub fn current() -> Self {
        REACTOR.with(|r| {
            r.borrow()
                .as_ref()
                .expect("Reactor::current must be called within a wstd runtime")
                .clone()
        })
    }

    /// Create a new instance of `Reactor`
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(InnerReactor {
                pollables: Mutex::new(Slab::new()),
                wakers: Mutex::new(HashMap::new()),
                ready_list: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// The reactor tracks the set of WASI pollables which have an associated
    /// Future pending on their readiness. This function returns indicating
    /// that set of pollables is not empty.
    pub(crate) fn pending_pollables_is_empty(&self) -> bool {
        self.inner.wakers.lock().unwrap().is_empty()
    }

    /// Block until at least one pending pollable is ready, waking a pending future.
    /// Precondition: self.nonempty_pending_pollables() is true.
    pub(crate) fn block_on_pollables(&self) {
        self.check_pollables(|targets| {
            debug_assert_ne!(
                targets.len(),
                0,
                "Attempting to block on an empty list of pollables - without any pending work, no progress can be made and wasip2::io::poll::poll will trap"
            );
            wasip2::io::poll::poll(targets)

        })
    }

    /// Without blocking, check for any ready pollables and wake the
    /// associated futures.
    pub(crate) fn nonblock_check_pollables(&self) {
        // If there are no pollables with associated pending futures, there is
        // no work to do here, so return immediately.
        if self.pending_pollables_is_empty() {
            return;
        }
        // Lazily create a pollable which always resolves to ready.
        use std::sync::LazyLock;
        static READY_POLLABLE: LazyLock<Pollable> =
            LazyLock::new(|| wasip2::clocks::monotonic_clock::subscribe_duration(0));

        self.check_pollables(|targets| {
            // Create a new set of targets, with the addition of the ready
            // pollable:
            let ready_index = targets.len();
            let mut new_targets = Vec::with_capacity(ready_index + 1);
            new_targets.extend_from_slice(targets);
            new_targets.push(&*READY_POLLABLE);

            // Poll is now guaranteed to return immediately, because at least
            // one member is ready:
            let mut ready_list = wasip2::io::poll::poll(&new_targets);

            // Erase our extra ready pollable from the ready list:
            ready_list.retain(|e| *e != ready_index as u32);
            ready_list
        })
    }

    /// Common core of blocking and nonblocking pollable checks. Wakes any
    /// futures which are pending on the pollables, according to the result of
    /// the check_ready function.
    /// Precondition: self.nonempty_pending_pollables() is true.
    fn check_pollables<F>(&self, check_ready: F)
    where
        F: FnOnce(&[&Pollable]) -> Vec<u32>,
    {
        let wakers = self.inner.wakers.lock().unwrap();
        let pollables = self.inner.pollables.lock().unwrap();

        // We're about to wait for a number of pollables. When they wake we get
        // the *indexes* back for the pollables whose events were available - so
        // we need to be able to associate the index with the right waker.

        // We start by iterating over the pollables, and keeping note of which
        // pollable belongs to which waker
        let mut indexed_wakers = Vec::with_capacity(wakers.len());
        let mut targets = Vec::with_capacity(wakers.len());
        for (waitee, waker) in wakers.iter() {
            let pollable_index = waitee.pollable.0.key;
            indexed_wakers.push(waker);
            targets.push(&pollables[pollable_index.0]);
        }

        // Now that we have that association, we're ready to check our targets for readiness.
        // (This is either a wasi poll, or the nonblocking variant.)
        let ready_indexes = check_ready(&targets);

        // Once we have the indexes for which pollables are available, we need
        // to convert it back to the right keys for the wakers. Earlier we
        // established a positional index -> waker key relationship, so we can
        // go right ahead and perform a lookup there.
        let ready_wakers = ready_indexes
            .into_iter()
            .map(|index| indexed_wakers[index as usize]);

        for waker in ready_wakers {
            waker.wake_by_ref()
        }
    }

    /// Turn a Wasi [`Pollable`] into an [`AsyncPollable`]
    pub fn schedule(&self, pollable: Pollable) -> AsyncPollable {
        let mut pollables = self.inner.pollables.lock().unwrap();
        let key = EventKey(pollables.insert(pollable));
        AsyncPollable(Arc::new(Registration { key }))
    }

    fn deregister_event(&self, key: EventKey) {
        let mut pollables = self.inner.pollables.lock().unwrap();
        pollables.remove(key.0);
    }

    fn deregister_waitee(&self, waitee: &Waitee) {
        let mut wakers = self.inner.wakers.lock().unwrap();
        wakers.remove(waitee);
    }

    fn ready(&self, waitee: &Waitee, waker: &Waker) -> bool {
        let ready = self
            .inner
            .pollables
            .lock()
            .unwrap()
            .get(waitee.pollable.0.key.0)
            .expect("only live EventKey can be checked for readiness")
            .ready();
        if !ready {
            self.inner
                .wakers
                .lock()
                .unwrap()
                .insert(waitee.clone(), waker.clone());
        }
        ready
    }

    /// We need an unchecked spawn for implementing `block_on`, where the
    /// implementation does not expose the resulting Task and does not return
    /// until all tasks have finished execution.
    ///
    /// # Safety
    /// Caller must ensure that the Task<T> does not outlive the Future F or
    /// F::Output T.
    #[allow(unsafe_code)]
    pub(crate) unsafe fn spawn_unchecked<F, T>(&self, fut: F) -> Task<T>
    where
        F: Future<Output = T>,
    {
        let this = self.clone();
        let schedule = move |runnable| this.inner.ready_list.lock().unwrap().push_back(runnable);

        // SAFETY:
        // we're using this exactly like async_task::spawn_local, except that
        // the schedule function is not Send or Sync, because Runnable is not
        // Send or Sync. This is safe because wasm32-wasip2 is always
        // single-threaded.
        #[allow(unsafe_code)]
        let (runnable, task) = unsafe { async_task::spawn_unchecked(fut, schedule) };
        self.inner.ready_list.lock().unwrap().push_back(runnable);
        task
    }

    /// Spawn a `Task` on the `Reactor`.
    pub fn spawn<F, T>(&self, fut: F) -> Task<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        // Safety: 'static constraints satisfy the lifetime requirements
        #[allow(unsafe_code)]
        unsafe {
            self.spawn_unchecked(fut)
        }
    }

    pub(super) fn pop_ready_list(&self) -> Option<Runnable> {
        self.inner.ready_list.lock().unwrap().pop_front()
    }

    pub(super) fn ready_list_is_empty(&self) -> bool {
        self.inner.ready_list.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    // Using WASMTIME_LOG, observe that this test doesn't even call poll() - the pollable is ready
    // immediately.
    #[test]
    fn subscribe_no_duration() {
        crate::runtime::block_on(async {
            let reactor = Reactor::current();
            let pollable = wasip2::clocks::monotonic_clock::subscribe_duration(0);
            let sched = reactor.schedule(pollable);
            sched.wait_for().await;
        })
    }
    // Using WASMTIME_LOG, observe that this test calls poll() until the timer is ready.
    #[test]
    fn subscribe_some_duration() {
        crate::runtime::block_on(async {
            let reactor = Reactor::current();
            let pollable = wasip2::clocks::monotonic_clock::subscribe_duration(10_000_000);
            let sched = reactor.schedule(pollable);
            sched.wait_for().await;
        })
    }

    // Using WASMTIME_LOG, observe that this test results in a single poll() on the second
    // subscription, rather than spinning in poll() with first subscription, which is instantly
    // ready, but not what the waker requests.
    #[test]
    fn subscribe_multiple_durations() {
        crate::runtime::block_on(async {
            let reactor = Reactor::current();
            let now = wasip2::clocks::monotonic_clock::subscribe_duration(0);
            let soon = wasip2::clocks::monotonic_clock::subscribe_duration(10_000_000);
            let now = reactor.schedule(now);
            let soon = reactor.schedule(soon);
            soon.wait_for().await;
            drop(now)
        })
    }

    // Using WASMTIME_LOG, observe that this test results in two calls to poll(), one with both
    // pollables because both are awaiting, and one with just the later pollable.
    #[test]
    fn subscribe_multiple_durations_zipped() {
        crate::runtime::block_on(async {
            let reactor = Reactor::current();
            let start = wasip2::clocks::monotonic_clock::now();
            let soon = wasip2::clocks::monotonic_clock::subscribe_duration(10_000_000);
            let later = wasip2::clocks::monotonic_clock::subscribe_duration(40_000_000);
            let soon = reactor.schedule(soon);
            let later = reactor.schedule(later);

            futures_lite::future::zip(
                async move {
                    soon.wait_for().await;
                    println!(
                        "*** subscribe_duration(soon) ready ({})",
                        wasip2::clocks::monotonic_clock::now() - start
                    );
                },
                async move {
                    later.wait_for().await;
                    println!(
                        "*** subscribe_duration(later) ready ({})",
                        wasip2::clocks::monotonic_clock::now() - start
                    );
                },
            )
            .await;
        })
    }

    #[test]
    fn progresses_wasi_independent_futures() {
        crate::runtime::block_on(async {
            let start = wasip2::clocks::monotonic_clock::now();

            let reactor = Reactor::current();
            const LONG_DURATION: u64 = 1_000_000_000;
            let later = wasip2::clocks::monotonic_clock::subscribe_duration(LONG_DURATION);
            let later = reactor.schedule(later);
            let mut polled_before = false;
            // This is basically futures_lite::future::yield_now, except with a boolean
            // `polled_before` so we can definitively observe what happened
            let wasi_independent_future = futures_lite::future::poll_fn(|cx| {
                if polled_before {
                    std::task::Poll::Ready(true)
                } else {
                    polled_before = true;
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            });
            let later = async {
                later.wait_for().await;
                false
            };
            let wasi_independent_future_won =
                futures_lite::future::race(wasi_independent_future, later).await;
            assert!(
                wasi_independent_future_won,
                "wasi_independent_future should win the race"
            );
            const SHORT_DURATION: u64 = LONG_DURATION / 100;
            let soon = wasip2::clocks::monotonic_clock::subscribe_duration(SHORT_DURATION);
            let soon = reactor.schedule(soon);
            soon.wait_for().await;

            let end = wasip2::clocks::monotonic_clock::now();

            let duration = end - start;
            assert!(
                duration > SHORT_DURATION,
                "{duration} greater than short duration shows awaited for `soon` properly"
            );
            // Upper bound is high enough that even the very poor windows CI machines meet it
            assert!(
                duration < (5 * SHORT_DURATION),
                "{duration} less than a reasonable multiple of short duration {SHORT_DURATION} shows did not await for `later`"
            );
        })
    }

    #[test]
    fn cooperative_concurrency() {
        crate::runtime::block_on(async {
            let cpu_heavy = async move {
                // Simulating a CPU-heavy task that runs for 1 second and yields occasionally
                for _ in 0..10 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    futures_lite::future::yield_now().await;
                }
                true
            };
            let timeout = async move {
                crate::time::Timer::after(crate::time::Duration::from_millis(200))
                    .wait()
                    .await;
                false
            };
            let mut future_group = futures_concurrency::future::FutureGroup::<
                Pin<Box<dyn std::future::Future<Output = bool>>>,
            >::new();
            future_group.insert(Box::pin(cpu_heavy));
            future_group.insert(Box::pin(timeout));
            let result = futures_lite::StreamExt::next(&mut future_group).await;
            assert_eq!(result, Some(false), "cpu_heavy task should have timed out");
        });
    }
}
