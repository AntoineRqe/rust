pub fn is_valid(s: String) -> bool {
    let mut filo : Vec<char> = Vec::new();
    
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => filo.push(c),
            ')' => if let Some (cs) = filo.last() 
            {
                if *cs == '(' {
                    filo.pop();
                } else {
                    return false
                }
            } else {
                return false;
            },
            ']' => if let Some (cs) = filo.last() 
            {
                if *cs == '[' {
                    filo.pop();
                } else {
                    return false
                }
            } else {
                return false;
            },
            '}' => if let Some (cs) = filo.last() 
            {
                if *cs == '{' {
                    filo.pop();
                } else {
                    return false
                }
            } else {
                return false;
            },
            _ => return false,
        }
        println!("filo : {filo:?}");
    }

    return filo.is_empty();   
}

fn main() {
    assert_eq!(is_valid(String::from("()")), true);
    assert_eq!(is_valid(String::from("()[]{}")), true);
    assert_eq!(is_valid(String::from("(]")), false);
    assert_eq!(is_valid(String::from("([])")), true);
    assert_eq!(is_valid(String::from("([)]")), false);
}