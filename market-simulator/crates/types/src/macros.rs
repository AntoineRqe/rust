macro_rules! define_id {
    ($name:ident) => {
        define_id!($name, 20);
    };

    ($name:ident, $size:expr) => {
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
        pub struct $name(pub [u8; $size]);

        impl std::ops::Deref for $name {
            type Target = [u8; $size];
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let s = std::str::from_utf8(&self.0).unwrap_or("<invalid utf-8>");
                write!(f, "{}", s.trim_matches(char::from(0)))
            }
        }   

        impl std::ops::Add for $name {
            type Output = Self;

            fn add(self, other: Self) -> Self {
                let mut result = [0u8; $size];
                for i in 0..$size {
                    result[i] = self.0[i] ^ other.0[i]; // Simple XOR for demonstration, not a real ID generation strategy
                }
                $name(result)
            }
        }

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        impl $name {

            pub fn new() -> Self {
                $name([0u8; $size])
            }

            pub const fn from_str_const(s: &str) -> Self {
                let bytes = s.as_bytes();
                assert!(bytes.len() <= $size, "id string must fit in configured id size");
                let mut arr = [0u8; $size];
                let mut i = 0;
                while i < bytes.len() {
                    arr[i] = bytes[i];
                    i += 1;
                }
                $name(arr)
            }

            pub const fn from_ascii(s: &str) -> Self {
                let mut bytes = [0u8; $size];
                let s_bytes = s.as_bytes();
                let mut i = 0;
                while i < s_bytes.len() && i < $size {
                    bytes[i] = s_bytes[i];
                    i += 1;
                }
                $name(bytes)
            }

            pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
                if bytes.len() != $size {
                    return None;
                }
                let mut arr = [0u8; $size];
                arr.copy_from_slice(bytes);
                Some($name(arr))
            }
    
            pub fn to_numeric(&self) -> u64 {
                let mut num = 0u64;
                let width = std::cmp::min($size, 8);
                for i in 0..width {
                    num <<= 8;
                    num |= self.0[i] as u64;
                }
                num
            }

            pub fn increment(&mut self) {
                for i in (0..self.0.len()).rev() {
                    if self.0[i] < 255 {
                        self.0[i] += 1;
                        break;
                    } else {
                        self.0[i] = 0; // Reset to zero and carry over to the next byte
                    }
                }
            }
        }
    };
}

// Usage:
define_id!(EntityId);
define_id!(OrderId);
define_id!(ClientId);
define_id!(FixedString);
define_id!(SymbolId, 4);