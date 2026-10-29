//! Whitespace and line breaks for nodes moving in and out of a block. Pure kdl-rs text work: see notes/kdl-rs.md,
//! "Editing".
//!
//! The rule: a node starts at the beginning of a line and ends its own line. A new node goes at the first line start
//! at or after its place. If the rest of the line it lands on is blank, that's after the line's own ending; otherwise
//! it gets a line ending of its own. Taking a node out keeps whatever followed it at the start of a line if it was
//! there before. Every line ending rkdl adds is in the block's own style: `\r\n` if it has one, else `\n`.
//!
//! The exception is a children block written on one line (`a { b; c }`). It stays on one line, with `;` between
//! nodes, as long as the new node fits on one line. One that doesn't spreads the block over lines first. An empty
//! block prints as `{}`.

use kdl::{KdlDocument, KdlDocumentFormat, KdlNode, KdlNodeFormat};

use crate::tree::node_at;

/// KDL's line endings, apart from `\r\n`, which is `\r` then `\n`.
const NEWLINES: &[char] = &['\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}'];

fn is_space(c: char) -> bool {
    c.is_whitespace() && !NEWLINES.contains(&c)
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// Whether `text` ends a line, give or take spaces after the line ending.
fn ends_line(text: &str) -> bool {
    text.trim_end_matches(is_space).ends_with(NEWLINES)
}

/// The rest of a line at the start of `text`, if it's blank: spaces, then a line ending.
fn blank_rest(text: &str) -> Option<&str> {
    let spaces = text.len() - text.trim_start_matches(is_space).len();
    let (_, rest) = text.split_at_checked(spaces)?;
    let ending = if rest.starts_with("\r\n") {
        2
    } else {
        rest.chars().next().filter(|c| NEWLINES.contains(c))?.len_utf8()
    };
    text.get(..spaces + ending)
}

/// Whether `text` ends in a `\` line continuation that still needs its line ending.
fn ends_in_open_escline(text: &str) -> bool {
    text.trim_end_matches(is_space).ends_with('\\')
}

/// `text` with its `\n` and `\r\n` line endings in the style `eol`.
fn restyle(text: &str, eol: &str) -> String {
    let unix = text.replace("\r\n", "\n");
    if eol == "\n" { unix } else { unix.replace('\n', eol) }
}

fn leading(node: &KdlNode) -> &str {
    node.format().map_or("", |f| &f.leading)
}

/// The indentation in a leading: the whitespace that starts its last line.
fn indent_of(leading: &str) -> &str {
    let line = leading.rsplit(NEWLINES).next().unwrap_or("");
    line.strip_suffix(line.trim_start_matches(is_space)).unwrap_or_default()
}

fn fmt_mut<'a>(node: &'a mut KdlNode, eol: &str) -> &'a mut KdlNodeFormat {
    if node.format().is_none() {
        node.set_format(KdlNodeFormat {
            before_children: " ".into(),
            terminator: eol.into(),
            ..Default::default()
        });
    }
    node.format_mut().expect("just set")
}

fn block_fmt_mut(block: &mut KdlDocument) -> &mut KdlDocumentFormat {
    if block.format().is_none() {
        block.set_format(KdlDocumentFormat::default());
    }
    block.format_mut().expect("just set")
}

/// The style of the first line ending in `text`.
fn eol_in(text: &str) -> Option<&'static str> {
    let (before, _) = text.split_once('\n')?;
    Some(if before.ends_with('\r') { "\r\n" } else { "\n" })
}

/// The style of the first line ending in a block's text. Entries are skipped: their only line endings are in `\`
/// continuations.
fn first_eol(block: &KdlDocument) -> Option<&'static str> {
    let f = block.format();
    f.and_then(|f| eol_in(&f.leading))
        .or_else(|| block.nodes().iter().find_map(node_eol))
        .or_else(|| f.and_then(|f| eol_in(&f.trailing)))
}

fn node_eol(node: &KdlNode) -> Option<&'static str> {
    let f = node.format();
    f.and_then(|f| eol_in(&f.leading).or_else(|| eol_in(&f.before_children)))
        .or_else(|| node.children().and_then(first_eol))
        .or_else(|| {
            f.and_then(|f| {
                eol_in(&f.before_terminator)
                    .or_else(|| eol_in(&f.terminator))
                    .or_else(|| eol_in(&f.trailing))
            })
        })
}

/// The document's line ending style.
pub fn line_ending(doc: &KdlDocument) -> &'static str {
    first_eol(doc).unwrap_or("\n")
}

/// One level of indentation, from the first child in the document that starts a line indented.
fn indent_unit(doc: &KdlDocument) -> String {
    doc.nodes()
        .iter()
        .filter_map(|n| n.children()?.nodes().first())
        .map(leading)
        .filter(|l| l.contains(NEWLINES))
        .map(indent_of)
        .find(|indent| !indent.is_empty())
        .unwrap_or("    ")
        .to_owned()
}

/// How a block lays out its nodes.
pub struct Style {
    eol: &'static str,
    /// For a children block: the parent's indent and one level of indentation. None at the top level.
    nested: Option<(String, String)>,
}

impl Style {
    /// The style of the children block of the node at `parent`, or of the top level if that's None. The block must
    /// exist.
    pub fn of(doc: &KdlDocument, parent: Option<&[usize]>) -> Style {
        let parent = parent.map(|path| node_at(doc, path));
        let block = parent
            .map_or(Some(doc), KdlNode::children)
            .expect("the parent has a block");
        Style {
            eol: first_eol(block).unwrap_or_else(|| line_ending(doc)),
            nested: parent.map(|p| (indent_of(leading(p)).to_owned(), indent_unit(doc))),
        }
    }
}

/// Swaps an `old` indent for `new` at the start of each line, and line endings for `eol`. `line_start` says whether
/// the text itself starts a line. Blank lines stay blank.
fn reindent(text: &str, old: &str, new: &str, line_start: bool, eol: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        let last = i + 1 == lines.len();
        let line = if last {
            line
        } else {
            line.strip_suffix('\r').unwrap_or(line)
        };
        if i > 0 {
            out.push_str(eol);
        }
        let blank = line.is_empty() && !last;
        match line.strip_prefix(old) {
            Some(rest) if (i > 0 || line_start) && !blank => {
                out.push_str(new);
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// Line endings in a node's own text, apart from its leading and its entries.
fn restyle_node(f: &mut KdlNodeFormat, eol: &str) {
    for text in [
        &mut f.before_children,
        &mut f.before_terminator,
        &mut f.terminator,
        &mut f.trailing,
    ] {
        *text = restyle(text, eol);
    }
}

/// Moves everything under `node` from indent `old` to indent `new`, so a moved subtree lines up with its new place,
/// and gives its line endings the style of the block it lands in.
fn reindent_children(node: &mut KdlNode, old: &str, new: &str, eol: &str) {
    let Some(block) = node.children_mut() else {
        return;
    };
    let mut line_start = false;
    if let Some(f) = block.format_mut() {
        f.leading = reindent(&f.leading, old, new, false, eol);
        line_start = ends_line(&f.leading);
    }
    for child in block.nodes_mut() {
        if let Some(f) = child.format_mut() {
            f.leading = reindent(&f.leading, old, new, line_start, eol);
            restyle_node(f, eol);
            line_start = ends_line(&format!("{}{}", f.terminator, f.trailing));
        }
        reindent_children(child, old, new, eol);
    }
    if let Some(f) = block.format_mut() {
        f.trailing = reindent(&f.trailing, old, new, line_start, eol);
    }
}

/// Whether a node's text ends its line.
fn ends_own_line(node: &KdlNode) -> bool {
    node.format()
        .is_some_and(|f| ends_line(&format!("{}{}", f.terminator, f.trailing)))
}

/// Whether the node ends in a `\` continuation, which would carry it onto whatever comes next.
fn open_escline(node: &KdlNode) -> bool {
    node.format()
        .is_some_and(|f| f.terminator.is_empty() && f.trailing.is_empty())
        && ends_in_open_escline(&node.to_string())
}

/// Finishes an open `\` continuation and ends the node with `;` on the line after, which keeps the old text as it was.
fn close_escline(node: &mut KdlNode, eol: &str) {
    if open_escline(node) {
        let f = fmt_mut(node, eol);
        f.before_terminator.push_str(eol);
        f.terminator = ";".into();
    }
}

/// Whether what comes before index `index` of `block` ends a line, so a node there starts one.
fn line_start_at(block: &KdlDocument, index: usize, nested: bool) -> bool {
    if let Some(prev) = index.checked_sub(1).and_then(|i| block.nodes().get(i)) {
        return ends_own_line(prev);
    }
    let leading = block.format().map_or("", |f| &f.leading);
    // The `{` of a children block doesn't end a line. The start of the document does.
    ends_line(leading) || !nested && leading.chars().all(|c| is_space(c) || c == '\u{feff}')
}

/// The text after index `index` of `block`: the next node's leading, or the block's trailing.
fn after_mut<'a>(block: &'a mut KdlDocument, index: usize, eol: &str) -> &'a mut String {
    if index < block.nodes().len() {
        let next = block.nodes_mut().get_mut(index).expect("checked the length");
        &mut fmt_mut(next, eol).leading
    } else {
        &mut block_fmt_mut(block).trailing
    }
}

/// Whether a children block is written on one line, like `a { b; c }`.
fn is_single_line(block: &KdlDocument) -> bool {
    !block.nodes().is_empty() && !block.to_string().contains(NEWLINES)
}

/// Whether `node` can go into a single-line block: its text, apart from the whitespace around it, has no line
/// endings, and it ends with at most a `;`.
fn fits_on_a_line(node: &KdlNode) -> bool {
    let Some(f) = node.format() else {
        return false;
    };
    let text = node.to_string();
    let tail = format!("{}{}", f.terminator, f.trailing);
    let core = text
        .strip_prefix(f.leading.as_str())
        .and_then(|t| t.strip_suffix(tail.as_str()))
        .unwrap_or(&text);
    !f.leading.trim_start().contains(NEWLINES)
        && !core.contains(NEWLINES)
        && !ends_in_open_escline(core)
        && matches!(tail.trim(), "" | ";")
}

/// The whitespace a node's leading starts with.
fn space_before(node: &KdlNode) -> String {
    let leading = leading(node);
    leading
        .strip_suffix(leading.trim_start_matches(is_space))
        .unwrap_or_default()
        .to_owned()
}

/// `place` for a single-line block: `;` between nodes, and the space before `}` stays at the end.
fn place_inline(block: &mut KdlDocument, index: usize, mut node: KdlNode) {
    let nodes = block.nodes_mut();
    let (space, terminator, closing) = if let Some(next) = nodes.get(index) {
        (space_before(next), ";".to_owned(), String::new())
    } else {
        // The new node takes over the end of the block from the old last one.
        let prev = nodes.last_mut().expect("a single-line block has nodes");
        let space = space_before(prev);
        let f = fmt_mut(prev, "\n");
        let content = f.before_terminator.trim_end_matches(is_space).len();
        let closing = f.before_terminator.split_off(content);
        (space, std::mem::replace(&mut f.terminator, ";".into()), closing)
    };
    let f = fmt_mut(&mut node, "\n");
    f.leading = format!("{space}{}", f.leading.trim_start());
    let content = f.before_terminator.trim_end_matches(is_space).len();
    f.before_terminator.truncate(content);
    f.before_terminator.push_str(&closing);
    f.terminator = terminator;
    f.trailing.clear();
    nodes.insert(index, node);
}

/// `unplace` for a single-line block. The node before keeps its `;`, so its text stays as it was.
fn unplace_inline(block: &mut KdlDocument, index: usize) -> KdlNode {
    let node = block.nodes_mut().remove(index);
    if index == block.nodes().len()
        && index > 0
        && let Some(taken) = node.format()
    {
        // The space before `}` was the taken node's.
        let bt = &taken.before_terminator;
        let closing = bt.get(bt.trim_end_matches(is_space).len()..).unwrap_or_default();
        block_fmt_mut(block).trailing.insert_str(0, closing);
    }
    node
}

/// Spreads a single-line block over lines, one node per line.
fn spread(block: &mut KdlDocument, parent_indent: &str, unit: &str, eol: &str) {
    for (i, node) in block.nodes_mut().iter_mut().enumerate() {
        let f = fmt_mut(node, eol);
        let start = if i == 0 { eol } else { "" };
        f.leading = format!("{start}{parent_indent}{unit}{}", f.leading.trim_start());
        let content = f.before_terminator.trim_end_matches(is_space).len();
        f.before_terminator.truncate(content);
        f.terminator = eol.into();
        f.trailing.clear();
    }
    let f = block_fmt_mut(block);
    if is_blank(&f.leading) {
        f.leading.clear();
    }
    if is_blank(&f.trailing) {
        parent_indent.clone_into(&mut f.trailing);
    }
}

/// The indent of the line the node before `index` starts on (or the node at 0), if one starts a line.
fn sibling_indent(block: &KdlDocument, index: usize) -> Option<String> {
    let nodes = block.nodes().get(..=index.saturating_sub(1))?;
    nodes
        .iter()
        .rev()
        .map(leading)
        .find(|l| l.contains(NEWLINES))
        .map(|l| indent_of(l).to_owned())
}

/// Puts `node` into `block` at `index`, laid out like its neighbours.
pub fn place(block: &mut KdlDocument, index: usize, mut node: KdlNode, style: &Style) {
    let eol = style.eol;
    if let Some((parent_indent, unit)) = &style.nested
        && is_single_line(block)
    {
        if fits_on_a_line(&node) {
            place_inline(block, index, node);
            return;
        }
        spread(block, parent_indent, unit, eol);
    }

    let indent = match &style.nested {
        None => String::new(),
        Some((parent_indent, unit)) => {
            if block.nodes().is_empty() {
                // The closing `}` lines up with the parent.
                let f = block_fmt_mut(block);
                if is_blank(&f.leading) && is_blank(&f.trailing) {
                    f.leading.clear();
                    parent_indent.clone_into(&mut f.trailing);
                }
            }
            sibling_indent(block, index).unwrap_or_else(|| format!("{parent_indent}{unit}"))
        }
    };
    if let Some(prev) = index.checked_sub(1).and_then(|i| block.nodes_mut().get_mut(i)) {
        close_escline(prev, eol);
    }
    let start = if line_start_at(block, index, style.nested.is_some()) {
        String::new()
    } else {
        let after = after_mut(block, index, eol);
        match blank_rest(after).map(str::to_owned) {
            Some(rest) => {
                after.replace_range(..rest.len(), "");
                rest
            }
            None => eol.to_owned(),
        }
    };

    let escline = open_escline(&node);
    let f = fmt_mut(&mut node, eol);
    let old = indent_of(&f.leading).to_owned();
    // Keep the node's own comments, but not the whitespace it had where it came from.
    let own = reindent(f.leading.trim_start(), &old, &indent, false, eol);
    f.leading = format!("{start}{indent}{own}");
    if is_blank(&f.trailing) {
        f.trailing.clear();
    }
    if escline {
        close_escline(&mut node, eol);
        fmt_mut(&mut node, eol).trailing = eol.into();
    } else if !ends_own_line(&node) {
        let f = fmt_mut(&mut node, eol);
        if f.trailing.is_empty() {
            f.terminator = eol.into();
        } else {
            f.trailing.push_str(eol);
        }
    }
    restyle_node(fmt_mut(&mut node, eol), eol);
    reindent_children(&mut node, &old, &indent, eol);
    block.nodes_mut().insert(index, node);
}

/// Takes the node at `index` out of `block`, the inverse of `place`.
pub fn unplace(block: &mut KdlDocument, index: usize, style: &Style) -> KdlNode {
    let nested = style.nested.is_some();
    let node = if nested && is_single_line(block) {
        unplace_inline(block, index)
    } else {
        let node = block.nodes_mut().remove(index);
        let after = match block.nodes().get(index) {
            Some(next) => leading(next),
            None => block.format().map_or("", |f| &f.trailing),
        };
        if ends_own_line(&node) && blank_rest(after).is_none() && !line_start_at(block, index, nested) {
            // What followed the node started a line, and still does.
            let ending = blank_rest(leading(&node)).unwrap_or(style.eol);
            after_mut(block, index, style.eol).insert_str(0, ending);
        }
        node
    };
    // An emptied block prints as `{}`.
    if nested
        && block.nodes().is_empty()
        && let Some(f) = block.format_mut()
        && is_blank(&f.leading)
        && is_blank(&f.trailing)
    {
        f.leading.clear();
        f.trailing.clear();
    }
    node
}

/// Gives `node` an empty children block, `{}`. `eol` is the document's line ending style.
pub fn open_block(node: &mut KdlNode, eol: &str) {
    let f = fmt_mut(node, eol);
    // Without a block, trailing slashdashed entries sit in before_terminator, which prints after the block. They have
    // to stay in front of it.
    if !is_blank(&f.before_terminator) {
        let mut moved = std::mem::take(&mut f.before_terminator);
        // Autoformat ends before_terminator with a line ending. That goes after the block, not before `{`, where it
        // would end the node early.
        if ends_line(&moved) {
            moved.truncate(moved.trim_end().len());
            if f.terminator.is_empty() {
                f.terminator = eol.into();
            }
        }
        if ends_in_open_escline(&moved) {
            moved.push_str(eol);
        }
        f.before_children = moved + &f.before_children;
    }
    if !f.before_children.ends_with(char::is_whitespace) {
        f.before_children.push(' ');
    }
    let mut block = KdlDocument::new();
    block.set_format(KdlDocumentFormat::default());
    node.set_children(block);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> KdlDocument {
        text.parse().expect("test documents parse")
    }

    fn node(text: &str) -> KdlNode {
        parse(text).nodes()[0].clone()
    }

    /// Places `new` at `index` in the block at `parent` (None: the top level).
    fn placed(text: &str, parent: Option<&[usize]>, index: usize, new: &str) -> String {
        let mut doc = parse(text);
        let style = Style::of(&doc, parent);
        let block = match parent {
            None => &mut doc,
            Some(path) => crate::tree::node_at_mut(&mut doc, path)
                .children_mut()
                .as_mut()
                .unwrap(),
        };
        place(block, index, node(new), &style);
        let out = doc.to_string();
        assert!(out.parse::<KdlDocument>().is_ok(), "{out:?} doesn't parse");
        out
    }

    fn unplaced(text: &str, parent: Option<&[usize]>, index: usize) -> String {
        let mut doc = parse(text);
        let style = Style::of(&doc, parent);
        let block = match parent {
            None => &mut doc,
            Some(path) => crate::tree::node_at_mut(&mut doc, path)
                .children_mut()
                .as_mut()
                .unwrap(),
        };
        unplace(block, index, &style);
        doc.to_string()
    }

    #[test]
    fn helpers() {
        assert_eq!(blank_rest("  \r\n    b"), Some("  \r\n"));
        assert_eq!(blank_rest("\n"), Some("\n"));
        assert_eq!(blank_rest(" // c\n"), None);
        assert_eq!(blank_rest(""), None);
        assert!(ends_line("a\r\n  "));
        assert!(!ends_line("a;"));
        assert_eq!(indent_of("\r\n    // c\r\n  "), "  ");
        assert_eq!(restyle("a\nb\r\nc", "\r\n"), "a\r\nb\r\nc");
        assert_eq!(restyle("a\r\nb", "\n"), "a\nb");
        assert_eq!(reindent("// c\n  x", "  ", "    ", false, "\r\n"), "// c\r\n    x");
    }

    #[test]
    fn line_ending_comes_from_the_block_then_the_document() {
        assert_eq!(line_ending(&parse("a\r\nb\r\n")), "\r\n");
        assert_eq!(line_ending(&parse("a {\n}\r\n")), "\n");
        assert_eq!(line_ending(&parse("a")), "\n");
        let doc = parse("a { b }\r\n");
        assert_eq!(Style::of(&doc, Some(&[0])).eol, "\r\n");
    }

    #[test]
    fn multi_line() {
        let text = "a {\n    b\n    c\n}\n";
        assert_eq!(placed(text, Some(&[0]), 0, "x"), "a {\n    x\n    b\n    c\n}\n");
        assert_eq!(placed(text, Some(&[0]), 1, "x"), "a {\n    b\n    x\n    c\n}\n");
        assert_eq!(placed(text, Some(&[0]), 2, "x"), "a {\n    b\n    c\n    x\n}\n");
        assert_eq!(unplaced(text, Some(&[0]), 0), "a {\n    c\n}\n");
        assert_eq!(unplaced(text, Some(&[0]), 1), "a {\n    b\n}\n");
        assert_eq!(unplaced("a {\n    b\n}\n", Some(&[0]), 0), "a {}\n");
    }

    #[test]
    fn crlf() {
        let text = "a {\r\n    b\r\n    c\r\n}\r\n";
        assert_eq!(
            placed(text, Some(&[0]), 0, "x"),
            "a {\r\n    x\r\n    b\r\n    c\r\n}\r\n"
        );
        assert_eq!(
            placed(text, Some(&[0]), 1, "x"),
            "a {\r\n    b\r\n    x\r\n    c\r\n}\r\n"
        );
        assert_eq!(
            placed(text, Some(&[0]), 2, "x"),
            "a {\r\n    b\r\n    c\r\n    x\r\n}\r\n"
        );
        assert_eq!(placed("a {}\r\n", Some(&[0]), 0, "x"), "a {\r\n    x\r\n}\r\n");
        assert_eq!(placed("a\r\nb", None, 2, "x"), "a\r\nb\r\nx\r\n");
        assert_eq!(unplaced(text, Some(&[0]), 0), "a {\r\n    c\r\n}\r\n");
        // A moved subtree takes the style of where it lands.
        assert_eq!(
            placed("a {}\r\n", Some(&[0]), 0, "x {\n    // c\n    y\n}"),
            "a {\r\n    x {\r\n        // c\r\n        y\r\n    }\r\n}\r\n"
        );
    }

    #[test]
    fn single_line() {
        let text = "a { b; c }\n";
        assert_eq!(placed(text, Some(&[0]), 0, "x"), "a { x; b; c }\n");
        assert_eq!(placed(text, Some(&[0]), 1, "x"), "a { b; x; c }\n");
        assert_eq!(placed(text, Some(&[0]), 2, "x 1 { y }"), "a { b; c; x 1 { y } }\n");
        assert_eq!(placed("a {b}", Some(&[0]), 1, "x"), "a {b;x}");
        assert_eq!(placed("a { b; }", Some(&[0]), 1, "x"), "a { b; x; }");
        assert_eq!(unplaced(text, Some(&[0]), 0), "a { c }\n");
        assert_eq!(unplaced(text, Some(&[0]), 1), "a { b; }\n");
        assert_eq!(unplaced("a { b }\n", Some(&[0]), 0), "a {}\n");
        // A node that needs more than one line spreads the block out.
        assert_eq!(
            placed(text, Some(&[0]), 1, "// about x\nx"),
            "a {\n    b\n    // about x\n    x\n    c\n}\n"
        );
    }

    #[test]
    fn line_rest_and_semicolons() {
        assert_eq!(placed("a {  \n    b\n}", Some(&[0]), 0, "x"), "a {  \n    x\n    b\n}");
        assert_eq!(
            placed("a {\n    b; c\n}", Some(&[0]), 2, "x"),
            "a {\n    b; c\n    x\n}"
        );
        assert_eq!(placed("a; b\n", None, 1, "x"), "a;\nx\n b\n");
        assert_eq!(unplaced("a {\n    b; c\n}", Some(&[0]), 1), "a {\n    b;\n}");
        assert_eq!(unplaced("a { b\n}", Some(&[0]), 0), "a {}");
        assert_eq!(unplaced("a { b\n    c\n}", Some(&[0]), 0), "a {\n    c\n}");
    }

    #[test]
    fn top_level() {
        assert_eq!(placed("", None, 0, "x"), "x\n");
        assert_eq!(placed("// hi", None, 0, "x"), "// hi\nx\n");
        assert_eq!(placed("\u{feff}a\n", None, 0, "x"), "\u{feff}x\na\n");
        assert_eq!(placed("node \\", None, 1, "x"), "node \\\n;\nx\n");
        assert_eq!(unplaced("a\nb\n", None, 0), "b\n");
        assert_eq!(unplaced("a; b; c\n", None, 1), "a; c\n");
    }

    #[test]
    fn a_moved_node_ending_in_a_continuation_ends_its_line() {
        let mut doc = parse("a\n");
        let style = Style::of(&doc, None);
        let continued = parse("n 1 \\").nodes()[0].clone();
        place(&mut doc, 0, continued, &style);
        assert_eq!(doc.to_string(), "n 1 \\\n;\na\n");
    }

    #[test]
    fn open_block_keeps_slashdashed_entries_in_front() {
        let mut doc = parse("n /- k=v\r\n");
        open_block(&mut doc.nodes_mut()[0], "\r\n");
        assert_eq!(doc.to_string(), "n /- k=v {}\r\n");
    }
}
