//! A small JSON reader, for KiCad's project files and DRC reports.
//!
//! Numbers keep their text, so that lengths in millimetres convert exactly, and objects
//! keep their keys in file order, so that nothing depends on hashing.

use super::KicadError;

/// A JSON value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    /// A number, as written.
    Number(String),
    String(String),
    Array(Vec<Json>),
    /// An object's members, in file order.
    Object(Vec<(String, Json)>),
}

/// The deepest nesting accepted, as for S-expressions.
const MAX_DEPTH: usize = 256;

/// Parses a whole document.
pub(crate) fn parse(text: &str) -> Result<Json, KicadError> {
    let mut p = Parser { text, pos: 0 };
    let value = p.value(0)?;
    p.skip_space();
    if p.pos < text.len() {
        return Err(p.error("unexpected text after the end of the document"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> KicadError {
        KicadError::syntax(self.text, self.pos, message)
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, word: &str) -> Result<(), KicadError> {
        if self.text[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(self.error(&format!("expected {word}")))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, KicadError> {
        if depth >= MAX_DEPTH {
            return Err(self.error("values are nested too deeply"));
        }
        self.skip_space();
        match self.peek() {
            Some(b'{') => {
                self.pos += 1;
                let mut members = Vec::new();
                self.skip_space();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    return Ok(Json::Object(members));
                }
                loop {
                    self.skip_space();
                    if self.peek() != Some(b'"') {
                        return Err(self.error("expected a key"));
                    }
                    let key = self.string()?;
                    self.skip_space();
                    self.expect(":")?;
                    members.push((key, self.value(depth + 1)?));
                    self.skip_space();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Json::Object(members));
                        }
                        _ => return Err(self.error("expected , or }")),
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.skip_space();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(self.error("expected , or ]")),
                    }
                }
            }
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') => self.expect("true").map(|()| Json::Bool(true)),
            Some(b'f') => self.expect("false").map(|()| Json::Bool(false)),
            Some(b'n') => self.expect("null").map(|()| Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.pos;
                while matches!(
                    self.peek(),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.pos += 1;
                }
                Ok(Json::Number(self.text[start..self.pos].to_string()))
            }
            _ => Err(self.error("expected a value")),
        }
    }

    /// A string, starting at its opening quote.
    fn string(&mut self) -> Result<String, KicadError> {
        let open = self.pos;
        self.pos += 1;
        let mut out = String::new();
        let mut chars = self.text[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => {
                    let Some((_, e)) = chars.next() else { break };
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'u' => {
                            let Some(first) = hex4(&mut chars) else { break };
                            let code = if (0xD800..0xDC00).contains(&first) {
                                // A surrogate pair: the second half follows as \uXXXX.
                                let low = match (chars.next(), chars.next()) {
                                    (Some((_, '\\')), Some((_, 'u'))) => hex4(&mut chars),
                                    _ => None,
                                };
                                match low {
                                    Some(low @ 0xDC00..0xE000) => {
                                        0x10000 + ((first - 0xD800) << 10) + (low - 0xDC00)
                                    }
                                    _ => 0xFFFD,
                                }
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
        self.pos = open;
        Err(self.error("this string is never closed"))
    }
}

/// Four hexadecimal digits, as after `\u`.
fn hex4(chars: &mut impl Iterator<Item = (usize, char)>) -> Option<u32> {
    let hex: String = chars.take(4).map(|(_, c)| c).collect();
    u32::from_str_radix(&hex, 16)
        .ok()
        .filter(|_| hex.len() == 4)
}

impl Json {
    /// The member of an object with that key.
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value at a path of keys.
    pub(crate) fn at(&self, path: &[&str]) -> Option<&Json> {
        path.iter().try_fold(self, |v, key| v.get(key))
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_number(&self) -> Option<&str> {
        match self {
            Json::Number(n) => Some(n),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }

    pub(crate) fn as_object(&self) -> &[(String, Json)] {
        match self {
            Json::Object(members) => members,
            _ => &[],
        }
    }

    /// Writes the value as JSON text.
    #[cfg(test)]
    pub(crate) fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => out.push_str(n),
            Json::String(s) => write_string(s, out),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

#[cfg(test)]
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes() {
        let text = r#" {"a": [1, -2.5e3, true, false, null], "b": {"c": "x\"\\\né😀"}, "d": []} "#;
        let v = parse(text).unwrap();
        assert_eq!(v.at(&["b", "c"]).unwrap().as_str(), Some("x\"\\\né😀"));
        assert_eq!(
            v.get("a").unwrap().as_array()[1].as_number(),
            Some("-2.5e3")
        );
        let mut out = String::new();
        v.write(&mut out);
        assert_eq!(parse(&out).unwrap(), v);
        for bad in ["{", "[1,]", "{\"a\" 1}", "\"open", "tru", "{} x", ""] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
