use std::collections::HashMap;
use std::fs::File;
use std::fmt;
use std::io::{Read};
use json::{object};


 
fn trim_newline(input: &str) -> &str {
    input.strip_suffix("\r\n")
    .or(input.strip_suffix("\n"))
    .unwrap_or(input)
}

pub struct TextAnalysis {
    count: usize,
    words: HashMap<String, u32>,
    file_path: String,
    contents: String
}

impl fmt::Display for TextAnalysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file : {}\ncount : {}\ncontent : {}", self.file_path, self.count, self.contents)
    }
}

impl TextAnalysis {
    pub fn new(file_path : &str) -> Result<TextAnalysis, std::io::Error> {
        let mut data = TextAnalysis {
            count: 0,
            words: HashMap::new(),
            file_path: file_path.to_string(),
            contents: String::new()
        };

        let mut file = File::open(file_path)?;
        file.read_to_string(&mut data.contents)?;

        Ok(data)
    }

    pub fn analyse_file(&mut self) -> Result<(), std::io::Error>{
        let words : Vec<&str> = self.contents.split(' ')
        .filter(|s| !s.is_empty())
        .map(|s| trim_newline(s))
        .collect();

        self.count = words.len();

        for word in words {
            let counter = self.words.entry(word.to_string()).or_insert(0);
            *counter += 1;
        }

        Ok(())
    }

    pub fn build_json(self: &TextAnalysis) -> String {
        let data = object! {
            file: self.file_path.clone(),
            count: self.count,
            words: json::stringify(self.words.clone())
        };

        data.dump()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_file() {
        let my_analyser = TextAnalysis::new("tests/no_file.txt");
        assert_eq!(my_analyser.is_err(), true);
    }

    #[test]
    fn test_empty_file() {
        let mut my_analyser = TextAnalysis::new("tests/empty.txt").unwrap();
        let result = my_analyser.analyse_file();
        assert_eq!(result.is_ok(), true);
        assert_eq!(my_analyser.file_path, "tests/empty.txt");
        assert_eq!(my_analyser.contents, "");
        assert_eq!(my_analyser.count, 0);
        assert_eq!(my_analyser.words.len(), 0);
    }

    #[test]
    fn test_weird_content() {
        let mut my_analyser = TextAnalysis::new("tests/weird.txt").unwrap();
        let result = my_analyser.analyse_file();
        assert_eq!(result.is_ok(), true);
        assert_eq!(my_analyser.file_path, "tests/weird.txt");
        assert_eq!(my_analyser.contents, "a  b    c\n");
        assert_eq!(my_analyser.count, 3);
        assert_eq!(my_analyser.words.get(&"a".to_string()), Some(&1));
        assert_eq!(my_analyser.words.get(&"b".to_string()), Some(&1));
        assert_eq!(my_analyser.words.get(&"c".to_string()), Some(&1));
    }

    #[test]
    fn test_valid_file() {
        let mut my_analyser = TextAnalysis::new("tests/test.txt").unwrap();
        let result = my_analyser.analyse_file();
        assert_eq!(result.is_ok(), true);
        assert_eq!(my_analyser.file_path, "tests/test.txt");
        assert_ne!(my_analyser.contents, "");
        assert_eq!(my_analyser.count, 271);
        assert_eq!(my_analyser.words.get(&"the".to_string()), Some(&11));
        assert_eq!(my_analyser.words.get(&"help".to_string()), Some(&3));
        assert_eq!(my_analyser.words.get(&"words".to_string()), Some(&5));
    }

    #[test]
    fn test_json() {
        let mut my_analyser = TextAnalysis::new("tests/test.txt").unwrap();
        my_analyser.analyse_file().unwrap();
        assert_eq!(my_analyser.build_json().len(), 2293);
    }

}