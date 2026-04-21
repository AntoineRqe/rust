/// Parse an ASCII byte slice representing a positive integer into a `u64`.
#[inline(always)]
pub fn bytes_to_number<T: From<u8> + std::ops::Add<Output = T> + std::ops::Mul<Output = T>>(bytes: &[u8]) -> Option<T> {
    let mut result: T = 0.into();
    for &b in bytes {
        match b {
            b'0'..=b'9' => result = result * 10.into() + (b - b'0').into(),
            _ => return None,
        }
    }
    Some(result)
}

/// Copy elements from `src` to `dst`, up to the length of the shorter slice.
#[inline(always)]
pub fn copy_array<T: Copy>(dst: &mut [T], src: &[T]) {
    let len = src.len().min(dst.len());
    dst[..len].copy_from_slice(&src[..len]);
}

///Remove trailing zeros from a fixed-size byte array and return a subslice containing only the valid data.
#[inline(always)]
pub fn field_str(f: &[u8]) -> &[u8] {
    let len = f.iter().position(|&b| b == 0).unwrap_or(f.len());
    &f[..len]
}

// ---------------------------------------
// ---- Number to bytes conversion ----
// ---------------------------------------
pub struct NumBytes {
    buf: [u8; 20],
    len: usize,
}

impl NumBytes {
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn from_number<T: Into<u64>>(num: T) -> Self {
        number_to_bytes(num)
    }
}

impl std::ops::Deref for NumBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] { self.as_bytes() }
}

/// Parse an integer into array of ASCII bytes. Returns the number of bytes written.
#[inline(always)]
pub fn number_to_bytes<T>(number: T) -> NumBytes
where T: Into<u64> {
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut number = number.into();

    if number == 0 {
        buf[0] = b'0';
        return NumBytes { buf, len: 1 };
    }

    while number > 0 {
        buf[i] = b'0' + (number % 10) as u8;
        number /= 10;
        i += 1;
    }

    // Reverse the buffer to get the correct order of digits
    // e.g. if number is 123, we write '3', then '2', then '1' into the buffer, so we need to reverse it to get "123".  
    buf[..i].reverse();
    NumBytes { buf, len: i }
}