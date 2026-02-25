/// Copy elements from `src` to `dst`, up to the length of the shorter slice.
#[inline(always)]
pub fn copy_array<T: Copy>(dst: &mut [T], src: &[T]) {
    let len = src.len().min(dst.len());
    dst[..len].copy_from_slice(&src[..len]);
}

/// Parse an ASCII byte slice representing a positive integer into a `u64`.
#[inline(always)]
pub fn parse_u64_ascii(bytes: &[u8]) -> Option<u64> {
    let mut result: u64 = 0;
    for &b in bytes {
        match b {
            b'0'..=b'9' => result = result * 10 + (b - b'0') as u64,
            _ => return None,
        }
    }
    Some(result)
}