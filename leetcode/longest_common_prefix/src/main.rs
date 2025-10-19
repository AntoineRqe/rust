pub fn longest_common_prefix(strs: Vec<String>) -> String {
    let mut longest = String::new();

    let mut index = 0;
    let mut current = char::default();

    while true {
        for str in &strs {
            if let Some(c) = str.chars().nth(index) {
                if current == char::default() {
                    current = c;
                } else if c != current {
                    return longest;
                } else {
                    continue;
                }
            } else {
                return longest;
            }
        }
        
        longest.push(current);
        current = char::default();
        index += 1;
    }

    longest
}

fn longest_common_prefix_mistral(strs: Vec<String>) -> String {
    if strs.is_empty() {
        return String::new();
    }

    let first = strs[0].as_str();
    let mut prefix = String::new();

    for (i, c) in first.chars().enumerate() {
        for s in &strs {
            if let Some(sc) = s.chars().nth(i) {
                if sc != c {
                    return prefix;
                }
            } else {
                return prefix;
            }
        }
        prefix.push(c);
    }

    prefix
}

fn main() {
    println!("Hello, world!");
}
