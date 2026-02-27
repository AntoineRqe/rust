use std::simd::{u8x64, cmp::SimdPartialEq};
// My computer has avx512f SIMD register width, so I will use 64 bytes wide vectors.

/// SOH delimiter byte (ASCII 0x01), separating tag=value pairs.
const SOH: u8 = 0x01;
const EQUALS: u8 = b'=';

/// Pre-defined FIX tags we care about on the hot path.
/// Using constants rather than an enum avoids match overhead.
pub mod tags {
    pub const MSG_TYPE: u32 = 35;
    pub const SENDER_COMP_ID: u32 = 49;
    pub const TARGET_COMP_ID: u32 = 56;
    pub const MSG_SEQ_NUM: u32 = 34;
    pub const SENDING_TIME: u32 = 52;
    pub const ORDER_ID: u32 = 37; // Unique order ID assigned by the exchange, for the sell side to track orders
    pub const CL_ORD_ID: u32 = 11;  // Client order ID, unique per order, for the buy side to track their orders
    pub const EXEC_ID: u32 = 17;
    pub const EXEC_TYPE: u32 = 150;
    pub const ORD_STATUS: u32 = 39;
    pub const SYMBOL: u32 = 55;
    pub const SIDE: u32 = 54;
    pub const ORDER_QTY: u32 = 38;
    pub const PRICE: u32 = 44;
    pub const LAST_QTY: u32 = 32;
    pub const LAST_PX: u32 = 31;
    pub const CUM_QTY: u32 = 14;
    pub const LEAVES_QTY: u32 = 151;
    pub const HEARTBEAT_INT: u32 = 108;
    pub const TEST_REQ_ID: u32 = 112;
    pub const BEGIN_SEQ_NO: u32 = 7;
    pub const END_SEQ_NO: u32 = 16;
    pub const CHECKSUM: u32 = 10;
}

/// Pre-defined FIX message types we care about on the hot path.
pub mod msg_types {
    pub const NEW_ORDER_SINGLE: &[u8] = b"D";
    pub const EXECUTION_REPORT: &[u8] = b"8";
    pub const ORDER_CANCEL_REQUEST: &[u8] = b"F";
    pub const ORDER_CANCEL_REPLACE_REQUEST: &[u8] = b"G";
}

pub mod side_types {
    pub const BUY: u32 = b'1' as u32;
    pub const SELL: u32 = b'2' as u32;
}

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

/// A parsed FIX message, containing a vector of fields. Each field is a tag-value pair, where the tag is a u32 and the value is a byte slice referencing the original message data.
pub struct FixMessage<'a> {
    pub fields: Vec<FixField<'a>>,
}

/// Find SOH positions in the input buffer using SIMD instructions.
/// Arguments:
/// - `buf`: The input byte slice containing the FIX message.
/// Returns:
/// - A vector of indices where the SOH delimiter (0x01) is found in the input buffer.
#[allow(dead_code)]
fn find_delimiter_positions(buf: &[u8], delimiter: u8) -> Vec<usize> {
    let mut positions = Vec::new();
    let len = buf.len();
    let chunks = len / 64; // Number of full 64-byte chunks

    let delimiter_vec = u8x64::splat(delimiter);

    for i in 0..chunks {
        let offset = i * 64;

        let block = u8x64::from_slice(&buf[offset..offset + 64]);
        let mask = block.simd_eq(delimiter_vec);

        let mut bits = mask.to_bitmask();

        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            positions.push(offset + bit);
            bits &= bits - 1;
        }
    }

    // Handle remaining bytes that don't fit into a full 64-byte chunk
    for i in (chunks * 64)..len {
        if buf[i] == delimiter {
            positions.push(i);
        }
    }

    positions
}

#[inline(always)]
pub fn parse_tag_fast(bytes: &[u8]) -> u32 {
    // FIX tags are 1-4 digits, handle each case with zero branching on the hot path
    // Uses the fact that ASCII digits are 0x30-0x39, so b - b'0' extracts the digit
    match bytes.len() {
        1 => (bytes[0] - b'0') as u32,
        2 => (bytes[0] - b'0') as u32 * 10
            + (bytes[1] - b'0') as u32,
        3 => (bytes[0] - b'0') as u32 * 100
            + (bytes[1] - b'0') as u32 * 10
            + (bytes[2] - b'0') as u32,
        4 => (bytes[0] - b'0') as u32 * 1000
            + (bytes[1] - b'0') as u32 * 100
            + (bytes[2] - b'0') as u32 * 10
            + (bytes[3] - b'0') as u32,
        // should never happen for valid FIX
        _ => bytes.iter().fold(0u32, |v, &b| v * 10 + (b - b'0') as u32)
    }
}

impl<'a> FixParser<'a> {
    #[inline(always)]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline(always)]
    /// Parse all field in data and return them as a vector of FixField. It uses SIMD for efficient parsing of the fields.
    pub fn get_fields(&mut self) -> FixMessage<'a> {
        if self.pos >= self.data.len() {
            return FixMessage { fields: Vec::new() };
        }

        let data = self.data;
        let len = data.len();
        let mut i  = self.pos;

        if i >= len {
            return FixMessage { fields: Vec::new() };
        }

        // Estimate capacity better upfront
        let estimated_fields = (len - i) / 10;
        let mut fields = Vec::with_capacity(estimated_fields.max(64)); // At least 64 fields capacity for small messages

        let soh_vec = u8x64::splat(SOH);
        let equal_vec = u8x64::splat(EQUALS);
    
        let mut start_field = i;

        while i + 64 <= len {
            let block = u8x64::from_slice(&data[i..i + 64]);
            let soh_mask = block.simd_eq(soh_vec).to_bitmask();
            let equal_mask = block.simd_eq(equal_vec).to_bitmask();
      
            let mut soh_bits = soh_mask;

            while soh_bits != 0 {
                let bit = soh_bits.trailing_zeros() as usize;
                let field_end = i + bit;
    
                // Check if the field start is in this block, if not, we need to look for the '=' in the previous block
                let range_mask = if start_field >= i {
                    // field started in this block
                    let range_start = start_field - i;
                    let window = ((1u64 << bit) - 1) & !((1u64 << range_start) - 1);
                    equal_mask & window
                } else {
                    // field started in previous block, but = might still be in this block
                    // if start_field is only slightly behind i (field spans the boundary)
                    // search from bit 0 up to soh_bit
                    let window = (1u64 << bit) - 1;  // bits 0..soh_bit
                    let mask = equal_mask & window;
                    if mask != 0 {
                        mask  // = is in this block, before the SOH
                    } else {
                        0     // = is truly in a previous block, scalar fallback
                    }
                };

                let eq_pos_abs = if range_mask != 0 {
                    // = found in this block
                    i + range_mask.trailing_zeros() as usize
                } else {
                    // = is in a previous block, scan scalar (rare for well-formed FIX)
                    match data[start_field..field_end].iter().position(|&b| b == EQUALS) {
                        Some(p) => start_field + p,
                        None    => { soh_bits &= soh_bits - 1; continue; }
                    }
                };

                let tag_bytes   = &data[start_field..eq_pos_abs];
                let value_bytes = &data[eq_pos_abs + 1..field_end];
                let tag         = parse_tag_fast(tag_bytes);
                fields.push(FixField { tag, value: value_bytes });

                start_field  = field_end + 1;
                soh_bits    &= soh_bits - 1;
            }
    
            i += 64;
        }

        // Handle remaining bytes that don't fit into a full 64-byte chunk
        while i < len {
            if data[i] == SOH {
                let field_end = i;
                if let Some(eq_pos) = data[start_field..field_end].iter().position(|&b| b == EQUALS) {
                    let tag_bytes = &data[start_field..start_field + eq_pos];
                    let value_bytes = &data[start_field + eq_pos + 1..field_end];
                    let tag = parse_tag_fast(tag_bytes);
                    fields.push(FixField { tag, value: value_bytes });
                }
                start_field = field_end + 1;
            }
            i += 1;
        }

        FixMessage { fields }
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
        let data = b"8=FIX.4.2\x019=12\x0135=D\x018=FIX.4.2\x019=12\x0135=D\x018=FIX.4.2\x019=12\x0135=D\x018=FIX.4.2\x019=12\x0135=D\x01";
        let mut parser = FixParser::new(data);
        let message = parser.get_fields();
        let fields = message.fields;

        assert_eq!(fields.len(), 12);

        for i in 0..4 {
            assert_eq!(fields[i*3].tag, 8);
            assert_eq!(fields[i*3].value, b"FIX.4.2");
            assert_eq!(fields[i*3 + 1].tag, 9);
            assert_eq!(fields[i*3 + 1].value, b"12");
            assert_eq!(fields[i*3 + 2].tag, 35);
            assert_eq!(fields[i*3 + 2].value, b"D");
        }
    }
}