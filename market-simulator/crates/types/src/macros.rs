macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Hash)]
        pub struct $name(pub [u8; 20]);

        impl std::ops::Deref for $name {
            type Target = [u8; 20];
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
                let mut result = [0u8; 20];
                for i in 0..20 {
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
                $name([0u8; 20])
            }

            pub const fn from_str_const(s: &str) -> Self {
                let bytes = s.as_bytes();
                assert!(bytes.len() <= 20, "id string must be <= 20 bytes");
                let mut arr = [0u8; 20];
                let mut i = 0;
                while i < bytes.len() {
                    arr[i] = bytes[i];
                    i += 1;
                }
                $name(arr)
            }

            pub const fn from_ascii(s: &str) -> Self {
                let mut bytes = [0u8; 20];
                let s_bytes = s.as_bytes();
                let mut i = 0;
                while i < s_bytes.len() && i < 20 {
                    bytes[i] = s_bytes[i];
                    i += 1;
                }
                $name(bytes)
            }

            pub fn to_numeric(&self) -> u64 {
                let mut num = 0u64;
                for i in 0..8 {
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
define_id!(TradeId);
define_id!(FixedString);