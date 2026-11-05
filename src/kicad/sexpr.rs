//! A reader for the S-expressions that KiCad writes its boards in.
//!
//! A file is one list. A list holds atoms and lists, and its first atom names it, as in
//! `(at 1.5 2 90)`. An atom is a run of characters other than whitespace and parentheses,
//! or a string in double quotes with backslash escapes. Nothing is interpreted here: an
//! atom stays text, so that numbers are converted exactly where their units are known.

use std::borrow::Cow;

use super::KicadError;

/// A node: an atom or a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sexp<'a> {
    /// An atom, unquoted or quoted, with the escapes of a quoted one resolved.
    Atom(Cow<'a, str>),
    /// A list.
    List(Vec<Sexp<'a>>),
}

/// The deepest nesting accepted. KiCad's files go about a dozen deep, and a limit keeps a
/// hostile file from overflowing the stack of the code that walks the tree.
const MAX_DEPTH: usize = 256;

/// Parses a whole file, which must be a single list, with nothing but whitespace after it.
pub(crate) fn parse(text: &str) -> Result<Sexp<'_>, KicadError> {
    let mut parser = Parser { text, pos: 0 };
    parser.skip_space();
    if parser.peek() != Some(b'(') {
        return Err(parser.error("expected a list"));
    }
    let root = parser.list(0)?;
    parser.skip_space();
    if parser.pos < text.len() {
        return Err(parser.error("unexpected text after the end of the file's list"));
    }
    Ok(root)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn error(&self, message: &str) -> KicadError {
        KicadError::syntax(self.text, self.pos, message)
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// A list, starting at its opening parenthesis.
    fn list(&mut self, depth: usize) -> Result<Sexp<'a>, KicadError> {
        if depth >= MAX_DEPTH {
            return Err(self.error("lists are nested too deeply"));
        }
        let start = self.pos;
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                None => {
                    self.pos = start;
                    return Err(self.error("this list is never closed"));
                }
                Some(b')') => {
                    self.pos += 1;
                    return Ok(Sexp::List(items));
                }
                Some(b'(') => items.push(self.list(depth + 1)?),
                Some(b'"') => items.push(self.quoted()?),
                Some(_) => items.push(self.bare()),
            }
        }
    }

    /// An unquoted atom, which ends at whitespace, a parenthesis or a quote.
    fn bare(&mut self) -> Sexp<'a> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b'(' | b')' | b'"') {
                break;
            }
            self.pos += 1;
        }
        Sexp::Atom(Cow::Borrowed(&self.text[start..self.pos]))
    }

    /// A string in double quotes. KiCad escapes quotes and backslashes, and writes `\n`
    /// for a line break. Any other escaped character stands for itself.
    fn quoted(&mut self) -> Result<Sexp<'a>, KicadError> {
        let open = self.pos;
        self.pos += 1;
        let start = self.pos;
        let bytes = self.text.as_bytes();
        // Most strings have no escapes, and can be borrowed.
        while let Some(&c) = bytes.get(self.pos) {
            match c {
                b'"' => {
                    let atom = &self.text[start..self.pos];
                    self.pos += 1;
                    return Ok(Sexp::Atom(Cow::Borrowed(atom)));
                }
                b'\\' => break,
                _ => self.pos += 1,
            }
        }
        let mut owned = String::from(&self.text[start..self.pos]);
        let mut chars = self.text[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos += i + 1;
                    return Ok(Sexp::Atom(Cow::Owned(owned)));
                }
                '\\' => match chars.next() {
                    Some((_, 'n')) => owned.push('\n'),
                    Some((_, 'r')) => owned.push('\r'),
                    Some((_, 't')) => owned.push('\t'),
                    Some((_, other)) => owned.push(other),
                    None => break,
                },
                c => owned.push(c),
            }
        }
        self.pos = open;
        Err(self.error("this string is never closed"))
    }
}

impl<'a> Sexp<'a> {
    /// The text of an atom.
    pub(crate) fn as_atom(&self) -> Option<&str> {
        match self {
            Sexp::Atom(text) => Some(text),
            Sexp::List(_) => None,
        }
    }

    /// The items of a list, with its name first. Empty for an atom.
    pub(crate) fn items(&self) -> &[Sexp<'a>] {
        match self {
            Sexp::List(items) => items,
            Sexp::Atom(_) => &[],
        }
    }

    /// The name of a list: its first item, if that is an atom.
    pub(crate) fn name(&self) -> Option<&str> {
        self.items().first().and_then(Sexp::as_atom)
    }

    /// Whether this is a list with that name.
    pub(crate) fn is(&self, name: &str) -> bool {
        self.name() == Some(name)
    }

    /// The items after the name.
    pub(crate) fn args(&self) -> &[Sexp<'a>] {
        self.items().get(1..).unwrap_or(&[])
    }

    /// The `i`th item after the name, if it is an atom.
    pub(crate) fn arg(&self, i: usize) -> Option<&str> {
        self.args().get(i).and_then(Sexp::as_atom)
    }

    /// The lists among the items, with the names given.
    pub(crate) fn children<'s>(&'s self, name: &'s str) -> impl Iterator<Item = &'s Sexp<'a>> {
        self.args().iter().filter(move |s| s.is(name))
    }

    /// The first list among the items with that name.
    pub(crate) fn child(&self, name: &str) -> Option<&Sexp<'a>> {
        self.args().iter().find(|s| s.is(name))
    }

    /// Whether the list has a flag: a bare atom `name`, a list `(name)`, or a list
    /// `(name yes)`. KiCad has written flags each of these ways over the years. `(name no)`
    /// is a flag that is not set.
    pub(crate) fn flag(&self, name: &str) -> bool {
        self.args().iter().any(|s| match s {
            Sexp::Atom(text) => text == name,
            Sexp::List(_) => s.is(name) && !matches!(s.arg(0), Some("no" | "false")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists_atoms_and_strings() {
        let s = parse("(a 1 -2.5 \"x y\" (b) (c \"q\\\"\\\\\\n\"))\n").unwrap();
        assert_eq!(s.name(), Some("a"));
        assert_eq!(s.arg(0), Some("1"));
        assert_eq!(s.arg(1), Some("-2.5"));
        assert_eq!(s.arg(2), Some("x y"));
        assert!(s.child("b").unwrap().args().is_empty());
        assert_eq!(s.child("c").unwrap().arg(0), Some("q\"\\\n"));
    }

    #[test]
    fn handles_crlf_and_unicode() {
        let s = parse("(a\r\n  \"Ω µ\"\r\n  (b é))\r\n").unwrap();
        assert_eq!(s.arg(0), Some("Ω µ"));
        assert_eq!(s.child("b").unwrap().arg(0), Some("é"));
    }

    #[test]
    fn flags_in_every_spelling() {
        let s = parse("(pad locked (a) (b yes) (c no) (d \"x\"))").unwrap();
        assert!(s.flag("locked") && s.flag("a") && s.flag("b") && s.flag("d"));
        assert!(!s.flag("c") && !s.flag("e"));
    }

    #[test]
    fn rejects_broken_files() {
        for (text, line) in [
            ("(a (b)", 1),
            ("(a\n\"open)", 2),
            ("x", 1),
            ("(a) (b)", 1),
            ("", 1),
        ] {
            let error = parse(text).unwrap_err();
            assert_eq!(error.line(), Some(line), "{text:?}: {error}");
        }
        let deep = "(".repeat(MAX_DEPTH + 1) + &")".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep).is_err());
        let ok = "(".repeat(MAX_DEPTH) + &")".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_ok());
    }
}
