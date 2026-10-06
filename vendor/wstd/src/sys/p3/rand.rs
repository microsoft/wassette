//! Random number generation.

use wasip3::random;

/// Return `len` cryptographically secure random bytes.
pub fn random_bytes(len: u64) -> Vec<u8> {
    random::random::get_random_bytes(len)
}

/// Return `len` insecure, non-cryptographic random bytes.
pub fn insecure_random_bytes(len: u64) -> Vec<u8> {
    random::insecure::get_insecure_random_bytes(len)
}
