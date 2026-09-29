//! Async network abstractions.

use std::io::{self, ErrorKind};
use wasip3::sockets::types::ErrorCode;

mod tcp_listener;
mod tcp_stream;

pub use tcp_listener::*;
pub use tcp_stream::*;

fn to_io_err(err: ErrorCode) -> io::Error {
    match err {
        ErrorCode::AccessDenied => ErrorKind::PermissionDenied.into(),
        ErrorCode::NotSupported => ErrorKind::Unsupported.into(),
        ErrorCode::InvalidArgument => ErrorKind::InvalidInput.into(),
        ErrorCode::OutOfMemory => ErrorKind::OutOfMemory.into(),
        ErrorCode::Timeout => ErrorKind::TimedOut.into(),
        ErrorCode::InvalidState => ErrorKind::InvalidData.into(),
        ErrorCode::AddressInUse => ErrorKind::AddrInUse.into(),
        ErrorCode::ConnectionRefused => ErrorKind::ConnectionRefused.into(),
        ErrorCode::ConnectionReset => ErrorKind::ConnectionReset.into(),
        ErrorCode::ConnectionAborted => ErrorKind::ConnectionAborted.into(),
        ErrorCode::RemoteUnreachable => ErrorKind::HostUnreachable.into(),
        _ => ErrorKind::Other.into(),
    }
}
