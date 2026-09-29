//! Random number generation primitives for the wasip2 backend.
//!
//! This is the platform half of the [`crate::rand`] facade: it only reaches the
//! host RNG. The length guard and slice copy live in the facade.

use wasip2::random;

/// Return `len` cryptographically secure random bytes.
pub fn random_bytes(len: u64) -> Vec<u8> {
    random::random::get_random_bytes(len)
}

/// Return `len` insecure, non-cryptographic random bytes.
pub fn insecure_random_bytes(len: u64) -> Vec<u8> {
    random::insecure::get_insecure_random_bytes(len)
}
