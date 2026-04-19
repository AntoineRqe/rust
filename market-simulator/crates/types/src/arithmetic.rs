use std::iter::Sum;

/// Price represented as integer with implicit 8 decimal places
/// e.g. 123.45678900 -> 12_345_678_900
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FixedPointArithmetic(pub i64);


impl FixedPointArithmetic {
    pub const ZERO: FixedPointArithmetic = FixedPointArithmetic(0);
    pub const SCALE: i64 = 100_000_000; // 10^8

    pub fn from_fix_bytes(bytes: &[u8]) -> Option<Self> {
        // parse "123.45678900" without any float conversion
        let mut integer_part: i64 = 0;
        let mut frac_part: i64 = 0;
        let mut frac_digits: i64 = 0;
        let mut in_frac = false;
        let mut negative = false;
        let mut i = 0;

        if bytes.first() == Some(&b'-') {
            negative = true;
            i += 1;
        }

        while i < bytes.len() {
            match bytes[i] {
                b'0'..=b'9' => {
                    let d = (bytes[i] - b'0') as i64;
                    if in_frac {
                        if frac_digits < 8 {
                            frac_part = frac_part * 10 + d;
                            frac_digits += 1;
                        }
                        // ignore extra decimal places
                    } else {
                        integer_part = integer_part * 10 + d;
                    }
                }
                b'.' => in_frac = true,
                0 => break, // null terminator for fixed-size byte arrays
                _ => return None,
            }
            i += 1;
        }

        // pad fractional part to 8 digits
        // e.g. "123.45" -> frac_part=45, frac_digits=2 -> pad by 10^6
        let scale = 10_i64.pow((8 - frac_digits) as u32);
        let raw = integer_part * Self::SCALE + frac_part * scale;

        Some(FixedPointArithmetic(if negative { -raw } else { raw }))
    }

    pub fn to_fix_bytes(self) -> [u8; 20] {
        let mut buf = [0u8; 20];
        let raw = self.0;
        let negative = raw < 0;
        let abs_raw = raw.abs();
        let integer_part = abs_raw / Self::SCALE;
        let frac_part = abs_raw % Self::SCALE;

        let mut i = 0;
        if negative {
            buf[i] = b'-';
            i += 1;
        }

        // Write integer part
        let int_str = integer_part.to_string();
        for b in int_str.as_bytes() {
            buf[i] = *b;
            i += 1;
        }

        buf[i] = b'.';
        i += 1;

        // Write fractional part, zero-padded to 8 digits
        let frac_str = format!("{:08}", frac_part);
        for b in frac_str.as_bytes() {
            buf[i] = *b;
            i += 1;
        }

        buf
    }

    pub fn to_f64(self) -> f64 {
        self.0 as f64 / Self::SCALE as f64
    }

    pub fn from_f64(number: f64) -> Self {
        FixedPointArithmetic((number * Self::SCALE as f64).round() as i64)
    }
    
    pub fn from_option_f64(value: Option<f64>) -> FixedPointArithmetic {
        value
            .map(FixedPointArithmetic::from_f64)
            .unwrap_or(FixedPointArithmetic::ZERO)
    }

    pub fn from_raw(raw: i64) -> Self {
        FixedPointArithmetic(raw)
    }

    pub fn raw(self) -> i64 {
        self.0
    }

    pub fn from_number<T: Into<i64>>(num: T) -> Self {
        FixedPointArithmetic::from_raw(num.into() * Self::SCALE)
    }
}

impl Sum for FixedPointArithmetic {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let mut total: i64 = 0;
        for price in iter {
            total += price.0;
        }
        FixedPointArithmetic::from_raw(total)
    }
}

impl std::ops::Add for FixedPointArithmetic {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        FixedPointArithmetic::from_raw(self.0 + other.0)
    }
}

impl std::ops::Sub for FixedPointArithmetic {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        FixedPointArithmetic::from_raw(self.0 - other.0)
    }
}

impl std::ops::Mul for FixedPointArithmetic {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        // (a * b) / SCALE to maintain the fixed-point representation
        FixedPointArithmetic::from_raw(((self.0 as i128 * other.0 as i128) / Self::SCALE as i128) as i64)
    }
}

impl std::ops::Div for FixedPointArithmetic {
    type Output = Self;

    fn div(self, other: Self) -> Self {
        // (a * SCALE) / b to maintain the fixed-point representation
        FixedPointArithmetic::from_raw(((self.0 as i128 * Self::SCALE as i128) / other.0 as i128) as i64)
    }
}

impl std::ops::AddAssign for FixedPointArithmetic {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl std::ops::SubAssign for FixedPointArithmetic {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl std::fmt::Display for FixedPointArithmetic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let raw = self.0;
        let integer_part = raw / Self::SCALE;
        let frac_part = (raw.abs() % Self::SCALE) as u64;
        write!(f, "{}.{:08}", integer_part, frac_part)
    }
}