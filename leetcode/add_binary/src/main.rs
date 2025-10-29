pub fn add_binary(a: String, b: String) -> String {
    let mut a_chars = a.chars().rev();
    let mut b_chars = b.chars().rev();
    let mut res = String::new();

    if a.is_empty() { return b };
    if b.is_empty() { return a };

    let mut carry = 0;

    loop {
        match (a_chars.next(), b_chars.next()) {
            (Some(a_char), Some(b_char)) => {
                let sum = a_char.to_digit(2).unwrap() + b_char.to_digit(2).unwrap() + carry;
                res.push(char::from_digit(sum % 2, 2).unwrap());
                carry = sum / 2;
            },
            (Some(a_char), None) => {
                let sum = a_char.to_digit(2).unwrap() + carry;
                res.push(char::from_digit(sum % 2, 2).unwrap());
                carry = sum / 2;
            },
            (None, Some(b_char)) => {
                let sum = b_char.to_digit(2).unwrap() + carry;
                res.push(char::from_digit(sum % 2, 2).unwrap());
                carry = sum / 2;
            },
            (None, None) => {
                if carry == 1 { res.push('1'); }
                break;
            }
        }
    }

    println!("{res}");

    res.chars().rev().collect()
}

fn main() {
    assert_eq!(add_binary("11".to_string(), "1".to_string()), "100");
    assert_eq!(add_binary("1".to_string(), "".to_string()), "1");
    assert_eq!(add_binary("".to_string(), "0".to_string()), "0");
    assert_eq!(add_binary("1".to_string(), "0100".to_string()), "0101");
    assert_eq!(add_binary("1010".to_string(), "1011".to_string()), "10101");
    assert_eq!(add_binary("1111".to_string(), "1111".to_string()), "11110");

}