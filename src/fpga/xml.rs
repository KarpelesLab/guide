//! A small streaming XML pull parser, covering only what VTR's file formats use.
//!
//! Real device graphs reach gigabytes, so nothing here builds a tree: the parser hands out
//! one event at a time and keeps only the current token and the names of the open elements.
//! Its memory is therefore bounded by [`Limits`], whatever the size of the input, and every
//! bound is also what stops a hostile file from exhausting memory.
//!
//! # What is supported
//!
//! Elements, attributes, comments, processing instructions, the XML declaration, a
//! `DOCTYPE` (skipped), the five predefined entities and numeric character references.
//!
//! # What is deliberately not
//!
//! - **Entities declared in a `DOCTYPE`.** They are never expanded, and using one is an
//!   error, so entity expansion attacks don't apply.
//! - **`CDATA` sections.** No VTR tool writes them, so one is an error rather than something
//!   to get subtly wrong.
//! - **Encodings other than UTF-8**, which includes plain ASCII. Anything else fails as
//!   invalid UTF-8.
//! - **Namespaces.** A prefixed name is just a name.
//!
//! It is lenient in one way: `--` inside a comment is accepted, as VTR's own parser does.

use std::io::{BufRead, ErrorKind};

use super::{ImportError, XmlErrorKind};

/// How much of the input the parser will hold or follow at once.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// The most bytes in one tag, between its `<` and `>`.
    pub tag_bytes: usize,
    /// The most bytes of text between two tags.
    pub text_bytes: usize,
    /// The most elements open at once.
    pub depth: usize,
    /// The most attributes on one element, which also bounds the duplicate check.
    pub attributes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            tag_bytes: 1 << 20,
            // A packed netlist lists every circuit input in one text node, and a large
            // design has millions of them, so text has to get far more room than tags.
            text_bytes: 256 << 20,
            depth: 256,
            attributes: 256,
        }
    }
}

/// Where an attribute's value is.
#[derive(Clone, Copy, Debug)]
enum ValueAt {
    /// In the tag as written, which is the case unless the value has entities.
    Raw,
    /// In the buffer of decoded values.
    Decoded,
}

/// One attribute of the current tag, as offsets so that no allocation is needed per tag.
#[derive(Clone, Copy, Debug)]
struct AttrSpan {
    name: (usize, usize),
    value: (usize, usize),
    value_at: ValueAt,
}

/// A start tag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tag<'a> {
    source: &'a str,
    decoded: &'a str,
    name_end: usize,
    attrs: &'a [AttrSpan],
    line: u64,
}

impl<'a> Tag<'a> {
    /// Returns the element's name.
    #[inline]
    pub fn name(&self) -> &'a str {
        self.source.get(..self.name_end).unwrap_or_default()
    }

    /// Returns the line the tag starts on.
    #[inline]
    pub fn line(&self) -> u64 {
        self.line
    }

    /// Returns the value of an attribute, with entities decoded.
    #[inline]
    pub fn attr(&self, name: &str) -> Option<&'a str> {
        self.attrs.iter().find_map(|span| {
            (self.source.get(span.name.0..span.name.1) == Some(name)).then(|| self.value(span))
        })
    }

    /// Returns every attribute as a name and value, in document order.
    #[cfg(test)]
    pub fn attrs(&self) -> impl Iterator<Item = (&'a str, &'a str)> {
        let source = self.source;
        let this = *self;
        self.attrs.iter().map(move |span| {
            (
                source.get(span.name.0..span.name.1).unwrap_or_default(),
                this.value(span),
            )
        })
    }

    fn value(&self, span: &AttrSpan) -> &'a str {
        let text = match span.value_at {
            ValueAt::Raw => self.source,
            ValueAt::Decoded => self.decoded,
        };
        // Spans are built at ASCII delimiters of validated text, so this can't miss.
        text.get(span.value.0..span.value.1).unwrap_or_default()
    }
}

/// One step through a document.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Event<'a> {
    /// An element opened. An element written as `<a/>` also produces an [`End`](Event::End).
    Start(Tag<'a>),
    /// The most recently opened element closed. Its name matched, or this is an error.
    End,
    /// Text inside an element, with entities decoded. Whitespace before it is left out, so
    /// text that is only whitespace isn't reported, and comments inside it don't split it.
    Text(&'a str),
}

/// A pull parser over a [`BufRead`].
///
/// [`next_event`](XmlReader::next_event) returns `Ok(None)` once, at the end of a complete
/// document, after checking that it was one. Malformed input gives an error, and the state
/// after an error is unspecified but still safe to use.
pub(crate) struct XmlReader<R> {
    reader: R,
    limits: Limits,
    /// The line that the next unread byte is on, counting from 1.
    line: u64,
    /// The line of the last tag, for errors found while taking it apart.
    tag_line: u64,

    tag: Vec<u8>,
    attrs: Vec<AttrSpan>,
    decoded_attrs: String,
    text: Vec<u8>,
    decoded_text: String,

    /// The names of the open elements, one after another, ended at `open_ends`. A single
    /// allocation, as elements open and close far too often to allocate per element.
    open_names: Vec<u8>,
    open_ends: Vec<usize>,

    started: bool,
    seen_root: bool,
    /// The last tag was `<a/>`, so an end event is owed.
    pending_end: bool,
    /// The `<` of a tag has been consumed, and text was reported before reading the tag.
    in_tag: bool,
}

/// Whether a byte is XML whitespace.
#[inline]
fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\t' | b'\r')
}

#[inline]
fn count_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|&&byte| byte == b'\n').count() as u64
}

/// Whether `name` can name an element or attribute. This rejects anything that would make
/// the document mean something else, and leaves the rest of the rules for names alone.
fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if first.is_ascii_digit() || first == b'-' || first == b'.' {
        return false;
    }
    bytes.iter().all(|&byte| {
        !(byte.is_ascii_control()
            || byte == b' '
            || matches!(
                byte,
                b'<' | b'>' | b'&' | b'"' | b'\'' | b'=' | b'/' | b'!' | b'?' | b';' | b','
            ))
    })
}

/// Appends `raw`, which contains at least one entity, to `out` with its entities decoded.
fn decode_entities(raw: &str, out: &mut String) -> Result<(), XmlErrorKind> {
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let invalid = || XmlErrorKind::InvalidEntity(after.chars().take(16).collect());
        // The longest valid reference is `&#x10FFFF;` or `&#1114111;`, plus leading zeros
        // that nothing writes, so a short look is enough, and this can't scan far.
        let window = &after.as_bytes()[..after.len().min(12)];
        let semicolon = window
            .iter()
            .position(|&byte| byte == b';')
            .ok_or_else(invalid)?;
        let body = &after[..semicolon];
        let decoded = match body {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = body.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(decimal) = body.strip_prefix('#') {
                    decimal.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32)
                    .filter(|&c| c != '\0')
                    .ok_or_else(invalid)?
            }
        };
        out.push(decoded);
        rest = &after[semicolon + 1..];
    }
    out.push_str(rest);
    Ok(())
}

/// What [`parse_tag`] found out, besides the attributes it wrote.
struct ParsedTag {
    name_end: usize,
    self_closing: bool,
}

/// Takes a start tag apart. On failure, gives the reason and the offset to report it at.
fn parse_tag(
    source: &str,
    attrs: &mut Vec<AttrSpan>,
    decoded: &mut String,
    limit: usize,
) -> Result<ParsedTag, (XmlErrorKind, usize)> {
    attrs.clear();
    decoded.clear();

    let bytes = source.as_bytes();
    // The tag was scanned with quotes in mind, so a final `/` is outside of any value.
    let (end, self_closing) = match bytes.last() {
        Some(b'/') => (bytes.len() - 1, true),
        _ => (bytes.len(), false),
    };

    let name_end = bytes[..end]
        .iter()
        .position(|&byte| is_space(byte))
        .unwrap_or(end);
    if !is_valid_name(&source[..name_end]) {
        let name = source[..name_end].chars().take(32).collect();
        return Err((XmlErrorKind::InvalidName(name), 0));
    }

    let mut at = name_end;
    loop {
        let before_space = at;
        while at < end && is_space(bytes[at]) {
            at += 1;
        }
        if at == end {
            break;
        }
        if at == before_space {
            return Err((XmlErrorKind::MissingSpace, at));
        }

        let name_start = at;
        while at < end && !is_space(bytes[at]) && bytes[at] != b'=' {
            at += 1;
        }
        let name = &source[name_start..at];
        if !is_valid_name(name) {
            let name = name.chars().take(32).collect();
            return Err((XmlErrorKind::InvalidName(name), name_start));
        }
        let attr_name = (name_start, at);

        while at < end && is_space(bytes[at]) {
            at += 1;
        }
        if at == end || bytes[at] != b'=' {
            return Err((XmlErrorKind::MissingAttributeValue, name_start));
        }
        at += 1;
        while at < end && is_space(bytes[at]) {
            at += 1;
        }
        let quote = match bytes.get(at) {
            Some(&quote @ (b'"' | b'\'')) if at < end => quote,
            _ => return Err((XmlErrorKind::MissingAttributeValue, name_start)),
        };
        at += 1;
        let value_start = at;
        let Some(length) = bytes[at..end].iter().position(|&byte| byte == quote) else {
            return Err((XmlErrorKind::MissingAttributeValue, name_start));
        };
        let value_end = at + length;
        at = value_end + 1;

        let raw = &source[value_start..value_end];
        if raw.contains('<') {
            return Err((XmlErrorKind::LessThanInAttribute, value_start));
        }
        let span = if raw.contains('&') {
            let start = decoded.len();
            decode_entities(raw, decoded).map_err(|kind| (kind, value_start))?;
            AttrSpan {
                name: attr_name,
                value: (start, decoded.len()),
                value_at: ValueAt::Decoded,
            }
        } else {
            AttrSpan {
                name: attr_name,
                value: (value_start, value_end),
                value_at: ValueAt::Raw,
            }
        };

        if attrs.len() == limit {
            return Err((XmlErrorKind::TooManyAttributes { limit }, name_start));
        }
        if attrs
            .iter()
            .any(|other| source[other.name.0..other.name.1] == *name)
        {
            return Err((
                XmlErrorKind::DuplicateAttribute(name.chars().take(32).collect()),
                name_start,
            ));
        }
        attrs.push(span);
    }

    Ok(ParsedTag {
        name_end,
        self_closing,
    })
}

impl<R: BufRead> XmlReader<R> {
    /// Starts reading a document with the default limits.
    pub fn new(reader: R) -> XmlReader<R> {
        XmlReader::with_limits(reader, Limits::default())
    }

    /// Starts reading a document with the given limits.
    pub fn with_limits(reader: R, limits: Limits) -> XmlReader<R> {
        XmlReader {
            reader,
            limits,
            line: 1,
            tag_line: 1,
            tag: Vec::new(),
            attrs: Vec::new(),
            decoded_attrs: String::new(),
            text: Vec::new(),
            decoded_text: String::new(),
            open_names: Vec::new(),
            open_ends: Vec::new(),
            started: false,
            seen_root: false,
            pending_end: false,
            in_tag: false,
        }
    }

    /// Returns the line the next unread byte is on.
    #[inline]
    pub fn line(&self) -> u64 {
        self.line
    }

    /// Returns the bytes held in buffers, to show that they stay within the limits.
    #[cfg(test)]
    pub fn buffered_bytes(&self) -> usize {
        self.tag.capacity()
            + self.decoded_attrs.capacity()
            + self.text.capacity()
            + self.decoded_text.capacity()
            + self.open_names.capacity()
    }

    fn error(&self, kind: XmlErrorKind) -> ImportError {
        ImportError::Xml {
            line: self.line,
            kind,
        }
    }

    /// Consumes the input up to and including a `>` that follows `run` bytes of `run_byte`.
    fn skip_past(
        &mut self,
        run_byte: u8,
        run: u32,
        expected: &'static str,
    ) -> Result<(), ImportError> {
        let mut seen = 0;
        loop {
            let available = fill(&mut self.reader, self.line)?;
            if available.is_empty() {
                return Err(self.error(XmlErrorKind::UnexpectedEof { expected }));
            }
            let mut end = None;
            for (at, &byte) in available.iter().enumerate() {
                if byte == run_byte {
                    seen += 1;
                } else {
                    if byte == b'>' && seen >= run {
                        end = Some(at + 1);
                        break;
                    }
                    seen = 0;
                }
            }
            let used = end.unwrap_or(available.len());
            self.line += count_newlines(&available[..used]);
            self.reader.consume(used);
            if end.is_some() {
                return Ok(());
            }
        }
    }

    /// Returns the next byte, consuming it, or `None` at the end of the input.
    fn next_byte(&mut self) -> Result<Option<u8>, ImportError> {
        let available = fill(&mut self.reader, self.line)?;
        let Some(&byte) = available.first() else {
            return Ok(None);
        };
        if byte == b'\n' {
            self.line += 1;
        }
        self.reader.consume(1);
        Ok(Some(byte))
    }

    /// Returns the next byte without consuming it, or `None` at the end of the input.
    fn peek_byte(&mut self) -> Result<Option<u8>, ImportError> {
        Ok(fill(&mut self.reader, self.line)?.first().copied())
    }

    /// Skips a `DOCTYPE`, once `<!DOCTYPE` has been read, including any internal subset.
    ///
    /// Its declarations are ignored, so entities declared there stay unknown.
    fn skip_doctype(&mut self) -> Result<(), ImportError> {
        const EXPECTED: &str = "the end of a DOCTYPE";
        // How far through `<!--` or `-->` the bytes have got. Comments can hold quotes and
        // brackets that mean nothing, so they have to be recognized.
        let mut comment_open = 0;
        let mut dashes = 0;
        let mut in_comment = false;
        let mut quote = None;
        let mut depth = 0u64;
        loop {
            let Some(byte) = self.next_byte()? else {
                return Err(self.error(XmlErrorKind::UnexpectedEof { expected: EXPECTED }));
            };
            if in_comment {
                match byte {
                    b'-' => dashes += 1,
                    b'>' if dashes >= 2 => {
                        in_comment = false;
                        dashes = 0;
                    }
                    _ => dashes = 0,
                }
                continue;
            }
            if let Some(open) = quote {
                if byte == open {
                    quote = None;
                }
                continue;
            }
            comment_open = match (comment_open, byte) {
                (0, b'<') => 1,
                (1, b'!') => 2,
                (2, b'-') => 3,
                (3, b'-') => {
                    in_comment = true;
                    0
                }
                _ => 0,
            };
            match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'[' => depth += 1,
                b']' => depth = depth.saturating_sub(1),
                b'>' if depth == 0 => return Ok(()),
                _ => {}
            }
        }
    }

    /// Reads up to and including the `>` that ends a tag into `self.tag`, without the `>`.
    fn read_tag(&mut self) -> Result<(), ImportError> {
        self.tag.clear();
        self.tag_line = self.line;
        let mut quote = 0u8;
        loop {
            let available = fill(&mut self.reader, self.line)?;
            if available.is_empty() {
                return Err(self.error(XmlErrorKind::UnexpectedEof {
                    expected: "the end of a tag",
                }));
            }
            let mut end = None;
            for (at, &byte) in available.iter().enumerate() {
                if quote != 0 {
                    if byte == quote {
                        quote = 0;
                    }
                } else if byte == b'>' {
                    end = Some(at);
                    break;
                } else if byte == b'"' || byte == b'\'' {
                    quote = byte;
                }
            }
            let copied = end.unwrap_or(available.len());
            if self.tag.len() + copied > self.limits.tag_bytes {
                let limit = self.limits.tag_bytes;
                return Err(ImportError::Xml {
                    line: self.tag_line,
                    kind: XmlErrorKind::TagTooLong { limit },
                });
            }
            self.tag.extend_from_slice(&available[..copied]);
            let used = end.map_or(copied, |at| at + 1);
            self.line += count_newlines(&available[..used]);
            self.reader.consume(used);
            if end.is_some() {
                return Ok(());
            }
        }
    }

    /// Handles an end tag, whose `</` has been consumed.
    fn read_end_tag(&mut self) -> Result<(), ImportError> {
        self.read_tag()?;
        let line = self.tag_line;
        let name_len = self
            .tag
            .iter()
            .rposition(|&byte| !is_space(byte))
            .map_or(0, |at| at + 1);
        let name = &self.tag[..name_len];

        let Some(&end) = self.open_ends.last() else {
            return Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::UnexpectedEndTag(lossy_prefix(name)),
            });
        };
        let start = self
            .open_ends
            .len()
            .checked_sub(2)
            .map_or(0, |at| self.open_ends[at]);
        if self.open_names[start..end] != *name {
            return Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::MismatchedEndTag {
                    expected: lossy_prefix(&self.open_names[start..end]),
                    found: lossy_prefix(name),
                },
            });
        }
        self.open_ends.pop();
        self.open_names.truncate(start);
        Ok(())
    }

    /// Handles a start tag, whose `<` has been consumed.
    fn read_start_tag(&mut self) -> Result<Event<'_>, ImportError> {
        self.read_tag()?;
        let line = self.tag_line;
        if self.seen_root && self.open_ends.is_empty() {
            return Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::MultipleRoots,
            });
        }
        let Ok(source) = core::str::from_utf8(&self.tag) else {
            return Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::InvalidUtf8,
            });
        };
        let parsed = match parse_tag(
            source,
            &mut self.attrs,
            &mut self.decoded_attrs,
            self.limits.attributes,
        ) {
            Ok(parsed) => parsed,
            Err((kind, offset)) => {
                let offset = offset.min(self.tag.len());
                return Err(ImportError::Xml {
                    line: line + count_newlines(&self.tag[..offset]),
                    kind,
                });
            }
        };
        if self.open_ends.len() == self.limits.depth {
            let limit = self.limits.depth;
            return Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::TooDeep { limit },
            });
        }
        self.seen_root = true;
        self.open_names
            .extend_from_slice(&source.as_bytes()[..parsed.name_end]);
        self.open_ends.push(self.open_names.len());
        self.pending_end = parsed.self_closing;
        Ok(Event::Start(Tag {
            source,
            decoded: &self.decoded_attrs,
            name_end: parsed.name_end,
            attrs: &self.attrs,
            line,
        }))
    }

    /// Consumes a byte order mark, which is only valid at the very start of a document.
    fn skip_byte_order_mark(&mut self) -> Result<(), ImportError> {
        for (at, expected) in [0xEF, 0xBB, 0xBF].into_iter().enumerate() {
            match self.peek_byte()? {
                Some(byte) if byte == expected => {
                    self.next_byte()?;
                }
                _ if at == 0 => return Ok(()),
                _ => return Err(self.error(XmlErrorKind::InvalidUtf8)),
            }
        }
        Ok(())
    }

    /// Appends the text up to the next `<` to `self.text`, leaving out whitespace before
    /// any other text. Returns whether a `<` was reached, in which case it was consumed.
    fn read_text(&mut self) -> Result<bool, ImportError> {
        loop {
            let available = fill(&mut self.reader, self.line)?;
            if available.is_empty() {
                return Ok(false);
            }
            let stop = available.iter().position(|&byte| byte == b'<');
            let piece = &available[..stop.unwrap_or(available.len())];
            // Almost all text in a device graph is indentation, so it is worth not copying.
            let kept = if self.text.is_empty() {
                let leading = piece.iter().take_while(|&&byte| is_space(byte)).count();
                &piece[leading..]
            } else {
                piece
            };
            if self.text.len() + kept.len() > self.limits.text_bytes {
                let limit = self.limits.text_bytes;
                return Err(self.error(XmlErrorKind::TextTooLong { limit }));
            }
            self.text.extend_from_slice(kept);
            let used = piece.len() + usize::from(stop.is_some());
            self.line += count_newlines(piece);
            self.reader.consume(used);
            if stop.is_some() {
                return Ok(true);
            }
        }
    }

    /// Returns `self.text`, which isn't empty, as an event.
    fn take_text(&mut self) -> Result<Option<Event<'_>>, ImportError> {
        if self.open_ends.is_empty() {
            return Err(self.error(XmlErrorKind::TextOutsideRoot));
        }
        let Ok(text) = core::str::from_utf8(&self.text) else {
            return Err(ImportError::Xml {
                line: self.line,
                kind: XmlErrorKind::InvalidUtf8,
            });
        };
        if text.contains('&') {
            self.decoded_text.clear();
            if let Err(kind) = decode_entities(text, &mut self.decoded_text) {
                return Err(ImportError::Xml {
                    line: self.line,
                    kind,
                });
            }
            Ok(Some(Event::Text(&self.decoded_text)))
        } else {
            Ok(Some(Event::Text(text)))
        }
    }

    /// Returns the next event, or `None` at the end of a complete document.
    pub fn next_event(&mut self) -> Result<Option<Event<'_>>, ImportError> {
        if !self.started {
            self.started = true;
            self.skip_byte_order_mark()?;
        }
        if self.pending_end {
            self.pending_end = false;
            let start = self
                .open_ends
                .len()
                .checked_sub(2)
                .map_or(0, |at| self.open_ends[at]);
            self.open_ends.pop();
            self.open_names.truncate(start);
            return Ok(Some(Event::End));
        }

        if !self.in_tag {
            self.text.clear();
        }
        loop {
            if !self.in_tag && !self.read_text()? {
                // The end of the input, which is only fine outside of the root element.
                if let Some(&end) = self.open_ends.last() {
                    let start = self
                        .open_ends
                        .len()
                        .checked_sub(2)
                        .map_or(0, |at| self.open_ends[at]);
                    let element = lossy_prefix(&self.open_names[start..end]);
                    return Err(self.error(XmlErrorKind::UnclosedElement(element)));
                }
                if !self.seen_root {
                    return Err(self.error(XmlErrorKind::NoRootElement));
                }
                if !self.text.is_empty() {
                    return Err(self.error(XmlErrorKind::TextOutsideRoot));
                }
                return Ok(None);
            }

            let Some(first) = self.peek_byte()? else {
                return Err(self.error(XmlErrorKind::UnexpectedEof {
                    expected: "a tag after `<`",
                }));
            };
            match first {
                b'?' => {
                    self.next_byte()?;
                    self.skip_past(b'?', 1, "the end of a processing instruction")?;
                }
                b'!' => {
                    self.next_byte()?;
                    self.read_declaration()?;
                }
                _ => {
                    // Text ends at a tag, so it has to go out before the tag can.
                    if !self.in_tag && !self.text.is_empty() {
                        self.in_tag = true;
                        return self.take_text();
                    }
                    self.in_tag = false;
                    if first == b'/' {
                        self.next_byte()?;
                        self.read_end_tag()?;
                        return Ok(Some(Event::End));
                    }
                    return self.read_start_tag().map(Some);
                }
            }
        }
    }

    /// Handles what follows `<!`.
    fn read_declaration(&mut self) -> Result<(), ImportError> {
        let line = self.line;
        match self.next_byte()? {
            Some(b'-') => {
                if self.next_byte()? == Some(b'-') {
                    self.skip_past(b'-', 2, "the end of a comment")
                } else {
                    Err(self.error(XmlErrorKind::UnsupportedMarkup("`<!-`")))
                }
            }
            Some(b'[') => Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::UnsupportedMarkup("CDATA"),
            }),
            Some(b'D') => {
                for expected in *b"OCTYPE" {
                    if self.next_byte()? != Some(expected) {
                        return Err(ImportError::Xml {
                            line,
                            kind: XmlErrorKind::UnsupportedMarkup("`<!D`"),
                        });
                    }
                }
                if self.seen_root {
                    return Err(ImportError::Xml {
                        line,
                        kind: XmlErrorKind::UnsupportedMarkup("DOCTYPE after the root element"),
                    });
                }
                self.skip_doctype()
            }
            None => Err(self.error(XmlErrorKind::UnexpectedEof {
                expected: "a declaration after `<!`",
            })),
            Some(_) => Err(ImportError::Xml {
                line,
                kind: XmlErrorKind::UnsupportedMarkup("unknown `<!` declaration"),
            }),
        }
    }
}

/// Returns more input from `reader`, which is empty only at the end of the input.
fn fill<R: BufRead>(reader: &mut R, line: u64) -> Result<&[u8], ImportError> {
    loop {
        // Asked twice because a borrow returned from a loop would keep `reader` borrowed
        // through the retry.
        match reader.fill_buf() {
            Ok(_) => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(source) => return Err(ImportError::Io { line, source }),
        }
    }
    reader
        .fill_buf()
        .map_err(|source| ImportError::Io { line, source })
}

/// Makes a short printable name from bytes that may not be valid text.
fn lossy_prefix(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(64)]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::{BufReader, Cursor, Read};

    /// An event with everything owned, so that tests can compare whole documents.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Owned {
        Start(String, Vec<(String, String)>),
        End,
        Text(String),
    }

    fn walk<R: BufRead>(reader: R, limits: Limits) -> Result<Vec<Owned>, ImportError> {
        let mut parser = XmlReader::with_limits(reader, limits);
        let mut out = Vec::new();
        while let Some(event) = parser.next_event()? {
            out.push(match event {
                Event::Start(tag) => Owned::Start(
                    tag.name().to_owned(),
                    tag.attrs()
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect(),
                ),
                Event::End => Owned::End,
                Event::Text(text) => Owned::Text(text.to_owned()),
            });
        }
        Ok(out)
    }

    fn parse(text: &str) -> Result<Vec<Owned>, ImportError> {
        walk(Cursor::new(text), Limits::default())
    }

    fn start(name: &str, attrs: &[(&str, &str)]) -> Owned {
        Owned::Start(
            name.to_owned(),
            attrs
                .iter()
                .map(|&(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        )
    }

    fn text(value: &str) -> Owned {
        Owned::Text(value.to_owned())
    }

    /// The line and reason of a failure, which the tests below have to pin.
    fn failure(input: &str) -> (u64, XmlErrorKind) {
        match parse(input) {
            Err(ImportError::Xml { line, kind }) => (line, kind),
            other => panic!("expected an XML error for {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn reads_elements_attributes_and_text() {
        let events = parse("<a x=\"1\" y='two'>hi<b/>there<c></c></a>").unwrap();
        assert_eq!(
            events,
            [
                start("a", &[("x", "1"), ("y", "two")]),
                text("hi"),
                start("b", &[]),
                Owned::End,
                text("there"),
                start("c", &[]),
                Owned::End,
                Owned::End,
            ]
        );
    }

    #[test]
    fn skips_declarations_comments_and_blank_text() {
        let input = "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <!-- before -- with dashes --->\n\
            <!DOCTYPE a [ <!ENTITY e \"x>y\"> <!-- ]> --> ] >\n\
            <?pi anything > goes?>\n\
            <a>\n  <!-- inside -->\n  <b/>\n</a>\n<!-- after -->\n<?end?>\n";
        assert_eq!(
            parse(input).unwrap(),
            [start("a", &[]), start("b", &[]), Owned::End, Owned::End]
        );
    }

    #[test]
    fn comments_do_not_split_text() {
        assert_eq!(
            parse("<a>one<!-- x --> <!-- y -->two</a>").unwrap(),
            [start("a", &[]), text("one two"), Owned::End]
        );
    }

    #[test]
    fn decodes_entities_in_text_and_attributes() {
        let events = parse("<a v=\"&lt;&gt;&amp;&quot;&apos;&#65;&#x42;&#x20AC;\">x&gt;y</a>");
        assert_eq!(
            events.unwrap(),
            [
                start("a", &[("v", "<>&\"'AB\u{20AC}")]),
                text("x>y"),
                Owned::End
            ]
        );
    }

    #[test]
    fn netlist_interconnect_names_decode() {
        // How a real packed netlist writes `->` in text.
        let events = parse("<port name=\"O\">fle[1].out[0]-&gt;clbouts1 open</port>");
        assert_eq!(events.unwrap()[1], text("fle[1].out[0]->clbouts1 open"),);
    }

    #[test]
    fn greater_than_in_values_and_slashes_in_names_of_values_are_fine() {
        assert_eq!(
            parse("<a v=\"1>2/\"/>").unwrap(),
            [start("a", &[("v", "1>2/")]), Owned::End]
        );
    }

    #[test]
    fn spaces_around_equals_and_in_end_tags_are_allowed() {
        assert_eq!(
            parse("<a\n  v = '1' ></a  >").unwrap(),
            [start("a", &[("v", "1")]), Owned::End]
        );
    }

    #[test]
    fn unicode_names_and_values_work() {
        assert_eq!(
            parse("<é ü='ñ'>日本語</é>").unwrap(),
            [start("é", &[("ü", "ñ")]), text("日本語"), Owned::End]
        );
    }

    #[test]
    fn errors_carry_the_line_they_happened_on() {
        let cases: [(&str, u64); 8] = [
            ("<a>\n<b>\n</c>\n</a>", 3),
            ("\n\n<a", 3),
            ("<a>\n\n", 3),
            ("<a\n  x='1'\n  x='2'/>", 3),
            ("<a\n  x='&nope;'/>", 2),
            ("<a>\n\n&nope;</a>", 3),
            ("<a/>\n<b/>", 2),
            ("\n</a>", 2),
        ];
        for (input, line) in cases {
            assert_eq!(failure(input).0, line, "for {input:?}");
        }
    }

    #[test]
    fn rejects_malformed_documents() {
        for input in [
            "",
            "   ",
            "text",
            "<",
            "<>",
            "</a>",
            "<a",
            "<a>",
            "<a></b>",
            "<a/><b/>",
            "<a/>text",
            "text<a/>",
            "<a x/>",
            "<a x=/>",
            "<a x=1/>",
            "<a x='1/>",
            "<a x='1'y='2'/>",
            "<a x='<'/>",
            "<a x='1' x='2'/>",
            "<1a/>",
            "< a/>",
            "<a b=c>",
            "<a>&;</a>",
            "<a>&#0;</a>",
            "<a>&#xD800;</a>",
            "<a>&#x110000;</a>",
            "<a>&unknown;</a>",
            "<a>&amp</a>",
            "<a><![CDATA[x]]></a>",
            "<a><!ELEMENT></a>",
            "<a><!-- unterminated</a>",
            "<?xml",
            "<!DOCTYPE a",
            "<a/><!DOCTYPE a>",
            "\u{FEFF}\u{FEFF}<a/>",
        ] {
            assert!(parse(input).is_err(), "should reject {input:?}");
        }
    }

    #[test]
    fn declared_entities_are_not_expanded() {
        let input = "<!DOCTYPE a [<!ENTITY lol \"lol\">]><a>&lol;</a>";
        assert!(matches!(failure(input).1, XmlErrorKind::InvalidEntity(_)));
    }

    #[test]
    fn rejects_invalid_utf8() {
        for input in [
            &b"<a x='\xFF'/>"[..],
            &b"<a>\xFF</a>"[..],
            &b"<\xFF/>"[..],
            &b"\xEF\xBB<a/>"[..],
        ] {
            assert!(
                walk(Cursor::new(input), Limits::default()).is_err(),
                "should reject {input:?}"
            );
        }
    }

    #[test]
    fn enforces_every_limit() {
        let limits = Limits {
            tag_bytes: 16,
            text_bytes: 8,
            depth: 2,
            attributes: 2,
        };
        let check = |input: &str| walk(Cursor::new(input), limits);
        assert!(check("<a b='1'><c/></a>").is_ok());
        assert!(matches!(
            check("<a b='0123456789abcdef'/>"),
            Err(ImportError::Xml {
                kind: XmlErrorKind::TagTooLong { .. },
                ..
            })
        ));
        assert!(matches!(
            check("<a>0123456789</a>"),
            Err(ImportError::Xml {
                kind: XmlErrorKind::TextTooLong { .. },
                ..
            })
        ));
        assert!(matches!(
            check("<a><b><c/></b></a>"),
            Err(ImportError::Xml {
                kind: XmlErrorKind::TooDeep { .. },
                ..
            })
        ));
        assert!(matches!(
            check("<a b='' c='' d=''></a>"),
            Err(ImportError::Xml {
                kind: XmlErrorKind::TooManyAttributes { .. },
                ..
            })
        ));
    }

    /// Generates a huge document without holding it, to show memory doesn't follow its size.
    struct Repeating {
        head: Cursor<&'static [u8]>,
        body: &'static [u8],
        at: usize,
        remaining: u64,
        tail: Cursor<&'static [u8]>,
    }

    impl Read for Repeating {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let from_head = self.head.read(out)?;
            if from_head > 0 {
                return Ok(from_head);
            }
            if self.remaining == 0 {
                return self.tail.read(out);
            }
            let piece = &self.body[self.at..];
            let used = piece.len().min(out.len());
            out[..used].copy_from_slice(&piece[..used]);
            self.at += used;
            if self.at == self.body.len() {
                self.at = 0;
                self.remaining -= 1;
            }
            Ok(used)
        }
    }

    #[test]
    fn memory_does_not_grow_with_the_document() {
        let source = Repeating {
            head: Cursor::new(b"<rr_graph><rr_edges>\n"),
            body: b"<edge src_node=\"200\" sink_node=\"0\" switch_id=\"3\">\n<metadata>\n\
                    <meta name=\"fasm_features\"> z1000.mux_0.s[0]</meta>\n</metadata>\n</edge>\n",
            at: 0,
            remaining: 200_000,
            tail: Cursor::new(b"</rr_edges></rr_graph>\n"),
        };
        let mut parser = XmlReader::new(BufReader::new(source));
        let mut events = 0u64;
        let mut peak = 0;
        while parser.next_event().unwrap().is_some() {
            events += 1;
            peak = peak.max(parser.buffered_bytes());
        }
        assert_eq!(events, 4 + 200_000 * 7);
        assert_eq!(parser.line(), 2 + 200_000 * 5 + 1);
        assert!(peak < 4096, "held {peak} bytes");
    }

    /// A document for mutating, with something of everything in it.
    const SAMPLE: &str = "<?xml version=\"1.0\"?>\n<!-- c -->\n<!DOCTYPE a [<!ENTITY e 'x'>]>\n\
        <a p=\"1\" q='&lt;'>\n  text &amp; more\n  <b r=\"é\"/>\n  <c>&#65;</c><!-- d -->\n</a>\n";

    fn any_document() -> impl Strategy<Value = String> {
        // Whole documents, not just bytes, so that the parser gets past the first character.
        let leaf = prop_oneof![
            Just("<b/>".to_owned()),
            Just("<b x='1' y=\"&amp;\"/>".to_owned()),
            Just("text".to_owned()),
            Just(" \n\t".to_owned()),
            Just("&lt;".to_owned()),
            Just("<!-- comment -->".to_owned()),
            Just("<?pi data?>".to_owned()),
        ];
        let nested = leaf.prop_recursive(4, 32, 4, |inner| {
            proptest::collection::vec(inner, 0..4)
                .prop_map(|parts| format!("<e a='{}'>{}</e>", parts.len(), parts.concat()))
        });
        nested.prop_map(|body| format!("<?xml version=\"1.0\"?><root>{body}</root>"))
    }

    proptest! {
        /// Whatever arrives, the answer is events or an error.
        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = walk(Cursor::new(bytes), Limits::default());
        }

        /// Bytes drawn from XML's own alphabet get a lot further than arbitrary ones.
        #[test]
        fn random_markup_never_panics(
            pieces in proptest::collection::vec(
                proptest::sample::select(vec![
                    "<", ">", "/", "=", "\"", "'", "&", ";", "!", "-", "?", "[", "]", "a", "b",
                    " ", "\n", "&lt;", "&#", "x", "<a>", "</a>", "<!--", "-->", "<?", "?>",
                    "<!DOCTYPE", "<![CDATA[", "\u{FEFF}", "é",
                ]),
                0..64,
            ),
        ) {
            let _ = parse(&pieces.concat());
        }

        /// Cutting a document short is an error, unless all that's gone is trailing space.
        #[test]
        fn truncated_documents_are_errors_with_a_sensible_line(document in any_document(), cut: prop::sample::Index) {
            let end = document.trim_end().len();
            let at = cut.index(end);
            let truncated = &document.as_bytes()[..at];
            match walk(Cursor::new(truncated), Limits::default()) {
                Err(ImportError::Xml { line, .. }) => {
                    let lines = truncated.iter().filter(|&&byte| byte == b'\n').count() as u64;
                    prop_assert!(line >= 1 && line <= lines + 1);
                }
                other => prop_assert!(false, "expected an error at {}, got {:?}", at, other),
            }
        }

        /// Damaging a document at random never panics, wherever the damage lands.
        #[test]
        fn mutated_documents_never_panic(
            edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        ) {
            let mut bytes = SAMPLE.as_bytes().to_vec();
            for (at, byte) in edits {
                let at = at.index(bytes.len());
                bytes[at] = byte;
            }
            let _ = walk(Cursor::new(bytes), Limits::default());
        }

        /// Results can't depend on where the reader's buffer happens to end, as a large file
        /// will have tokens straddling every possible boundary.
        #[test]
        fn buffer_size_does_not_matter(document in any_document(), capacity in 1usize..64) {
            let whole = walk(Cursor::new(document.as_bytes()), Limits::default()).unwrap();
            let chunked = walk(
                BufReader::with_capacity(capacity, document.as_bytes()),
                Limits::default(),
            )
            .unwrap();
            prop_assert_eq!(whole, chunked);
        }

        /// Reading a document's tags back gives what was written, with values round-tripped
        /// through escaping.
        #[test]
        fn attribute_values_round_trip(value in "[ -~\u{e9}\u{20AC}]{0,40}") {
            let escaped = value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('"', "&quot;");
            let document = format!("<a v=\"{escaped}\"/>");
            let events = parse(&document).unwrap();
            prop_assert_eq!(&events[0], &start("a", &[("v", &value)]));
        }
    }
}
