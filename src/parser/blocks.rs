//! Puts back the text kdl-rs drops after a block's `}`.
//!
//! After a node's block (and any slashdashed blocks after it), kdl-rs reads spaces, comments and line continuations
//! and throws them away, and with them the node's terminator, which its caller then reads and doesn't keep either:
//! `a {}  // end` followed by a line ending prints as `a {}`. Only a terminator right after the `}` survives, as the
//! node's own. The node's span still covers the dropped text, so it can be found in the source: from the last `}` of
//! the node's level before the span's end, to the span's end, then the terminator after it. It goes back where kdl-rs
//! keeps the same text after a node without a block: the spaces and comments at the end of `before_terminator`, and
//! the `;`, line ending or line comment as the terminator.
//!
//! Finding the `}` takes the prescan's record of where blocks close, which it has only if it followed the whole text.
//! The scan follows every valid document, so that's whenever kdl-rs parses one, unless kdl-rs takes text the spec
//! doesn't. Without the record, or if the dropped text can't be placed, only the terminator comes back: the terminator
//! is right after the node's span, so it's found without knowing where the `}` is. The spaces and comments between
//! the `}` and the terminator stay lost, but the line ending or `;` that keeps the next node apart is back, and the
//! printed text still parses.

use kdl::{KdlDocument, KdlNode};

use super::prescan::NEWLINES;

/// `closes` is the scan's record of where blocks close, by depth, or None if the scan didn't follow the text.
pub(crate) fn restore(text: &str, doc: &mut KdlDocument, closes: Option<&[Vec<usize>]>) {
    // A work list rather than recursion: the parse thread has stack to spare, but this needn't use it.
    let mut pending = vec![(doc, 0)];
    while let Some((block, depth)) = pending.pop() {
        let closes = closes.map(|closes| closes.get(depth).map_or(&[][..], Vec::as_slice));
        for node in block.nodes_mut() {
            restore_node(text, node, closes);
            if let Some(children) = node.children_mut() {
                pending.push((children, depth + 1));
            }
        }
    }
}

fn restore_node(text: &str, node: &mut KdlNode, closes: Option<&[usize]>) {
    if node.children().is_none() {
        return;
    }
    let span = node.span();
    let (start, end) = (span.offset(), span.offset() + span.len());
    let Some(f) = node.format_mut() else {
        return;
    };
    if !f.terminator.is_empty() {
        return;
    }
    let Some(rest) = text.get(end..) else {
        return;
    };
    f.terminator = terminator(rest).into();
    // The node's last `}`: its block's, or that of a slashdashed block after it, which ends `before_terminator`.
    let Some(&close) = closes.and_then(|closes| closes.get(closes.partition_point(|&c| c < end).wrapping_sub(1)))
    else {
        return;
    };
    let after = close + 1;
    let (Some(kept), Some(dropped)) = (text.get(start..after), text.get(after..end)) else {
        return;
    };
    if close >= start && kept.ends_with(&*f.before_terminator) {
        f.before_terminator.push_str(dropped);
    }
}

/// The node terminator `rest` starts with: `;`, a line ending, or a line comment and its line ending. Empty at the end
/// of the text or before a `}`.
fn terminator(rest: &str) -> &str {
    let len = if rest.starts_with(';') {
        1
    } else if rest.starts_with("\r\n") {
        2
    } else if rest.starts_with(NEWLINES) {
        rest.chars().next().map_or(0, char::len_utf8)
    } else if rest.starts_with("//") {
        match rest.find(NEWLINES) {
            Some(i) => i + terminator(rest.get(i..).unwrap_or_default()).len(),
            None => rest.len(),
        }
    } else {
        0
    };
    rest.get(..len).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::prescan;

    fn round_trip(text: &str) -> String {
        let mut doc: KdlDocument = text.parse().expect("test documents parse");
        let scan = prescan::scan(text);
        assert_eq!(scan.broke_at, None);
        restore(text, &mut doc, Some(&scan.closes));
        doc.to_string()
    }

    /// As if the scan hadn't followed the text.
    fn without_closes(text: &str) -> String {
        let mut doc: KdlDocument = text.parse().expect("test documents parse");
        restore(text, &mut doc, None);
        doc.to_string()
    }

    #[test]
    fn without_the_scan_the_terminator_comes_back() {
        for (text, printed) in [
            ("a {\n  b\n} // after\nc", "a {\n  b\n}// after\nc"),
            ("a {}  \nb", "a {}\nb"),
            ("a {} /* c */ ;b", "a {};b"),
            ("a { b {}  \n}", "a { b {}\n}"),
            ("a {} \\\n ;", "a {};"),
            ("a {}  ", "a {}"),
            ("a {};b", "a {};b"),
        ] {
            let out = without_closes(text);
            assert_eq!(out, printed);
            assert!(out.parse::<KdlDocument>().is_ok(), "{out:?}");
        }
    }

    #[test]
    fn text_after_a_closing_brace_comes_back() {
        for text in [
            "a {}  \nb",
            "a {} // c\nb",
            "a {} /* c */ // d\r\nb",
            "a {}\t;b",
            "a {} /-{} /-{ x }  \nb",
            "p {\n    a {} // end a\n} // end p\n",
            "a { b {} }",
            "a { b { c {} } } ",
            "a {} \\\n ;",
            "a {}  ",
            "a {} // no line ending",
            "a { b {} // c\r\n}",
            "\u{feff}a {}  \n",
            "a {\n    /- b { c {} }\n    d {} } // x\n",
        ] {
            assert_eq!(round_trip(text), text);
        }
    }

    #[test]
    fn terminators() {
        assert_eq!(terminator(";x"), ";");
        assert_eq!(terminator("\r\nx"), "\r\n");
        assert_eq!(terminator("\u{2028}x"), "\u{2028}");
        assert_eq!(terminator("// c\r\nx"), "// c\r\n");
        assert_eq!(terminator("// c"), "// c");
        assert_eq!(terminator("}"), "");
        assert_eq!(terminator(""), "");
    }
}
