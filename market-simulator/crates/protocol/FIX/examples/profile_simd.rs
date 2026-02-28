use fix::parser::{FixParser};

const ITERATIONS: usize = 1_000_000;

fn main() {
    let data = build_fix_message::<65536>();

    for _ in 0..ITERATIONS {
        let mut parser = FixParser::new(&data);
        let message = parser.get_fields();

        std::hint::black_box(message);
    }
}

const fn build_fix_message<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    let mut i = 0;

    // Basic FIX header
    let header = b"8=FIX.4.4\x019=0000\x0135=D\x0149=SENDER\x0156=TARGET\x0134=1\x0152=20240219-12:30:00.000\x01";

    let mut h = 0;
    while h < header.len() {
        buf[i] = header[h];
        i += 1;
        h += 1;
    }

    // Repeat fields until we hit ~64 KB
    while i + 32 < N {
        let field = b"55=EURUSD\x0138=1000000\x0144=1.23456\x01";

        let mut f = 0;
        while f < field.len() {
            buf[i] = field[f];
            i += 1;
            f += 1;
        }
    }

    // Trailer
    let trailer = b"10=123\x01";
    let mut t = 0;
    while t < trailer.len() {
        buf[i] = trailer[t];
        i += 1;
        t += 1;
    }

    buf
}