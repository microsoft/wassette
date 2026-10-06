//! Async IO abstractions.

mod copy;
mod cursor;
mod empty;
mod read;
mod seek;
mod write;

#[cfg(wstd_p2)]
pub use crate::runtime::AsyncPollable;
pub use crate::sys::io::*;
pub use copy::*;
pub use cursor::*;
pub use empty::*;
pub use read::*;
pub use seek::*;
pub use write::*;

/// The error type for I/O operations.
///
pub use std::io::Error;

/// A specialized Result type for I/O operations.
///
pub use std::io::Result;
