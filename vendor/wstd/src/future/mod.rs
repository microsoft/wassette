//! Asynchronous values.
//!
//! # Cancellation
//!
//! Futures can be cancelled by dropping them before they finish executing. This
//! is useful when we're no longer interested in the result of an operation, as
//! it allows us to stop doing needless work. This also means that a future may cancel at any `.await` point, and so just
//! like with `?` we have to be careful to roll back local state if our future
//! halts there.
//!
//!
//! ```no_run
//! use futures_lite::prelude::*;
//! use wstd::prelude::*;
//! use wstd::time::Duration;
//!
//! #[wstd::main]
//! async fn main() {
//!     let mut counter = 0;
//!     let value = async { "meow" }
//!         .delay(Duration::from_millis(100))
//!         .timeout(Duration::from_millis(200))
//!         .await;
//!
//!     assert_eq!(value.unwrap(), "meow");
//! }
//! ```

mod delay;
mod future_ext;
mod timeout;

pub use delay::Delay;
pub use future_ext::FutureExt;
pub use timeout::Timeout;
