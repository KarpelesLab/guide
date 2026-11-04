//! Placements: VPR's `.place` format.
//!
//! The format is lines of text, as `read_place.cpp` in VPR parses them: an optional header
//! naming the netlist, the size of the device, and then a line for each block, as
//! `name x y subblk [layer] [#comment]`. A `#` that starts a token begins a comment.

use std::collections::HashMap;
use std::io::{BufRead, ErrorKind};

use super::ImportError;

/// The longest line accepted. Well over any real line, which has one short name on it, and
/// this is the only memory that the reader's scanning holds on to.
const MAX_LINE_BYTES: usize = 1 << 20;

/// Where a block is placed. Fixed I/O pads are placed the same way as anything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedBlock {
    /// The block's name in the netlist.
    pub name: String,
    /// The column of the tile.
    pub x: u32,
    /// The row of the tile.
    pub y: u32,
    /// The position within the tile, for tiles that hold several blocks.
    pub sub_block: u32,
    /// The die, on a 3D device.
    pub layer: u32,
}

/// A placement read from a `.place` file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    netlist_file: Option<String>,
    netlist_id: Option<String>,
    array_size: Option<(u32, u32)>,
    blocks: Vec<PlacedBlock>,
    by_name: HashMap<String, u32>,
}

impl Placement {
    /// Returns the netlist file that the header names, if there's a header.
    pub fn netlist_file(&self) -> Option<&str> {
        self.netlist_file.as_deref()
    }

    /// Returns the digest of the netlist that the header records, if there's one. VPR writes
    /// this as `SHA256:` and the hex of the digest of the `.net` file's bytes.
    pub fn netlist_id(&self) -> Option<&str> {
        self.netlist_id.as_deref()
    }

    /// Returns the size of the device that the header gives, if it gives one, as width and
    /// height.
    pub fn array_size(&self) -> Option<(u32, u32)> {
        self.array_size
    }

    /// Returns the blocks, in the order of the file.
    pub fn blocks(&self) -> &[PlacedBlock] {
        &self.blocks
    }

    /// Finds a block by name.
    pub fn block(&self, name: &str) -> Option<&PlacedBlock> {
        let &at = self.by_name.get(name)?;
        self.blocks.get(at as usize)
    }
}

/// Reads up to the next newline into `line`, without the newline, or returns `false` at the
/// end of the input.
///
/// This stops at a limit where [`BufRead::read_line`] wouldn't, as an untrusted file doesn't
/// have to have any newlines in it at all.
pub(crate) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
    line_number: u64,
) -> Result<bool, ImportError> {
    line.clear();
    let mut any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(ImportError::Io {
                    line: line_number,
                    source,
                });
            }
        };
        if available.is_empty() {
            return Ok(any);
        }
        any = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let piece = &available[..newline.unwrap_or(available.len())];
        if line.len() + piece.len() > MAX_LINE_BYTES {
            return Err(ImportError::LineTooLong {
                line: line_number,
                limit: MAX_LINE_BYTES,
            });
        }
        line.extend_from_slice(piece);
        let used = piece.len() + usize::from(newline.is_some());
        reader.consume(used);
        if newline.is_some() {
            return Ok(true);
        }
    }
}

fn parse_field(line: u64, what: &'static str, value: &str) -> Result<u32, ImportError> {
    value.parse().map_err(|_| ImportError::InvalidValue {
        line,
        what,
        value: value.chars().take(64).collect(),
    })
}

/// Reads a placement.
///
/// Lines can be in any order, and blocks need not be in the netlist. A placement that leaves
/// out a block of the netlist is only found to be wrong by
/// [`resolve_nets`](super::resolve_nets), as VPR also accepts partial files of this form
/// for fixing a subset of the blocks.
///
/// # Errors
///
/// Fails on a line that isn't one of the above, a field that isn't a non-negative number or
/// is out of the range of the format, a block that's listed twice, invalid UTF-8, or a read
/// that fails, each with the line where it happened.
///
/// # Examples
///
/// ```
/// use guide::fpga::read_placement;
///
/// let text = "Netlist_File: top.net Netlist_ID: SHA256:ab\n\
///             Array size: 4 x 4 logic blocks\n\
///             \n\
///             #block name\tx\ty\tsubblk\tlayer\tblock number\n\
///             n27\t1\t1\t0\t0\t#0\n\
///             out:p6\t0\t1\t7\n";
/// let placement = read_placement(text.as_bytes())?;
/// assert_eq!(placement.netlist_file(), Some("top.net"));
/// assert_eq!(placement.array_size(), Some((4, 4)));
/// assert_eq!(placement.block("out:p6").unwrap().sub_block, 7);
/// # Ok::<(), guide::fpga::ImportError>(())
/// ```
pub fn read_placement<R: BufRead>(mut reader: R) -> Result<Placement, ImportError> {
    let mut placement = Placement::default();
    let mut line = Vec::new();
    let mut number = 0u64;
    loop {
        number += 1;
        if !read_bounded_line(&mut reader, &mut line, number)? {
            return Ok(placement);
        }
        let Ok(text) = core::str::from_utf8(&line) else {
            return Err(ImportError::Malformed {
                line: number,
                message: "invalid UTF-8".to_owned(),
            });
        };

        // At most 8 tokens matter, so a line of very many can't take very long.
        let mut tokens = [""; 8];
        let mut count = 0;
        for token in text.split_ascii_whitespace() {
            if token.starts_with('#') {
                break;
            }
            if count == tokens.len() {
                return Err(ImportError::Malformed {
                    line: number,
                    message: "too many fields".to_owned(),
                });
            }
            tokens[count] = token;
            count += 1;
        }
        let tokens = &tokens[..count];

        match *tokens {
            [] => {}
            ["Netlist_File:", file, "Netlist_ID:", id] => {
                placement.netlist_file = Some(file.to_owned());
                placement.netlist_id = Some(id.to_owned());
            }
            // What the documentation shows, and what older versions of VPR wrote.
            ["Netlist", "file:", file, "Architecture", "file:", _] => {
                placement.netlist_file = Some(file.to_owned());
            }
            ["Array", "size:", width, "x", height, "logic", "blocks"] => {
                placement.array_size = Some((
                    parse_field(number, "array width", width)?,
                    parse_field(number, "array height", height)?,
                ));
            }
            [name, x, y, sub_block] => {
                add_block(&mut placement, number, name, x, y, sub_block, "0")?;
            }
            [name, x, y, sub_block, layer] => {
                add_block(&mut placement, number, name, x, y, sub_block, layer)?;
            }
            _ => {
                return Err(ImportError::Malformed {
                    line: number,
                    message: format!("not a placement line: {:?}", truncated(text)),
                });
            }
        }
    }
}

fn truncated(text: &str) -> String {
    text.chars().take(64).collect()
}

fn add_block(
    placement: &mut Placement,
    line: u64,
    name: &str,
    x: &str,
    y: &str,
    sub_block: &str,
    layer: &str,
) -> Result<(), ImportError> {
    let block = PlacedBlock {
        name: name.to_owned(),
        x: parse_field(line, "x coordinate", x)?,
        y: parse_field(line, "y coordinate", y)?,
        sub_block: parse_field(line, "sub-block", sub_block)?,
        layer: parse_field(line, "layer", layer)?,
    };
    let index = placement.blocks.len() as u32;
    if placement.by_name.insert(name.to_owned(), index).is_some() {
        return Err(ImportError::Malformed {
            line,
            message: format!("block {} is placed twice", truncated(name)),
        });
    }
    placement.blocks.push(block);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::{BufReader, Cursor};

    const GENUINE: &str = include_str!("../../testdata/vtr/mult_4x4.place");
    const GENUINE_FIXED: &str = include_str!("../../testdata/vtr/apex2_block_locations.place");

    #[test]
    fn reads_a_genuine_placement() {
        let placement = read_placement(GENUINE.as_bytes()).unwrap();
        assert_eq!(placement.netlist_file(), Some("mult_4x4.net"));
        assert_eq!(
            placement.netlist_id(),
            Some("SHA256:7c3af470d4d65989ad5784af5f36aa171cc20fb464a2e5b5ff70b5ec873d787b")
        );
        assert_eq!(placement.array_size(), Some((4, 4)));
        assert_eq!(placement.blocks().len(), 19);
        assert_eq!(
            placement.block("out:p6"),
            Some(&PlacedBlock {
                name: "out:p6".to_owned(),
                x: 0,
                y: 1,
                sub_block: 7,
                layer: 0,
            })
        );
        assert_eq!(placement.block("n22").map(|block| block.x), Some(2));
    }

    #[test]
    fn reads_vprs_partial_placement_of_fixed_blocks() {
        // From a regression test of VPR's `--fix_clusters`, with odd names such as `[493]`.
        let placement = read_placement(GENUINE_FIXED.as_bytes()).unwrap();
        assert!(placement.array_size().is_none());
        assert!(placement.netlist_file().is_none());
        assert_eq!(placement.block("[493]").map(|block| block.x), Some(2));
        assert_eq!(
            placement.block("n_n1829").map(|block| block.sub_block),
            Some(4)
        );
    }

    #[test]
    fn reads_every_documented_form() {
        // The two-dimensional example of VPR's documentation.
        let text = "Netlist file: xor5.net   Architecture file: sample.xml\n\
            Array size: 2 x 2 logic blocks\n\n\
            #block name x       y       subblk  block number\n\
            #---------- --      --      ------- -----------\n\
            a           0       1       0       #0  -- NB: block number is a comment.\n\
            out:xor5    0       2       0       #5\n\
            [1]         1       1       0       #7\n\
            b 1 2 3 4\n\
            no_newline_at_end 1 1 1";
        let placement = read_placement(text.as_bytes()).unwrap();
        assert_eq!(placement.netlist_file(), Some("xor5.net"));
        assert_eq!(placement.netlist_id(), None);
        assert_eq!(placement.blocks().len(), 5);
        assert_eq!(placement.block("a").unwrap().layer, 0);
        assert_eq!(placement.block("b").unwrap().layer, 4);
        assert_eq!(placement.block("no_newline_at_end").unwrap().sub_block, 1);
    }

    #[test]
    fn rejects_bad_lines_with_their_numbers() {
        for (text, line) in [
            ("a 1 2\n", 1),
            ("\n\na 1 2 x\n", 3),
            ("a 1 2 3\na 1 2 4\n", 2),
            ("a -1 2 3\n", 1),
            ("a 1 2 3 4 5\n", 1),
            ("Array size: x x 2 logic blocks\n", 1),
            ("a 1 2 3\n\u{7F}\u{7F}\n", 2),
            ("a 99999999999 0 0\n", 1),
        ] {
            let error = read_placement(text.as_bytes()).unwrap_err();
            assert_eq!(error.line(), Some(line), "for {text:?}: {error}");
        }
    }

    #[test]
    fn rejects_invalid_utf8_and_overlong_lines() {
        let error = read_placement(&b"\nb\xFF 1 2 3\n"[..]).unwrap_err();
        assert_eq!(error.line(), Some(2));

        let long = vec![b'x'; MAX_LINE_BYTES + 1];
        assert!(matches!(
            read_placement(long.as_slice()),
            Err(ImportError::LineTooLong { line: 1, .. })
        ));
    }

    #[test]
    fn the_longest_line_allowed_is_read() {
        let mut line = vec![b'#'; MAX_LINE_BYTES];
        line.push(b'\n');
        line.extend_from_slice(b"a 1 2 3");
        let placement = read_placement(line.as_slice()).unwrap();
        assert_eq!(placement.blocks().len(), 1);
    }

    proptest! {
        #[test]
        fn random_input_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = read_placement(bytes.as_slice());
        }

        #[test]
        fn cutting_a_genuine_file_never_panics_and_keeps_what_came_before(cut: prop::sample::Index) {
            let whole = read_placement(GENUINE.as_bytes()).unwrap();
            let at = cut.index(GENUINE.len());
            // A cut line can leave a shorter valid line, or an invalid one, but never panics.
            if let Ok(cut_short) = read_placement(&GENUINE.as_bytes()[..at]) {
                prop_assert!(cut_short.blocks().len() <= whole.blocks().len());
            }
        }

        #[test]
        fn buffer_size_does_not_matter(capacity in 1usize..32) {
            let whole = read_placement(Cursor::new(GENUINE)).unwrap();
            let chunked =
                read_placement(BufReader::with_capacity(capacity, GENUINE.as_bytes())).unwrap();
            prop_assert_eq!(whole, chunked);
        }
    }
}
