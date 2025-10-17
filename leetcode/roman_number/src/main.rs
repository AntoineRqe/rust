pub fn roman_to_int(s: String) -> i32 {

    let mut result : i32 = 0;
    let mut prev_value : i32 = 0;
    let bytes = s.as_bytes();

    for &b in bytes.iter().rev() {
        let value = match b {
            b'I' => 1,
            b'V' => 5,
            b'X' => 10,
            b'L' => 50,
            b'C' => 100,
            b'D' => 500,
            b'M' => 1000,
            _ => 0
        };

        if value < prev_value {
            result -= value
        } else {
            result += value;
        }

        prev_value = value;
    }

    result
}


fn main() {

    //println!("{}", roman_to_int("III".to_string()));
    //println!("{}", roman_to_int("LVIII".to_string()));
    println!("{}", roman_to_int("MCMXCIV".to_string()));
}

