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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct UtcTimestamp {
    pub year:   u16,
    pub month:  u8,
    pub day:    u8,
    pub hour:   u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
}

impl UtcTimestamp {
    /// Parses "20240219-12:30:00.000"
    pub fn from_fix_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 17 {
            return None;
        }

        // "20240219-12:30:00.000"
        //  0123456789012345678901
        let year   = parse_unsigned_ascii::<u16>(&b[0..4])?;
        let month  = parse_unsigned_ascii::<u8>(&b[4..6])?;
        let day    = parse_unsigned_ascii::<u8>(&b[6..8])?;
        // b[8] == b'-'
        let hour   = parse_unsigned_ascii::<u8>(&b[9..11])?;
        // b[11] == b':'
        let minute = parse_unsigned_ascii::<u8>(&b[12..14])?;
        // b[14] == b':'
        let second = parse_unsigned_ascii::<u8>(&b[15..17])?;

        let millis = if b.len() >= 21 && b[17] == b'.' {
            parse_unsigned_ascii::<u16>(&b[18..21])?
        } else {
            0
        };

        Some(Self { year, month, day, hour, minute, second, millis })
    }

    /// Convert to unix timestamp in milliseconds
    pub fn to_unix_ms(&self) -> i64 {
        // days since unix epoch (1970-01-01)
        let days = days_since_epoch(self.year, self.month, self.day) as i64;
        let ms = days        * 86_400_000
               + self.hour   as i64 * 3_600_000
               + self.minute as i64 *    60_000
               + self.second as i64 *     1_000
               + self.millis as i64;
        ms
    }

    pub fn from_unix_ms(ms: i64) -> Self {
        let days = ms / 86_400_000;
        let time_ms = ms % 86_400_000;

        // Convert days back to year/month/day using the inverse of the days_since_epoch calculation
        let z = days + 719_468;
        let era = z / 146_097;
        let doe = z % 146_097;
        let yoe = (doe - doe/4 + doe/100) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365*yoe + yoe/4 - yoe/100);
        let m = (5*doy + 2)/153;
        let d = doy - (153*m+2)/5 + 1;
        let m = m + if m < 10 { 3 } else { -9 };
        let y = y + if m <= 2 { 1 } else { 0 };

        let hour   = (time_ms / 3_600_000) as u8;
        let minute = ((time_ms % 3_600_000) / 60_000) as u8;
        let second = ((time_ms % 60_000) / 1_000) as u8;
        let millis = (time_ms % 1_000) as u16;

        Self {
            year: y as u16,
            month: m as u8,
            day: d as u8,
            hour,
            minute,
            second,
            millis,
        }
    }
}

/// Parse an ASCII byte slice representing a positive integer into a `u64`.
#[inline(always)]
pub fn parse_unsigned_ascii<T: From<u8> + std::ops::Add<Output = T> + std::ops::Mul<Output = T>>(bytes: &[u8]) -> Option<T> {
    let mut result: T = 0.into();
    for &b in bytes {
        match b {
            b'0'..=b'9' => result = result * 10.into() + (b - b'0').into(),
            _ => return None,
        }
    }
    Some(result)
}

/// Proleptic Gregorian calendar days since 1970-01-01
fn days_since_epoch(year: u16, month: u8, day: u8) -> u32 {
    let y = year as u32;
    let m = month as u32;
    let d = day as u32;

    // adjust months so March = 1, to simplify leap year handling
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };

    let era    = y / 400;
    let yoe    = y % 400;                                  // year of era [0, 399]
    let doy    = (153 * m + 2) / 5 + d - 1;               // day of year [0, 365]
    let doe    = yoe * 365 + yoe / 4 - yoe / 100 + doy;   // day of era  [0, 146096]

    era * 146_097 + doe - 719_468                          // days since epoch
}