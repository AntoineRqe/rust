use crate::functions::{bytes_to_number, number_to_bytes, copy_array};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct UtcTimestamp {
    pub year:   u16,
    pub month:  u8,
    pub day:    u8,
    pub hour:   u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
    pub micros: Option<u16>,
    pub nanos: Option<u16>,
}

impl UtcTimestamp {
    /// Parses "20240219-12:30:00.000"
    pub fn from_fix_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 17 {
            return None;
        }

        // "20240219-12:30:00.000"
        //  0123456789012345678901
        let year   = bytes_to_number::<u16>(&b[0..4])?;
        let month  = bytes_to_number::<u8>(&b[4..6])?;
        let day    = bytes_to_number::<u8>(&b[6..8])?;
        // b[8] == b'-'
        let hour   = bytes_to_number::<u8>(&b[9..11])?;
        // b[11] == b':'
        let minute = bytes_to_number::<u8>(&b[12..14])?;
        // b[14] == b':'
        let second = bytes_to_number::<u8>(&b[15..17])?;

        let millis = if b.len() >= 21 && b[17] == b'.' {
            bytes_to_number::<u16>(&b[18..21])?
        } else {
            0
        };

        Some(Self { year, month, day, hour, minute, second, millis, micros: None, nanos: None })
    }

    pub fn to_instant(&self) -> std::time::Instant {
        let unix_ms = self.to_unix_ms();
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let offset_ms = unix_ms.saturating_sub(now_ms);
        std::time::Instant::now() + std::time::Duration::from_millis(offset_ms)
    }

    pub fn to_fix_bytes(&self) -> [u8; 26] {
        let mut buf = [0u8; 26];

        let binding_year = number_to_bytes(self.year);
        let year_bytes = binding_year.as_bytes();
        let binding_month = number_to_bytes(self.month);
        let month_bytes = binding_month.as_bytes();
        let binding_day = number_to_bytes(self.day);
        let day_bytes = binding_day.as_bytes();
        let binding_hour = number_to_bytes(self.hour);
        let hour_bytes = binding_hour.as_bytes();
        let binding_minute = number_to_bytes(self.minute);
        let minute_bytes = binding_minute.as_bytes();
        let binding_second = number_to_bytes(self.second);
        let second_bytes = binding_second.as_bytes();
        let binding_millis = number_to_bytes(self.millis);
        let millis_bytes = binding_millis.as_bytes();

        copy_array(&mut buf[0..4], year_bytes);
        if month_bytes.len() == 1 {
            buf[4] = b'0'; // zero-pad month
            copy_array(&mut buf[5..6], month_bytes);
        } else {
            copy_array(&mut buf[4..6], month_bytes);
        }
        if day_bytes.len() == 1 {
            buf[6] = b'0'; // zero-pad day
            copy_array(&mut buf[7..8], day_bytes);
        } else {
            copy_array(&mut buf[6..8], day_bytes);
        }
        buf[8] = b'-';

        if hour_bytes.len() == 1 {
             buf[9] = b'0'; // zero-pad hour
             copy_array(&mut buf[10..11], hour_bytes);
        } else {
            copy_array(&mut buf[9..11], hour_bytes);
        }
        buf[11] = b':';
        if minute_bytes.len() == 1 {
            buf[12] = b'0'; // zero-pad minute
            copy_array(&mut buf[13..14], minute_bytes);
        } else {
            copy_array(&mut buf[12..14], minute_bytes);
        }
        buf[14] = b':';
        if second_bytes.len() == 1 {
            buf[15] = b'0'; // zero-pad second
            copy_array(&mut buf[16..17], second_bytes);
        } else {
            copy_array(&mut buf[15..17], second_bytes);
        }

        if self.millis > 0 {
            buf[17] = b'.';
            copy_array(&mut buf[18..21], millis_bytes);
            return buf;
        }

        buf[17] = 0; // null-terminate if no millis
        buf
    }

    pub fn now() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        Self::from_unix_ns(now.as_nanos() as u64)
    }

    /// Convert to unix timestamp in milliseconds
    pub fn to_unix_ms(&self) -> u64 {
        // days since unix epoch (1970-01-01)
        let days = days_since_epoch(self.year, self.month, self.day) as u64;
        let ms = days        * 86_400_000
               + self.hour   as u64 * 3_600_000
               + self.minute as u64 *    60_000
               + self.second as u64 *     1_000
               + self.millis as u64;
        ms
    }

    // Convert to unix timestamp in nanoseconds
    pub fn to_unix_ns(&self) -> u64 {
        let ms = self.to_unix_ms();
        let micros = self.micros.unwrap_or(0) as u64;
        let nanos = self.nanos.unwrap_or(0) as u64;
        ms * 1_000_000 + micros * 1_000 + nanos
    }

    pub fn from_unix_ms(ms: u64) -> Self {
        let days = ms.div_euclid(86_400_000);
        let time_ms = ms.rem_euclid(86_400_000);

        let z   = days + 719_467;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);                              // [0, 146096]
        let yoe = (doe - doe/1_460 + doe/36_524 - doe/146_096) / 365; // [0, 399] -- was doe/4, must be doe/1460
        let y   = yoe + era * 400;
        let doy = doe - (365*yoe + yoe/4 - yoe/100);                 // [0, 365]
        let m   = (5*doy + 2) / 153;                                  // [0, 11]
        let d   = doy - (153*m + 2)/5 + 1;                           // [1, 31]
        let m   = if m < 10 { m + 3 } else { m - 9 };
        let y   = if m <= 2 { y + 1 } else { y };

        let hour   = (time_ms / 3_600_000) as u8;
        let minute = ((time_ms % 3_600_000) / 60_000) as u8;
        let second = ((time_ms % 60_000) / 1_000) as u8;
        let millis = (time_ms % 1_000) as u16;

        Self {
            year: y as u16, month: m as u8, day: d as u8,
            hour, minute, second, millis, micros: None, nanos: None
        }
    }

    pub fn from_unix_ns(ns: u64) -> Self {
        let ms = ns / 1_000_000;
        let micros = (ns / 1_000) % 1_000;
        let nanos = ns % 1_000;

        let days = ms.div_euclid(86_400_000);
        let time_ms = ms.rem_euclid(86_400_000);

        let z   = days + 719_467;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);                              // [0, 146096]
        let yoe = (doe - doe/1_460 + doe/36_524 - doe/146_096) / 365; // [0, 399] -- was doe/4, must be doe/1460
        let y   = yoe + era * 400;
        let doy = doe - (365*yoe + yoe/4 - yoe/100);                 // [0, 365]
        let m   = (5*doy + 2) / 153;                                  // [0, 11]
        let d   = doy - (153*m + 2)/5 + 1;                           // [1, 31]
        let m   = if m < 10 { m + 3 } else { m - 9 };
        let y   = if m <= 2 { y + 1 } else { y };

        let hour   = (time_ms / 3_600_000) as u8;
        let minute = ((time_ms % 3_600_000) / 60_000) as u8;
        let second = ((time_ms % 60_000) / 1_000) as u8;
        let millis = (time_ms % 1_000) as u16;
        let micros = micros as u16;
        let nanos = nanos as u16;

        Self {
            year: y as u16, month: m as u8, day: d as u8,
            hour, minute, second, millis, micros: Some(micros), nanos: Some(nanos)
        }
    }
}

/// Proleptic Gregorian calendar days since 1970-01-01
fn days_since_epoch(year: u16, month: u8, day: u8) -> u32 {
    let y = year as u32;
    let m = month as u32;
    let d = day as u32;

    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };

    let era = y / 400;
    let yoe = y % 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era * 146_097 + doe - 719_467  // was 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utc_timestamp() {
        let ts = UtcTimestamp::from_fix_bytes(b"20240219-12:30:00.123").unwrap();
        assert_eq!(ts.year, 2024);
        assert_eq!(ts.month, 2);
        assert_eq!(ts.day, 19);
        assert_eq!(ts.hour, 12);
        assert_eq!(ts.minute, 30);
        assert_eq!(ts.second, 0);
        assert_eq!(ts.millis, 123);
    }

    #[test]
    fn test_utc_timestamp_unix_conversion() {
        let ts = UtcTimestamp { year: 2024, month: 2, day: 19, hour: 12, minute: 30, second: 0, millis: 123, micros: None, nanos: None };
        let unix_ms = ts.to_unix_ms();
        let ts_converted = UtcTimestamp::from_unix_ms(unix_ms);
        assert_eq!(ts, ts_converted);
    }

    #[test]
    fn test_utc_timestamp_unix_conversion_ns() {
        let ts = UtcTimestamp { year: 2024, month: 2, day: 19, hour: 12, minute: 30, second: 0, millis: 123, micros: Some(456), nanos: Some(789) };
        let unix_ns = ts.to_unix_ns();
        let ts_converted = UtcTimestamp::from_unix_ns(unix_ns);
        assert_eq!(ts, ts_converted);
    }

    #[test]
    fn test_utc_timestamp_fix_bytes() {
        let ts = UtcTimestamp { year: 2024, month: 2, day: 19, hour: 12, minute: 30, second: 0, millis: 123, micros: None, nanos: None };
        let fix_bytes = ts.to_fix_bytes();
        let ts_converted = UtcTimestamp::from_fix_bytes(&fix_bytes).unwrap();
        assert_eq!(ts, ts_converted);
    }

    #[test]
    fn test_bytes_to_number() {
        assert_eq!(bytes_to_number::<u64>(b"12345"), Some(12345));
        assert_eq!(bytes_to_number::<u64>(b"00001"), Some(1));
        assert_eq!(bytes_to_number::<u64>(b"abc"), None);
        assert_eq!(bytes_to_number::<u32>(b"12346"), Some(12346));
        assert_eq!(bytes_to_number::<u32>(b"00002"), Some(2));
        assert_eq!(bytes_to_number::<u32>(b"abc"), None);
        assert_eq!(bytes_to_number::<u16>(b"12345"), Some(12345));
        assert_eq!(bytes_to_number::<u16>(b"00001"), Some(1));
        assert_eq!(bytes_to_number::<u16>(b"abc"), None);
    }

    #[test]
    fn test_number_to_bytes() {
        assert_eq!(number_to_bytes(12345u64).as_bytes(), b"12345");
        assert_eq!(number_to_bytes(0u64).as_bytes(), b"0");
        assert_eq!(number_to_bytes(12345u32).as_bytes(), b"12345");
        assert_eq!(number_to_bytes(0u32).as_bytes(), b"0");
    }   
}