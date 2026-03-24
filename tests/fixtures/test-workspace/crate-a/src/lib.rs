use regex::Regex;

pub fn is_valid(s: &str) -> bool {
    let re = Regex::new(r"^[a-z]+$").unwrap();
    re.is_match(s)
}

// once_cell is declared as a dependency but never used in source
