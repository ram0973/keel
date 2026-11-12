//! Лексер Keel.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(i64),
    Ident(String),
    Str(String),
    Punct(String),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

const TWO_CHAR: &[&str] = &[
    "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "..", "::",
];
const ONE_CHAR: &[&str] = &[
    "+", "-", "*", "/", "%", "=", "<", ">", "!", "&", ".", ",", ";", ":", "(", ")", "[", "]", "{", "}",
];

pub fn lex(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut out = Vec::new();

    while i < chars.len() {
        let c = chars[i];

        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // Комментарии
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }

        // Числа
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            let v = s
                .parse::<i64>()
                .map_err(|_| format!("line {}: число слишком большое: {}", line, s))?;
            out.push(Token { tok: Tok::Int(v), line });
            continue;
        }

        // Идентификаторы и ключевые слова
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            out.push(Token { tok: Tok::Ident(s), line });
            continue;
        }

        // Строки
        if c == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    s.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        '\\' => '\\',
                        '"' => '"',
                        other => other,
                    });
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= chars.len() {
                return Err(format!("line {}: незакрытая строка", line));
            }
            i += 1;
            out.push(Token { tok: Tok::Str(s), line });
            continue;
        }

        // Операторы
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        if TWO_CHAR.contains(&two.as_str()) {
            out.push(Token { tok: Tok::Punct(two), line });
            i += 2;
            continue;
        }
        let one = c.to_string();
        if ONE_CHAR.contains(&one.as_str()) {
            out.push(Token { tok: Tok::Punct(one), line });
            i += 1;
            continue;
        }

        return Err(format!("line {}: неожиданный символ '{}'", line, c));
    }

    out.push(Token { tok: Tok::Eof, line });
    Ok(out)
}
