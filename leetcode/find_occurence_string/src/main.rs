pub fn str_str(haystack: String, needle: String) -> i32 {
    let first_needle = needle.chars().next().unwrap();
    let needle_len = needle.len();
    for (index , c) in haystack.chars().enumerate() {
        println!("{index} -> {c}");
        if c == first_needle {
            println!("{} vs {}", index + needle_len, haystack.len());
            if (index + needle_len) > haystack.len() {
                return -1;
            }

            if haystack[index..(index + needle_len)] == needle {
                return index as i32;
            }
        }
    }
    -1 as i32
}

fn main() {
    assert_eq!(str_str("dadbutnotsad".to_string(), "sad".to_string()), 9);
}