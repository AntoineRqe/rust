fn is_palindrome(x: i32) -> bool {

    if x < 0 { return false };

    let x_str = x.to_string();
    let chars : Vec<_> = x_str.chars().collect();

    let len = chars.len();

    for i in 0 .. len/2 {
        let i = i as usize;
        if chars[i] != chars[len - 1 -i] {
            return false;
        }
    } 

    true    
}

fn reverse_str(s: &String) -> String {

    let mut s_chars : Vec<_> = s.chars().collect();

    if s.len() == 2 {
        let c = s_chars[0];
        s_chars[0] = s_chars[1];
        s_chars[1] = c;
    } else {
        for i in 0 .. s.len() {
            let j = s.len() - i - 1;

            println!("{i}-{j}");
            
            if i >= j { break; }
            let c = s_chars[i];
            s_chars[i] = s_chars[s.len() - i - 1];
            s_chars[s.len() - i - 1] = c;
        
        }
    }

    return s_chars.iter().collect();
}

fn main() {
    println!("Hello, world!");
    let _ = is_palindrome(1122);
}
