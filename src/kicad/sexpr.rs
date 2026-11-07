//! KiCad's S-expressions, through the crate's shared reader.

use super::KicadError;

pub(crate) use crate::sexpr::Sexp;

/// Parses a whole file, which must be a single list, with nothing but whitespace after it.
pub(crate) fn parse(text: &str) -> Result<Sexp<'_>, KicadError> {
    crate::sexpr::parse(text).map_err(|e| KicadError::syntax(text, e.offset, e.message))
}
