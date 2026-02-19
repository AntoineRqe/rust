use std::simd::{u8x32, cmp::SimdPartialEq};

/// SOH delimiter byte (ASCII 0x01), separating tag=value pairs.
const SOH: u8 = 0x01;
const EQUALS: u8 = b'=';


/// Both tag and value are slices into the original TCP payload -
/// zero copies, zero allocations.
#[derive(Debug, Clone, Copy)]
pub struct FixField<'a> {
    pub tag: u32,
    pub value: &'a [u8],
}

/// FIX parser that operates on a byte slice, returning an iterator over FIX fields (tag=value pairs).
pub struct FixParser<'a> {
    data: &'a [u8],
    pos: usize,
}

/// Find SOH positions in the input buffer using SIMD instructions.
/// Arguments:
/// - `buf`: The input byte slice containing the FIX message.
/// Returns:
/// - A vector of indices where the SOH delimiter (0x01) is found in the input buffer.
fn find_delimiter_positions(buf: &[u8], delimiter: u8) -> Vec<usize> {
    let mut positions = Vec::new();
    let len = buf.len();
    let chunks = len / 32;

    for i in 0..chunks {
        let offset = i * 32;

        let block = u8x32::from_slice(&buf[offset..offset + 32]);
        let mask = block.simd_eq(u8x32::splat(delimiter));

        let mut bits = mask.to_bitmask();

        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            positions.push(offset + bit);
            bits &= bits - 1;
        }
    }

    for i in (chunks * 32)..len {
        if buf[i] == delimiter {
            positions.push(i);
        }
    }

    positions
}

#[inline(always)]
fn parse_tag_fast(bytes: &[u8]) -> u32 {
    let mut v = 0u32;
    for &b in bytes {
        v = v * 10 + (b - b'0') as u32;
    }
    v
}

impl<'a> FixParser<'a> {
    #[inline(always)]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline(always)]
    /// Parse all field in data and return them as a vector of FixField. It uses SIMD for efficient parsing of the fields.
    pub fn get_fields(&mut self) -> Vec<FixField<'a>> {
        if self.pos >= self.data.len() {
            return Vec::new();
        }

        let soh_positions = find_delimiter_positions(&self.data[self.pos..], SOH);

        if soh_positions.is_empty() {
            return Vec::new(); // Malformed message, no more fields
        }

        let mut fields = Vec::new();

        for pos in soh_positions.iter() {
            if *pos >= self.data.len() {
                break; // Malformed message, SOH beyond end
            }

            let field_data = &self.data[self.pos..*pos];
            self.pos += field_data.len() + 1; // Move past this field and the SOH

            if let Some(eq_pos) = field_data.iter().position(|&b| b == EQUALS) {
                let tag_str = &field_data[..eq_pos];
                let value = &field_data[eq_pos + 1..];

                let tag = parse_tag_fast(tag_str);
                fields.push(FixField { tag, value });
            }
        }
        fields
    }

    /// Parse the next tag=value field. Returns None at end of message.
    #[inline(always)]
    pub fn next_field_scalar(&mut self) -> Option<FixField<'a>> {
        if self.pos >= self.data.len() {
            return None;
        }
        // Parse tag (digits before '=')
        let mut tag: u32 = 0;

        #[cfg(feature = "branchless")]
        while self.pos < self.data.len() && self.data[self.pos] != EQUALS {
            // Branchless digit accumulation, trust that input is well-formed (digits only)
            tag = tag * 10 + (self.data[self.pos] - b'0') as u32;
            self.pos += 1;
        }

        #[cfg(not(feature = "branchless"))]
        // Actually doesn't change a thing on perf, my guess is that the branch predictor is doing a good job here since the input is well-formed and the loop will always exit on '='
        while self.pos < self.data.len() && self.data[self.pos] != EQUALS {
            let c = self.data[self.pos];
            if c >= b'0' && c <= b'9' {
                tag = tag * 10 + (c - b'0') as u32;
            } else {
                break;
            }
            self.pos += 1;
        }

        if self.pos >= self.data.len() {
            return None; // Malformed: no '='
        }

        self.pos += 1; // Skip '='

        // Parse value (bytes before SOH)
        let value_start = self.pos;
    
        while self.pos < self.data.len() && self.data[self.pos] != SOH {
            self.pos += 1;
        }

        let value = &self.data[value_start..self.pos];

        if self.pos < self.data.len() {
            self.pos += 1; // Skip SOH for next field
        }

        Some(FixField { tag, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_field_scalar() {
        let data = b"8=FIX.4.2\x019=12\x0135=D\x01";
        let mut parser = FixParser::new(data);

        let field = parser.next_field_scalar().unwrap();
        assert_eq!(field.tag, 8);
        assert_eq!(field.value, b"FIX.4.2");

        let field = parser.next_field_scalar().unwrap();
        assert_eq!(field.tag, 9);
        assert_eq!(field.value, b"12");

        let field = parser.next_field_scalar().unwrap();
        assert_eq!(field.tag, 35);
        assert_eq!(field.value, b"D");

        assert!(parser.next_field_scalar().is_none());
    }

    #[test]
    fn test_get_fields() {
        let data = b"8=FIX.4.2\x019=12\x0135=D\x01";
        let mut parser = FixParser::new(data);
        let fields = parser.get_fields();

        println!("{:?}", fields);
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].tag, 8);
        assert_eq!(fields[0].value, b"FIX.4.2");
        assert_eq!(fields[1].tag, 9);
        assert_eq!(fields[1].value, b"12");
        assert_eq!(fields[2].tag, 35);
        assert_eq!(fields[2].value, b"D");
    }
}