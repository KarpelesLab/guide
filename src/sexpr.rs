//! A reader for S-expressions, in the two dialects that interchange formats here use:
//! KiCad's boards, and Specctra's DSN files.
//!
//! A file is one list. A list holds atoms and lists, and its first atom names it, as in
//! `(at 1.5 2 90)`. An atom is a run of characters other than whitespace and parentheses,
//! or a quoted string. Nothing is interpreted here: an atom stays text, so that numbers are
//! converted exactly where their units are known.
//!
//! The dialects differ only in strings:
//!
//! - **KiCad** quotes with `"`, and escapes quotes and backslashes with a backslash. An
//!   unquoted atom also ends at a `"`.
//! - **Specctra** has no escapes. The quote character is `"` until the file says otherwise
//!   with `(string_quote c)`, where `c` is a single character, often the quote itself, as
//!   in `(string_quote ")`. With `(space_in_quoted_tokens off)`, a space ends a quoted
//!   string as well as the quote does; it is `on` until the file says otherwise, which is
//!   kinder to files that quote names with spaces before saying so, as KiCad's do. An
//!   unquoted atom runs to whitespace or a parenthesis, quotes included.

use std::borrow::Cow;

/// A node: an atom or a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sexp<'a> {
    /// An atom, unquoted or quoted, with the escapes of a quoted one resolved.
    Atom(Cow<'a, str>),
    /// A list.
    List(Vec<Sexp<'a>>),
}

/// Why a text is not an S-expression: what, and the byte offset where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub(crate) offset: usize,
    pub(crate) message: &'static str,
}

/// The line, counted from 1, that a byte offset of a text is on.
pub(crate) fn line_of(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// The deepest nesting accepted. Files go about a dozen deep, and a limit keeps a hostile
/// file from overflowing the stack of the code that walks the tree.
const MAX_DEPTH: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    Kicad,
    Specctra {
        /// The quote character.
        quote: char,
        /// Whether a quoted string may hold spaces.
        spaces: bool,
    },
}

/// Parses a whole file in KiCad's dialect, which must be a single list, with nothing but
/// whitespace after it.
#[cfg_attr(not(feature = "kicad"), allow(dead_code))]
pub(crate) fn parse(text: &str) -> Result<Sexp<'_>, SyntaxError> {
    parse_in(text, Dialect::Kicad)
}

/// Parses a whole file in Specctra's dialect, as [`parse`] does KiCad's.
#[cfg_attr(not(feature = "dsn"), allow(dead_code))]
pub(crate) fn parse_specctra(text: &str) -> Result<Sexp<'_>, SyntaxError> {
    parse_in(
        text,
        Dialect::Specctra {
            quote: '"',
            spaces: true,
        },
    )
}

fn parse_in(text: &str, dialect: Dialect) -> Result<Sexp<'_>, SyntaxError> {
    let mut parser = Parser {
        text,
        pos: 0,
        dialect,
    };
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
    dialect: Dialect,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn error(&self, message: &'static str) -> SyntaxError {
        SyntaxError {
            offset: self.pos,
            message,
        }
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

    /// The quote character, as a byte, if it is ASCII, as it always is in practice.
    fn quote(&self) -> Option<u8> {
        match self.dialect {
            Dialect::Kicad => Some(b'"'),
            Dialect::Specctra { quote, .. } => u8::try_from(quote).ok(),
        }
    }

    /// A list, starting at its opening parenthesis.
    fn list(&mut self, depth: usize) -> Result<Sexp<'a>, SyntaxError> {
        if depth >= MAX_DEPTH {
            return Err(self.error("lists are nested too deeply"));
        }
        let start = self.pos;
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip_space();
            let c = self.peek();
            // `(string_quote c)`: whatever character comes next is the new quote.
            if let Dialect::Specctra { spaces, .. } = self.dialect
                && items.len() == 1
                && items[0] == Sexp::Atom(Cow::Borrowed("string_quote"))
                && c.is_some_and(|c| c != b')')
            {
                let quote = self.text[self.pos..].chars().next().unwrap_or('"');
                let end = self.pos + quote.len_utf8();
                items.push(Sexp::Atom(Cow::Borrowed(&self.text[self.pos..end])));
                self.pos = end;
                self.dialect = Dialect::Specctra { quote, spaces };
                continue;
            }
            match c {
                None => {
                    self.pos = start;
                    return Err(self.error("this list is never closed"));
                }
                Some(b')') => {
                    self.pos += 1;
                    if let Dialect::Specctra { quote, .. } = self.dialect
                        && items.first()
                            == Some(&Sexp::Atom(Cow::Borrowed("space_in_quoted_tokens")))
                    {
                        let on = items.get(1).and_then(Sexp::as_atom) != Some("off");
                        self.dialect = Dialect::Specctra { quote, spaces: on };
                    }
                    return Ok(Sexp::List(items));
                }
                Some(b'(') => items.push(self.list(depth + 1)?),
                Some(c) if Some(c) == self.quote() => items.push(self.quoted()?),
                Some(_) => items.push(self.bare()),
            }
        }
    }

    /// An unquoted atom, which ends at whitespace or a parenthesis, and in KiCad's dialect
    /// at a quote.
    fn bare(&mut self) -> Sexp<'a> {
        let start = self.pos;
        let kicad = self.dialect == Dialect::Kicad;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b'(' | b')') || (kicad && c == b'"') {
                break;
            }
            self.pos += 1;
        }
        Sexp::Atom(Cow::Borrowed(&self.text[start..self.pos]))
    }

    /// A quoted string.
    fn quoted(&mut self) -> Result<Sexp<'a>, SyntaxError> {
        match self.dialect {
            Dialect::Kicad => self.kicad_quoted(),
            Dialect::Specctra { quote, spaces } => self.specctra_quoted(quote, spaces),
        }
    }

    /// A string in Specctra's quotes, which has no escapes. Without spaces in quoted
    /// tokens, a space ends it too, and is taken as its closing quote.
    fn specctra_quoted(&mut self, quote: char, spaces: bool) -> Result<Sexp<'a>, SyntaxError> {
        let open = self.pos;
        self.pos += quote.len_utf8();
        let start = self.pos;
        for (i, c) in self.text[start..].char_indices() {
            if c == quote || (!spaces && c == ' ') {
                self.pos = start + i + c.len_utf8();
                return Ok(Sexp::Atom(Cow::Borrowed(&self.text[start..start + i])));
            }
        }
        self.pos = open;
        Err(self.error("this string is never closed"))
    }

    /// A string in double quotes. KiCad escapes quotes and backslashes, and writes `\n`
    /// for a line break. Any other escaped character stands for itself.
    fn kicad_quoted(&mut self) -> Result<Sexp<'a>, SyntaxError> {
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
    #[cfg_attr(not(feature = "kicad"), allow(dead_code))]
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
            assert_eq!(line_of(text, error.offset), line, "{text:?}: {error:?}");
        }
        let deep = "(".repeat(MAX_DEPTH + 1) + &")".repeat(MAX_DEPTH + 1);
        assert!(parse(&deep).is_err());
        let ok = "(".repeat(MAX_DEPTH) + &")".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn specctra_quotes_as_the_file_says() {
        // KiCad's header: the quote is `"`, given unquoted, and a quoted name follows.
        let s = parse_specctra(
            "(pcb \"a b.dsn\" (parser (string_quote \") (space_in_quoted_tokens on)) \
             (net \"x\\y\" (pins U1-1)) (via \"Via[0-1]_800:400_um\"))",
        )
        .unwrap();
        assert_eq!(s.arg(0), Some("a b.dsn"));
        let parser = s.child("parser").unwrap();
        assert_eq!(parser.child("string_quote").unwrap().arg(0), Some("\""));
        // No escapes: a backslash is a backslash.
        assert_eq!(s.child("net").unwrap().arg(0), Some("x\\y"));
        assert_eq!(s.child("via").unwrap().arg(0), Some("Via[0-1]_800:400_um"));

        // Another quote character, and quotes inside an unquoted atom.
        let s = parse_specctra("(pcb (parser (string_quote $)) (net $a \"b$ 5\"x))").unwrap();
        let net = s.child("net").unwrap();
        assert_eq!(net.arg(0), Some("a \"b"));
        assert_eq!(net.arg(1), Some("5\"x"));

        // Without spaces in quoted tokens, a space ends a quoted string.
        let s = parse_specctra("(pcb (parser (space_in_quoted_tokens off)) (net \"a b\"))");
        let s = s.unwrap();
        let net = s.child("net").unwrap();
        assert_eq!(net.arg(0), Some("a"));
        assert_eq!(net.arg(1), Some("b\""));

        assert!(parse_specctra("(pcb (net \"open))").is_err());
        assert!(parse_specctra("(pcb (parser (string_quote))))").is_err());
    }
}
