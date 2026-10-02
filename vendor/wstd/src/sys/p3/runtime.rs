use async_task::{Runnable, Task as AsyncTask};
use core::future::Future;
use std::cell::RefCell;

// There are no threads in WASI, so this is just a safe way to thread a single reactor to all
// use sites in the background.
std::thread_local! {
    pub(crate) static REACTOR: RefCell<Option<Reactor>> = const { RefCell::new(None) };
}

/// Start the event loop. Blocks until the future completes.
///
/// Delegates to wit-bindgen's block_on which integrates with the component
/// model's async runtime (waitable-set polling) for native p3 async support.
pub fn block_on<F>(fut: F) -> F::Output
where
    F: Future + 'static,
    F::Output: 'static,
{
    // Guard against nested `block_on`. Spawn support itself is provided by
    // wit-bindgen's executor, not this reactor, so the stored value is only
    // used to detect re-entrancy.
    let reactor = Reactor::new();
    let prev = REACTOR.replace(Some(reactor));
    if prev.is_some() {
        panic!("cannot wstd::runtime::block_on inside an existing block_on!")
    }

    let result = wasip3::wit_bindgen::rt::async_support::block_on(fut);

    REACTOR.replace(None);
    result
}

/// Helper used by the `#[wstd::main]` expansion to map an async `main`'s output
/// into the `Result<(), ()>` that the `wasi:cli/run` export requires.
///
/// On `Err`, the error is reported to stderr before returning a failure so the
/// component exits with a non-zero status.
#[doc(hidden)]
pub fn __finish_main<T: __MainReturn>(value: T) -> Result<(), ()> {
    value.into_main_result()
}

/// Conversion from an async `main`'s return type to the `wasi:cli/run` result.
///
/// Implemented for `()` and `Result<(), E: Debug>`, matching the return types
/// permitted on `async fn main`.
#[doc(hidden)]
pub trait __MainReturn {
    /// Convert into the `Result<(), ()>` expected by `wasi:cli/run`.
    fn into_main_result(self) -> Result<(), ()>;
}

impl __MainReturn for () {
    fn into_main_result(self) -> Result<(), ()> {
        Ok(())
    }
}

impl<E: core::fmt::Debug> __MainReturn for Result<(), E> {
    fn into_main_result(self) -> Result<(), ()> {
        match self {
            Ok(()) => Ok(()),
            Err(err) => {
                eprintln!("Error: {err:?}");
                Err(())
            }
        }
    }
}

/// Marker for the currently running `wstd` async runtime.
///
/// On p3, task scheduling is handled by wit-bindgen's component-model async
/// executor, so the reactor carries no state of its own. It remains a distinct
/// type so the target-agnostic facade in `src/runtime.rs` (shared with p2) can
/// call `Reactor::current().spawn(..)` uniformly across backends.
#[derive(Debug, Clone)]
pub struct Reactor {
    _private: (),
}

impl Reactor {
    /// Return a `Reactor` for the currently running `wstd` async runtime.
    ///
    /// On p3, task scheduling is delegated to wit-bindgen's component-model
    /// async executor, which is live throughout any async component task —
    /// both [`block_on`] and the async-lifted `#[wstd::main]` export. The
    /// reactor holds no state, so this always succeeds.
    pub fn current() -> Self {
        Self::new()
    }

    /// Create a new instance of `Reactor`
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Spawn a `Task` on the `Reactor`.
    ///
    /// Each runnable is driven on wit-bindgen's component-model async executor
    /// (the same one [`block_on`] runs on), so the spawned future is polled to
    /// completion concurrently with the task that spawned it. Every time the
    /// task is woken, `schedule` runs again and re-submits the runnable.
    pub fn spawn<F, T>(&self, fut: F) -> AsyncTask<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        let schedule = move |runnable: Runnable| {
            wasip3::wit_bindgen::rt::async_support::spawn_local(async move {
                runnable.run();
            });
        };

        // Safety: 'static constraints satisfy the lifetime requirements
        #[allow(unsafe_code)]
        let (runnable, task) = unsafe { async_task::spawn_unchecked(fut, schedule) };
        runnable.schedule();
        task
    }
}
