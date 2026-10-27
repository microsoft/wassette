//! Random number generation.

/// Fill the slice with cryptographically secure random bytes.
pub fn get_random_bytes(buf: &mut [u8]) {
    if buf.is_empty() {
        return;
    }
    let output = crate::sys::rand::random_bytes(buf.len() as u64);
    buf.copy_from_slice(&output);
}

/// Fill the slice with insecure random bytes.
pub fn get_insecure_random_bytes(buf: &mut [u8]) {
    if buf.is_empty() {
        return;
    }
    let output = crate::sys::rand::insecure_random_bytes(buf.len() as u64);
    buf.copy_from_slice(&output);
}
