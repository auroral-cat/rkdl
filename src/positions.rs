//! Where parsed nodes and entries were in the text, as lines and columns.
//!
//! kdl-rs records byte spans into the source, which stop meaning anything once the document is edited. So positions
//! are worked out once, at parse time, and kept by id. Each parse gets an `Origin`, so a node moved into a document
//! parsed from other text can drop positions that belong to another file.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::parser::prescan::NEWLINES;

/// A line and a column, both starting at 1. Columns count characters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pos {
    pub(crate) line: u32,
    pub(crate) column: u32,
}

/// One parse of one text. Positions only mean something in a document from the same parse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Origin(NonZeroU64);

static NEXT_ORIGIN: AtomicU64 = AtomicU64::new(1);

impl Origin {
    pub(crate) fn fresh() -> Self {
        Origin(NonZeroU64::new(NEXT_ORIGIN.fetch_add(1, Ordering::Relaxed)).expect("origin counter overflowed"))
    }
}

/// The positions of byte `offsets` into `text`, by key, in one pass over the text.
///
/// Lines break at every KDL newline, with CRLF as one. A byte order mark at the start isn't a column: editors don't
/// show it.
pub(crate) fn locate(text: &str, mut offsets: Vec<(usize, u64)>) -> HashMap<u64, Pos> {
    offsets.sort_unstable_by_key(|&(offset, _)| offset);
    let mut found = HashMap::with_capacity(offsets.len());
    let start = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut chars = text.get(start..).unwrap_or_default().char_indices().peekable();
    let (mut line, mut column) = (1u32, 1u32);
    for (offset, key) in offsets {
        while let Some(&(i, c)) = chars.peek() {
            if start + i >= offset {
                break;
            }
            chars.next();
            if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                // The `\n` ends the line.
                continue;
            }
            if NEWLINES.contains(&c) {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(1);
            }
        }
        found.insert(key, Pos { line, column });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, offset: usize) -> (u32, u32) {
        let pos = locate(text, vec![(offset, 0)])[&0];
        (pos.line, pos.column)
    }

    #[test]
    fn lines_and_columns() {
        assert_eq!(at("abc", 0), (1, 1));
        assert_eq!(at("abc", 2), (1, 3));
        assert_eq!(at("a\nbc", 3), (2, 2));
        assert_eq!(at("a\r\nbc", 4), (2, 2));
        assert_eq!(at("a\rbc", 3), (2, 2));
        assert_eq!(at("a\r\n\r\nb", 5), (3, 1));
        assert_eq!(at("a\u{2028}b", 4), (2, 1));
        assert_eq!(at("\tb", 1), (1, 2));
        assert_eq!(at("é b", 3), (1, 3));
        assert_eq!(at("\u{feff}a b", 5), (1, 3));
    }

    #[test]
    fn many_offsets_in_any_order() {
        let text = "a\nb c\n  d";
        let found = locate(text, vec![(8, 3), (0, 0), (4, 2), (2, 1)]);
        let pos = |k: u64| (found[&k].line, found[&k].column);
        assert_eq!(pos(0), (1, 1));
        assert_eq!(pos(1), (2, 1));
        assert_eq!(pos(2), (2, 3));
        assert_eq!(pos(3), (3, 3));
    }
}
