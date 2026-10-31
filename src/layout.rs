//! Whitespace and line breaks for nodes moving in and out of a block. All of it is text in kdl-rs's format records,
//! which print in order: a node's `KdlNodeFormat` holds the text before it (`leading`, comments included), before its
//! block, before its terminator, the terminator (a line ending or `;`), and `trailing` after that. A block's
//! `KdlDocumentFormat` holds the text before its first node and after its last. A node carries its own text with it,
//! so this module only moves and rewrites those strings.
//!
//! The rule: a node starts at the beginning of a line and ends its own line, and every line break belongs to the thing
//! before it. A node's line ending is in its own text. The one after a block's `{` is at the start of the block's first
//! node, which is where kdl-rs puts it. kdl-rs also puts the one after a `;` at the start of what follows, but it's the
//! node's all the same, with any comments before it on the node's line. So is a `//` comment that ends the node's line,
//! and at the end of the file, where the end of the text ends the line, so are `/* */` comments. A copy of a node has
//! them too. A new node goes at the first line start at or after its place: if the node before it doesn't end its line,
//! it gets a line ending first, and a node that went on after the place on that line starts the line after the new
//! one, at its indent. If the node before ends in a `\` continuation, which would carry it onto the new node, it ends
//! with `;` on the next line instead, at its indent. A line ending rkdl adds after `{` and the comments on its line
//! comes right after them, without spaces. Taking a node out takes its own line ending with it, and whatever followed
//! it keeps starting a line if it did: a node that was second on a line takes over its start. Replacing a node and
//! reversing a block keep the layout by position instead: each place keeps the whitespace before it and the end of its
//! line, its `;` and line ending included, and the nodes move with their own comments, the ones at the end of their
//! line too. A `//` comment that lands at the end of a line takes the place of the spaces there.
//!
//! Comments above a node are its own only up to the last blank line above it. What's above that line, the line
//! included, is the gap: it belongs to the block and stays where it is. Taking a node out leaves its gap in front of
//! whatever followed it, a new node goes below the gap of the node it goes in front of, and reordering keeps gaps by
//! position. So a file's header comment, if a blank line follows it, stays at the top. Slashdashed nodes in the leading
//! text count as comments. A comment is one piece: a blank line inside it doesn't end the gap. The rest of the line
//! before a node that doesn't start one is in the gap too.
//!
//! Every line ending rkdl adds is in the block's own style, its first line ending, or else the document's, which is
//! the first line ending in the text it was parsed from (`\n` if there was none). A moved node takes the style and the
//! indentation of where it lands, unit and character included, except inside strings. That goes for its comments, its
//! children and the lines of its own text: `\` continuations, and comments and slashdashed parts after its name. Each
//! line shifts by the change in the node's indent, plus the change in unit for each level of its children it's in.
//!
//! The exception is a children block written on one line (`a { b; c }`). It stays on one line, with `;` between
//! nodes, as long as the new node fits on one line. One that doesn't spreads the block over lines first, with the
//! comments before its `}` on a line of their own and `}` lined up with the parent. An empty block prints as `{}`.
//!
//! rkdl never writes whitespace right after a `}`: kdl-rs drops it when reading, a known issue. rkdl's own
//! parse puts it back (`parser/blocks.rs`), but what rkdl writes stays readable by kdl-rs alone. After a `}` rkdl
//! writes comes a line ending, a `;`, or the parent's `}`. So a new block goes after a node's trailing spaces, and a
//! node with a block at the end of a one-line block ends with `;` before the space: `a { b {}; }`.

use kdl::{KdlDocument, KdlDocumentFormat, KdlNode, KdlNodeFormat};

use crate::tree::{node_at, node_at_mut};

/// KDL's line endings, apart from `\r\n`, which is `\r` then `\n`.
const NEWLINES: &[char] = &['\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}'];

fn is_space(c: char) -> bool {
    c.is_whitespace() && !NEWLINES.contains(&c)
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// The line ending `text` starts with.
fn eol_at(text: &str) -> Option<&'static str> {
    if text.starts_with("\r\n") {
        return Some("\r\n");
    }
    Some(match text.chars().next()? {
        '\n' => "\n",
        '\r' => "\r",
        '\u{b}' => "\u{b}",
        '\u{c}' => "\u{c}",
        '\u{85}' => "\u{85}",
        '\u{2028}' => "\u{2028}",
        '\u{2029}' => "\u{2029}",
        _ => return None,
    })
}

/// The first line ending in `text`.
fn eol_in(text: &str) -> Option<&'static str> {
    let (_, rest) = text.split_at_checked(text.find(NEWLINES)?)?;
    eol_at(rest)
}

/// The lines of `text`, each with its line ending. The last one's is empty.
fn lines(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((line, tail)) = rest.find(NEWLINES).and_then(|i| rest.split_at_checked(i))
        && let Some((ending, next)) = eol_at(tail).and_then(|e| tail.split_at_checked(e.len()))
    {
        out.push((line, ending));
        rest = next;
    }
    out.push((rest, ""));
    out
}

/// Whether `text` ends a line, give or take spaces after the line ending.
fn ends_line(text: &str) -> bool {
    text.trim_end_matches(is_space).ends_with(NEWLINES)
}

/// The rest of a line at the start of `text`, if it's blank: spaces, then a line ending.
fn blank_rest(text: &str) -> Option<&str> {
    let spaces = text.len() - text.trim_start_matches(is_space).len();
    let (_, rest) = text.split_at_checked(spaces)?;
    text.get(..spaces + eol_at(rest)?.len())
}

/// The rest of a line at the start of `text`, if it holds only spaces and comments: up to and including the line
/// ending. A `//` comment with nothing after it is at the end of the file, which ends its line too.
fn line_rest(text: &str) -> Option<&str> {
    let line = text.split(NEWLINES).next().unwrap_or_default();
    let ending = text.get(line.len()..).and_then(eol_at);
    let mut rest = line.trim_start_matches(is_space);
    while rest.starts_with("/*") {
        let mut scan = Scan { text: rest, pos: 0 };
        if !scan.block_comment() {
            // It goes on past the line.
            return None;
        }
        rest = scan.text_at().trim_start_matches(is_space);
    }
    match ending {
        Some(ending) => (rest.is_empty() || rest.starts_with("//")).then(|| text.get(..line.len() + ending.len()))?,
        None => rest.starts_with("//").then_some(text),
    }
}

/// `line_rest`, where `file_end` says whether the end of the text is the end of the file, which ends the line too.
fn line_rest_in(text: &str, file_end: bool) -> Option<&str> {
    let comments = file_end && !is_blank(text) && !text.contains(NEWLINES) && closing_comments_at(text) == 0;
    line_rest(text).or(comments.then_some(text))
}

/// The comments in the rest of a line, as `line_rest` gives it, with the spaces before them. Spaces after them are
/// the line's, but for a `//` comment's, which are part of it.
fn comments_in(rest: &str) -> &str {
    let line = rest.split(NEWLINES).next().unwrap_or_default();
    if line_comment_at(line) < line.len() {
        line
    } else {
        line.trim_end_matches(is_space)
    }
}

/// Where the comments that end `text` start: spaces and `/* */` comments on one line, up to the end.
fn closing_comments_at(text: &str) -> usize {
    let mut scan = Scan { text, pos: 0 };
    let mut at = 0;
    while let Some(c) = scan.peek() {
        let start = scan.pos;
        if is_space(c) {
            scan.bump();
            continue;
        } else if scan.text_at().starts_with("/*") {
            scan.block_comment();
            if !text.get(start..scan.pos).unwrap_or_default().contains(NEWLINES) {
                continue;
            }
        } else if scan.eat("//") {
            scan.line_comment();
        } else if matches!(c, '"' | '#') {
            scan.string();
        } else {
            scan.bump();
        }
        at = scan.pos;
    }
    at
}

/// Where the `//` comment in a line of comments starts, if it has one, else its length.
fn line_comment_at(comments: &str) -> usize {
    let mut scan = Scan { text: comments, pos: 0 };
    while let Some(c) = scan.peek() {
        if scan.text_at().starts_with("//") {
            break;
        } else if scan.text_at().starts_with("/*") {
            scan.block_comment();
        } else {
            scan.pos += c.len_utf8();
        }
    }
    scan.pos
}

/// The byte ranges of the pieces of a node's leading text that can hold line endings of their own: `/* */` comments
/// and slashdashed nodes. A blank line in one is part of it, not a break between pieces.
fn multi_line_pieces(leading: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut scan = Scan { text: leading, pos: 0 };
    while let Some(c) = scan.peek() {
        let start = scan.pos;
        if scan.text_at().starts_with("/*") {
            scan.block_comment();
            out.push(start..scan.pos);
        } else if scan.eat("/-") {
            scan.dashed_node();
            out.push(start..scan.pos);
        } else if scan.eat("//") {
            scan.line_comment();
        } else {
            scan.pos += c.len_utf8();
        }
    }
    out
}

/// The byte ranges of the strings in `text`, quotes included.
fn string_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut scan = Scan { text, pos: 0 };
    while let Some(c) = scan.peek() {
        let start = scan.pos;
        if scan.eat("//") {
            scan.line_comment();
        } else if scan.text_at().starts_with("/*") {
            scan.block_comment();
        } else if matches!(c, '"' | '#') {
            scan.string();
            out.push(start..scan.pos);
        } else {
            scan.bump();
        }
    }
    out
}

/// Whether `text` ends in a `\` continuation, which carries the node on over its line ending: None if it doesn't,
/// else whether that line ending is in `text` too.
fn continuation(text: &str) -> Option<bool> {
    let mut scan = Scan { text, pos: 0 };
    let mut open = None;
    while let Some(c) = scan.peek() {
        if NEWLINES.contains(&c) {
            // A second line ending ends the node.
            open = (open == Some(false)).then_some(true);
            if !scan.eat("\r\n") {
                scan.bump();
            }
        } else if is_space(c) {
            scan.bump();
        } else if scan.eat("//") {
            scan.line_comment();
        } else if scan.text_at().starts_with("/*") {
            scan.block_comment();
        } else if scan.eat("\\") {
            open = Some(false);
        } else {
            open = None;
            if matches!(c, '"' | '#') {
                scan.string();
            } else {
                scan.bump();
            }
        }
    }
    open
}

/// Reads valid KDL text far enough to tell where comments, strings and slashdashed nodes end.
struct Scan<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Scan<'a> {
    fn text_at(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        self.text_at().chars().next()
    }

    fn bump(&mut self) {
        self.pos += self.peek().map_or(0, char::len_utf8);
    }

    fn eat(&mut self, s: &str) -> bool {
        let found = self.text_at().starts_with(s);
        if found {
            self.pos += s.len();
        }
        found
    }

    /// Up to the line ending, which is left.
    fn line_comment(&mut self) {
        self.pos += self.text_at().find(NEWLINES).unwrap_or(self.text_at().len());
    }

    /// From a `/*` to its `*/`, if the text has it. They nest.
    fn block_comment(&mut self) -> bool {
        self.pos += 2;
        let mut depth = 1;
        while depth > 0 && self.peek().is_some() {
            if self.eat("/*") {
                depth += 1;
            } else if self.eat("*/") {
                depth -= 1;
            } else {
                self.bump();
            }
        }
        depth == 0
    }

    /// A string at a `"` or `#`, or a keyword like `#true` (up to its name, which is plain text).
    fn string(&mut self) {
        let hashes = self.text_at().len() - self.text_at().trim_start_matches('#').len();
        self.pos += hashes;
        let quotes = if self.text_at().starts_with(r#"""""#) {
            r#"""""#
        } else {
            "\""
        };
        if !self.eat(quotes) {
            return;
        }
        let close = format!("{quotes}{}", "#".repeat(hashes));
        if hashes > 0 {
            // Raw: no escapes.
            self.pos += self
                .text_at()
                .find(&close)
                .map_or(self.text_at().len(), |i| i + close.len());
            return;
        }
        while self.peek().is_some() && !self.eat(&close) {
            // A `\` escapes what follows it.
            self.eat("\\");
            self.bump();
        }
    }

    /// The node after a `/-`, up to its terminator: a `;` is the node's, a line ending isn't.
    fn dashed_node(&mut self) {
        let mut depth = 0usize;
        // A `/-` or `\` reaches over line endings.
        let mut dashed = true;
        let mut escline = false;
        while let Some(c) = self.peek() {
            if NEWLINES.contains(&c) {
                if depth == 0 && !dashed && !escline {
                    return;
                }
                escline = false;
                self.bump();
            } else if is_space(c) {
                self.bump();
            } else if self.eat("//") {
                self.line_comment();
            } else if self.text_at().starts_with("/*") {
                self.block_comment();
            } else if self.eat("/-") {
                dashed = true;
            } else if self.eat("\\") {
                escline = true;
            } else {
                dashed = false;
                escline = false;
                match c {
                    '"' | '#' => self.string(),
                    ';' if depth == 0 => {
                        self.bump();
                        return;
                    }
                    '}' if depth == 0 => return,
                    '{' => {
                        depth += 1;
                        self.bump();
                    }
                    '}' => {
                        depth -= 1;
                        self.bump();
                    }
                    _ => self.bump(),
                }
            }
        }
    }
}

/// The blank lines in `text`, as the byte ranges they take, line endings included. `line_start` says whether the text
/// starts a line. If it doesn't, its first line is the rest of another one, and never counts. Nor does a blank line
/// inside a comment or a slashdashed node.
fn blank_lines(text: &str, line_start: bool) -> Vec<std::ops::Range<usize>> {
    let pieces = multi_line_pieces(text);
    let mut out = Vec::new();
    let mut at = 0;
    for (i, (line, ending)) in lines(text).into_iter().enumerate() {
        let end = at + line.len() + ending.len();
        if !ending.is_empty()
            && (i > 0 || line_start)
            && line.chars().all(is_space)
            && !pieces.iter().any(|p| p.start < end && at < p.end)
        {
            out.push(at..end);
        }
        at = end;
    }
    out
}

/// The length of the gap at the start of a node's leading text: everything up to the end of its last blank line. The
/// gap belongs to the block, where it stays; the rest, the comments right above the node and its indent, is the node's
/// own and goes where the node goes.
fn gap_len(leading: &str, line_start: bool) -> usize {
    let blanks = blank_lines(leading, line_start).last().map_or(0, |r| r.end);
    // The rest of the line before a node that doesn't start one belongs to what's before it.
    let rest = if line_start {
        0
    } else {
        line_rest(leading).map_or(0, str::len)
    };
    blanks.max(rest)
}

/// `gap` without the blank lines it ends with.
fn without_closing_blanks(gap: &str, line_start: bool) -> &str {
    let mut start = gap.len();
    for r in blank_lines(gap, line_start).iter().rev() {
        if r.end != start {
            break;
        }
        start = r.start;
    }
    gap.get(..start).unwrap_or(gap)
}

/// A line of whitespace and comments, which can hold slashdashed values: strings, whose lines are content.
struct Line<'a> {
    text: &'a str,
    ending: &'a str,
    /// Whether the line starts inside a string.
    in_string: bool,
    /// Whether its line ending is inside one.
    ends_in_string: bool,
}

/// The lines of `text`, as `lines` gives them, with where strings are.
fn text_lines(text: &str) -> Vec<Line<'_>> {
    let strings = if text.contains(NEWLINES) {
        string_ranges(text)
    } else {
        Vec::new()
    };
    // Asked in order, so strings behind are dropped: a text can have many lines and many strings.
    let mut strings = strings.into_iter().peekable();
    let mut inside = |at: usize| {
        while strings.next_if(|r| r.end <= at).is_some() {}
        strings.peek().is_some_and(|r| r.start < at)
    };
    let mut at = 0;
    lines(text)
        .into_iter()
        .map(|(line, ending)| {
            let out = Line {
                text: line,
                ending,
                in_string: inside(at),
                ends_in_string: inside(at + line.len()),
            };
            at += line.len() + ending.len();
            out
        })
        .collect()
}

/// `text` with its line endings in the style `eol`, but for those in strings.
fn restyle(text: &str, eol: &str) -> String {
    if !text.contains(NEWLINES) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for line in text_lines(text) {
        out.push_str(line.text);
        out.push_str(if line.ends_in_string || line.ending.is_empty() {
            line.ending
        } else {
            eol
        });
    }
    out
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

/// The style of the first line ending in a block's text. Entries are skipped: their only line endings are in `\`
/// continuations.
fn first_eol(block: &KdlDocument) -> Option<&'static str> {
    enum Part<'a> {
        Text(&'a str),
        Block(&'a KdlDocument),
        Node(&'a KdlNode),
    }
    // The parts still to read, next on top. A work list rather than recursion, here and in the other walks below:
    // documents can nest deeper than the caller's stack goes.
    let mut pending = vec![Part::Block(block)];
    while let Some(part) = pending.pop() {
        match part {
            Part::Text(text) => {
                if let Some(eol) = eol_in(text) {
                    return Some(eol);
                }
            }
            Part::Block(block) => {
                let f = block.format();
                pending.extend(f.map(|f| Part::Text(&f.trailing)));
                pending.extend(block.nodes().iter().rev().map(Part::Node));
                pending.extend(f.map(|f| Part::Text(&f.leading)));
            }
            Part::Node(node) => {
                let f = node.format();
                if let Some(f) = f {
                    pending.extend([&f.trailing, &f.terminator, &f.before_terminator].map(|t| Part::Text(t)));
                }
                pending.extend(node.children().map(Part::Block));
                if let Some(f) = f {
                    pending.extend([&f.before_children, &f.leading].map(|t| Part::Text(t)));
                }
            }
        }
    }
    None
}

/// The line ending style of a parsed document: its first line ending, or `\n` if it has none.
pub fn line_ending(doc: &KdlDocument) -> &'static str {
    first_eol(doc).unwrap_or("\n")
}

/// Whether a node's text ends its line.
fn ends_own_line(node: &KdlNode) -> bool {
    node.format()
        .is_some_and(|f| ends_line(&format!("{}{}", f.terminator, f.trailing)))
}

/// One level of indentation in `block`, whose parent's line is indented `indent`: what the first node that starts a
/// line further in adds to it. `top` is for the document itself, whose nodes may all be indented.
fn unit_in<'a>(block: &'a KdlDocument, indent: &'a str, top: bool) -> Option<String> {
    /// A block being read: its nodes still to read, and its parent's line's indent.
    struct Level<'a> {
        nodes: std::slice::Iter<'a, KdlNode>,
        indent: &'a str,
        top: bool,
        line_start: bool,
    }
    let mut levels = vec![Level {
        nodes: block.nodes().iter(),
        indent,
        top,
        line_start: top,
    }];
    while let Some(level) = levels.last_mut() {
        let Some(node) = level.nodes.next() else {
            levels.pop();
            continue;
        };
        let lead = leading(node);
        let starts = level.line_start || lead.contains(NEWLINES);
        let own = if starts { indent_of(lead) } else { level.indent };
        if starts
            && !level.top
            && let Some(unit) = own.strip_prefix(level.indent).filter(|unit| !unit.is_empty())
        {
            return Some(unit.to_owned());
        }
        level.line_start = ends_own_line(node);
        if let Some(block) = node.children() {
            levels.push(Level {
                nodes: block.nodes().iter(),
                indent: own,
                top: false,
                line_start: false,
            });
        }
    }
    None
}

/// One level of indentation in the document.
fn indent_unit(doc: &KdlDocument) -> String {
    unit_in(doc, "", true).unwrap_or_else(|| "    ".to_owned())
}

/// How a block lays out its nodes.
pub struct Style {
    eol: &'static str,
    /// For a children block, the parent's indent. None at the top level.
    parent: Option<String>,
    /// One level of indentation in the document.
    unit: String,
}

impl Style {
    /// The style of the children block of the node at `parent`, or of the top level if that's None. The block must
    /// exist. `eol` is the document's line ending style, for a block that has none of its own.
    pub fn of(doc: &KdlDocument, parent: Option<&[usize]>, eol: &'static str) -> Style {
        let parent = parent.map(|path| node_at(doc, path));
        let block = parent
            .map_or(Some(doc), KdlNode::children)
            .expect("the parent has a block");
        Style {
            eol: first_eol(block).unwrap_or(eol),
            parent: parent.map(|p| indent_of(leading(p)).to_owned()),
            unit: indent_unit(doc),
        }
    }
}

/// Moves text to another indentation: a line indented `from` plus some levels of `from_unit` gets `to` plus as many
/// levels of `to_unit`. Its line endings take the style `eol`.
struct Shift {
    from: String,
    to: String,
    from_unit: String,
    to_unit: String,
    eol: &'static str,
}

impl Shift {
    /// The move of `node` to indent `to`, in a block of `style`.
    fn of(node: &KdlNode, to: &str, style: &Style) -> Shift {
        let from = indent_of(leading(node)).to_owned();
        let from_unit = node
            .children()
            .and_then(|block| unit_in(block, &from, false))
            .unwrap_or_default();
        Shift {
            from,
            to: to.to_owned(),
            from_unit,
            to_unit: style.unit.clone(),
            eol: style.eol,
        }
    }

    /// Moves a node's text and its children. Its leading is the caller's.
    fn move_node(&self, node: &mut KdlNode) {
        self.move_own_text(node);
        reindent_children(node, self);
    }

    /// Moves the lines in a node's own text: `\` continuations, and comments and slashdashed parts after its name.
    /// What follows its terminator is the next line's start, which isn't the node's.
    fn move_own_text(&self, node: &mut KdlNode) {
        edit_own_text(node, |text, before_terminator| {
            if before_terminator {
                self.apply(text, false)
            } else {
                restyle(text, self.eol)
            }
        });
    }

    /// The new indentation for a line indented `spaces`, if it's indented `from` at least.
    fn indent(&self, spaces: &str) -> Option<String> {
        let mut rest = spaces.strip_prefix(self.from.as_str())?;
        let mut out = self.to.clone();
        if !self.from_unit.is_empty() {
            while let Some(after) = rest.strip_prefix(self.from_unit.as_str()) {
                out.push_str(&self.to_unit);
                rest = after;
            }
        }
        out.push_str(rest);
        Some(out)
    }

    /// `text`, moved. `line_start` says whether the text itself starts a line. Blank lines stay blank, and lines in
    /// strings stay as they are.
    fn apply(&self, text: &str, line_start: bool) -> String {
        let mut out = String::with_capacity(text.len());
        for (i, line) in text_lines(text).into_iter().enumerate() {
            let blank = line.text.is_empty() && !line.ending.is_empty();
            let content = line.text.trim_start_matches(is_space);
            let spaces = line.text.strip_suffix(content).unwrap_or_default();
            match self.indent(spaces) {
                Some(indent) if (i > 0 || line_start) && !blank && !line.in_string => {
                    out.push_str(&indent);
                    out.push_str(content);
                }
                _ => out.push_str(line.text),
            }
            out.push_str(if line.ends_in_string || line.ending.is_empty() {
                line.ending
            } else {
                self.eol
            });
        }
        out
    }
}

/// Rewrites the text of a node apart from its leading, its children and its values: the whitespace around its
/// entries, where `\` continuations end their lines, and comments and slashdashed parts. `edit` also gets whether the
/// text comes before the terminator.
fn edit_own_text(node: &mut KdlNode, mut edit: impl FnMut(&str, bool) -> String) {
    crate::parser::annotation::editing(node, |node| {
        if let Some(f) = node.format_mut() {
            for text in [
                &mut f.before_ty_name,
                &mut f.after_ty_name,
                &mut f.after_ty,
                &mut f.before_children,
                &mut f.before_terminator,
            ] {
                *text = edit(text, true);
            }
            for text in [&mut f.terminator, &mut f.trailing] {
                *text = edit(text, false);
            }
        }
    });
    for entry in node.entries_mut() {
        if let Some(f) = entry.format_mut() {
            for text in [
                &mut f.leading,
                &mut f.trailing,
                &mut f.before_ty_name,
                &mut f.after_ty_name,
                &mut f.after_ty,
                &mut f.after_key,
                &mut f.after_eq,
            ] {
                *text = edit(text, true);
            }
        }
    }
}

/// Line endings in a node's own text.
fn restyle_node(node: &mut KdlNode, eol: &str) {
    edit_own_text(node, |text, _| restyle(text, eol));
}

/// Puts every line ending in the document's whitespace and comments in the style `eol`. Line endings inside values
/// stay as they are.
pub fn restyle_document(doc: &mut KdlDocument, eol: &str) {
    let mut pending = vec![doc];
    while let Some(block) = pending.pop() {
        if let Some(f) = block.format_mut() {
            f.leading = restyle(&f.leading, eol);
            f.trailing = restyle(&f.trailing, eol);
        }
        for node in block.nodes_mut() {
            if let Some(f) = node.format_mut() {
                f.leading = restyle(&f.leading, eol);
            }
            restyle_node(node, eol);
        }
        pending.extend(block.nodes_mut().iter_mut().filter_map(|n| n.children_mut().as_mut()));
    }
}

/// Moves everything under `node` to its new indentation and line ending style.
fn reindent_children(node: &mut KdlNode, shift: &Shift) {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        let Some(block) = node.children_mut() else {
            continue;
        };
        let mut line_start = false;
        if let Some(f) = block.format_mut() {
            f.leading = shift.apply(&f.leading, false);
            // Spaces after the last line ending were reindented already, as the start of that line. Only an empty
            // block has more text on that line: its trailing.
            line_start = f.leading.ends_with(NEWLINES);
        }
        for child in block.nodes_mut() {
            if let Some(f) = child.format_mut() {
                f.leading = shift.apply(&f.leading, line_start);
                line_start = ends_line(&format!("{}{}", f.terminator, f.trailing));
            }
            shift.move_own_text(child);
        }
        if let Some(f) = block.format_mut() {
            f.trailing = shift.apply(&f.trailing, line_start);
        }
        pending.extend(block.nodes_mut());
    }
}

/// Whether the node ends in a `\` continuation, which would carry it onto whatever comes next: None if it doesn't,
/// else whether the continuation's line ending is in its text.
fn open_escline(node: &KdlNode) -> Option<bool> {
    node.format()
        .filter(|f| f.terminator.is_empty() && f.trailing.is_empty())
        .and_then(|_| continuation(&last_part(node)))
}

/// Finishes an open `\` continuation and ends the node with `;` on the line after, at `indent`, which keeps the old
/// text as it was.
fn close_escline(node: &mut KdlNode, eol: &str, indent: &str) {
    if let Some(ended) = open_escline(node) {
        let f = fmt_mut(node, eol);
        f.before_terminator
            .truncate(f.before_terminator.trim_end_matches(is_space).len());
        if !ended {
            f.before_terminator.push_str(eol);
        }
        f.before_terminator.push_str(indent);
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

/// The text after the node at `index`: the next node's leading, or the block's trailing.
fn after(block: &KdlDocument, index: usize) -> &str {
    match block.nodes().get(index + 1) {
        Some(next) => leading(next),
        None => block.format().map_or("", |f| &f.trailing),
    }
}

/// The rest of the line after the node at `index`, if the node ends in a `;` and only spaces and comments follow it
/// there. It's the node's, though kdl-rs keeps it in the text after it. `top` says whether the block is the
/// document's.
fn semicolon_rest(block: &KdlDocument, index: usize, top: bool) -> Option<&str> {
    let node = block.nodes().get(index)?;
    let semicolon = node.format().is_some_and(|f| f.terminator == ";");
    if !semicolon || ends_own_line(node) {
        return None;
    }
    line_rest_in(after(block, index), top && index + 1 == block.nodes().len())
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

/// Takes the gap off the leading of the node at `index`, if there's one there, and returns it.
fn split_gap(block: &mut KdlDocument, index: usize, nested: bool) -> String {
    let start = line_start_at(block, index, nested);
    let Some(f) = block.nodes_mut().get_mut(index).and_then(KdlNode::format_mut) else {
        return String::new();
    };
    let own = f.leading.split_off(gap_len(&f.leading, start));
    std::mem::replace(&mut f.leading, own)
}

/// Puts back the gap of a node taken out at `index`, in front of what followed it: the next node's leading, or the end
/// of the block. If what follows starts with a blank line, or is the end of the block with nothing else in it, the
/// blank lines at the end of the gap go. So removing a node between two blank lines leaves one, and removing the last
/// node leaves no blank line at the end. In a block left empty, the gap goes at the start, so new nodes go below it.
fn rejoin_gap(block: &mut KdlDocument, index: usize, gap: &str, line_start: bool, eol: &str) {
    if gap.is_empty() {
        return;
    }
    let after = match block.nodes().get(index) {
        Some(next) => leading(next),
        None => block.format().map_or("", |f| &f.trailing),
    };
    let at_end = index == block.nodes().len();
    let gap = if blank_rest(after).is_some() || at_end && is_blank(after) {
        without_closing_blanks(gap, line_start)
    } else {
        gap
    };
    if block.nodes().is_empty() {
        block_fmt_mut(block).leading.push_str(gap);
    } else {
        after_mut(block, index, eol).insert_str(0, gap);
    }
}

/// Takes the gap off `copy`, a copy of the node at `path` in `doc`: only the node's own comments go with it, the ones
/// after its `;` included.
pub fn drop_gap(doc: &KdlDocument, path: &[usize], copy: &mut KdlNode) {
    let (&index, parent) = path.split_last().expect("a node's path isn't empty");
    let block = if parent.is_empty() {
        doc
    } else {
        node_at(doc, parent).children().expect("a parent has a block")
    };
    let start = line_start_at(block, index, !parent.is_empty());
    if let Some(f) = copy.format_mut() {
        let gap = gap_len(&f.leading, start);
        f.leading.replace_range(..gap, "");
        // Where `take` puts them, so they go back after the `;` when the copy is placed.
        if let Some(rest) = semicolon_rest(block, index, parent.is_empty()).filter(|rest| !is_blank(rest)) {
            f.trailing.push_str(rest);
        }
    }
}

/// Whether a children block is written on one line, like `a { b; c }`.
fn is_single_line(block: &KdlDocument) -> bool {
    !block.nodes().is_empty() && !block_breaks(block)
}

/// Whether `node` can go into a single-line block: its text, apart from the whitespace around it, has no line
/// endings, and it ends with at most a `;`.
fn fits_on_a_line(node: &KdlNode) -> bool {
    let Some(f) = node.format() else {
        return false;
    };
    let tail = format!("{}{}", f.terminator, f.trailing);
    !f.leading.trim_start().contains(NEWLINES)
        && !own_text_breaks(node)
        && !node
            .children()
            .is_some_and(|block| block.format().is_none() || block_breaks(block))
        && continuation(&last_part(node)).is_none()
        && matches!(tail.trim(), "" | ";")
}

// The checks below read the parts kdl-rs prints rather than printing them: its printing recurses once per level.

/// Whether a block's text has a line ending, its nodes' included.
fn block_breaks(block: &KdlDocument) -> bool {
    let mut blocks = vec![block];
    while let Some(block) = blocks.pop() {
        if block
            .format()
            .is_some_and(|f| f.leading.contains(NEWLINES) || f.trailing.contains(NEWLINES))
        {
            return true;
        }
        for node in block.nodes() {
            let ends = node.format().is_some_and(|f| {
                [&f.leading, &f.terminator, &f.trailing]
                    .iter()
                    .any(|t| t.contains(NEWLINES))
            });
            if ends || own_text_breaks(node) {
                return true;
            }
            if let Some(children) = node.children() {
                // kdl-rs starts a new line after the `{` of a block without a format.
                if children.format().is_none() {
                    return true;
                }
                blocks.push(children);
            }
        }
    }
    false
}

/// Whether a node's own text has a line ending: everything but its leading text, its children, its terminator and its
/// trailing text.
fn own_text_breaks(node: &KdlNode) -> bool {
    let f = node.format();
    let format_breaks = f.is_some_and(|f| {
        [
            &f.before_ty_name,
            &f.after_ty_name,
            &f.after_ty,
            &f.before_children,
            &f.before_terminator,
        ]
        .iter()
        .any(|t| t.contains(NEWLINES))
    });
    format_breaks
        || node.name().to_string().contains(NEWLINES)
        || node.ty().is_some_and(|ty| ty.to_string().contains(NEWLINES))
        || node.entries().iter().any(|e| e.to_string().contains(NEWLINES))
}

/// The last part of a node's text that isn't all spaces, before its terminator.
fn last_part(node: &KdlNode) -> String {
    let bt = node.format().map_or("", |f| f.before_terminator.as_str());
    if !bt.trim_end_matches(is_space).is_empty() {
        return bt.to_owned();
    }
    if node.children().is_some() {
        return "}".to_owned();
    }
    let entries = node.entries().iter().rev().map(ToString::to_string);
    entries
        .chain(std::iter::once(node.name().to_string()))
        .find(|text| !text.trim_end_matches(is_space).is_empty())
        .unwrap_or_default()
}

/// The whitespace a node's leading starts with.
fn space_before(node: &KdlNode) -> String {
    let leading = leading(node);
    leading
        .strip_suffix(leading.trim_start_matches(is_space))
        .unwrap_or_default()
        .to_owned()
}

/// Splits the spaces off the end of a node's `before_terminator`. At the end of a single-line block, they're the space
/// before the block's `}`.
fn split_closing(node: &mut KdlNode) -> String {
    node.format_mut().map_or_else(String::new, |f| {
        let content = f.before_terminator.trim_end_matches(is_space).len();
        f.before_terminator.split_off(content)
    })
}

/// Whether nothing follows the node's text: it ends at the parent's `}` or the end of the document. Only the last node
/// of a block can, since every other one ends with a line ending or a `;`.
fn ends_open(node: &KdlNode) -> bool {
    node.format()
        .is_some_and(|f| f.terminator.is_empty() && f.trailing.is_empty())
}

/// Gives `node` a place in a single-line block: `space` before it, then its `terminator`, with `closing` in between
/// if it's the last node.
fn fit_inline(node: &mut KdlNode, space: &str, closing: &str, terminator: String) {
    split_closing(node);
    let f = fmt_mut(node, "\n");
    f.leading = format!("{space}{}", f.leading.trim_start());
    f.before_terminator.push_str(closing);
    f.terminator = terminator;
    f.trailing.clear();
}

/// Ends the node at `index` with `;` if it has a block and the space before the parent's `}` follows it. That space
/// would sit right after its own `}`, which kdl-rs drops when it reads the text back. After the
/// `;`, the space belongs to the block.
fn semicolon_after_block(block: &mut KdlDocument, index: usize) {
    let Some(node) = block.nodes_mut().get_mut(index) else {
        return;
    };
    if node.children().is_none() || !ends_open(node) {
        return;
    }
    let closing = split_closing(node);
    if closing.is_empty() {
        return;
    }
    fmt_mut(node, "\n").terminator = ";".into();
    block_fmt_mut(block).trailing.insert_str(0, &closing);
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
        let closing = split_closing(prev);
        let f = fmt_mut(prev, "\n");
        (space, std::mem::replace(&mut f.terminator, ";".into()), closing)
    };
    fit_inline(&mut node, &space, &closing, terminator);
    nodes.insert(index, node);
    semicolon_after_block(block, index);
}

/// `unplace` for a single-line block. The node before keeps its `;`, so its text stays as it was.
fn unplace_inline(block: &mut KdlDocument, index: usize) -> KdlNode {
    let mut node = block.nodes_mut().remove(index);
    // The space before `}` belongs to the block, so it stays behind.
    let closing = split_closing(&mut node);
    if index == block.nodes().len() && index > 0 {
        block_fmt_mut(block).trailing.insert_str(0, &closing);
    }
    node
}

/// `replace` for a single-line block: the new node takes the old one's place on the line.
fn replace_inline(block: &mut KdlDocument, index: usize, mut node: KdlNode) -> KdlNode {
    let old = block.nodes_mut().get_mut(index).expect("replace checks the index");
    let space = space_before(old);
    let closing = split_closing(old);
    let terminator = fmt_mut(old, "\n").terminator.clone();
    fit_inline(&mut node, &space, &closing, terminator);
    let old = std::mem::replace(old, node);
    semicolon_after_block(block, index);
    old
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
    // `}` lines up with the parent, below any comments before it.
    let comments = f.trailing.trim();
    f.trailing = if comments.is_empty() {
        parent_indent.to_owned()
    } else {
        format!("{parent_indent}{unit}{comments}{eol}{parent_indent}")
    };
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

/// Puts the block's `}` on a line of its own, lined up with the parent, if it was on the line of a node that now
/// ends its line.
fn close_on_own_line(block: &mut KdlDocument, style: &Style) {
    if let Some(parent) = &style.parent {
        let f = block_fmt_mut(block);
        if is_blank(&f.trailing) && !f.trailing.contains(NEWLINES) {
            parent.clone_into(&mut f.trailing);
        }
    }
}

/// Ends the line of the node at `index`: with `rest`, the rest of its line taken from the text after it, or else with
/// a line ending of its own.
fn end_line(block: &mut KdlDocument, index: usize, rest: Option<String>, style: &Style) {
    let last = index + 1 == block.nodes().len();
    let node = block.nodes_mut().get_mut(index).expect("the caller checks the index");
    match rest {
        Some(rest) => fmt_mut(node, style.eol).trailing.push_str(&rest),
        None if ends_open(node) => {
            // Spaces that came before a `}` would now end the line.
            split_closing(node);
            fmt_mut(node, style.eol).terminator = style.eol.into();
            if last {
                close_on_own_line(block, style);
            }
        }
        None => fmt_mut(node, style.eol).trailing.push_str(style.eol),
    }
}

/// Puts `node` into `block` at `index`, laid out like its neighbours. It goes below the gap of the node at `index`
/// and above that node's own comments.
pub fn place(block: &mut KdlDocument, index: usize, node: KdlNode, style: &Style) {
    place_at(block, index, node, style, None, None);
}

/// `place`, for a node that takes the place of one indented `replacing` if that's Some. `gap` is the gap to put above
/// the node, if it's Some, instead of the one of the node at `index`.
fn place_at(
    block: &mut KdlDocument,
    index: usize,
    mut node: KdlNode,
    style: &Style,
    replacing: Option<String>,
    gap: Option<String>,
) {
    let eol = style.eol;
    if let Some(parent) = &style.parent
        && is_single_line(block)
    {
        if fits_on_a_line(&node) {
            place_inline(block, index, node);
            return;
        }
        spread(block, parent, &style.unit, eol);
    }

    let indent = match (&style.parent, replacing) {
        (_, Some(indent)) => indent,
        (None, None) => String::new(),
        (Some(parent), None) => {
            if block.nodes().is_empty() {
                // The closing `}` lines up with the parent.
                let f = block_fmt_mut(block);
                if is_blank(&f.leading) && is_blank(&f.trailing) {
                    f.leading.clear();
                    parent.clone_into(&mut f.trailing);
                }
            }
            sibling_indent(block, index).unwrap_or_else(|| format!("{parent}{}", style.unit))
        }
    };
    if let Some(prev) = index.checked_sub(1).and_then(|i| block.nodes_mut().get_mut(i)) {
        close_escline(prev, eol, &indent);
    }
    // Below the gap, which ends a line.
    let gap = gap.unwrap_or_else(|| split_gap(block, index, style.parent.is_some()));
    let mut start = String::new();
    if gap.is_empty() && !line_start_at(block, index, style.parent.is_some()) {
        let next = index < block.nodes().len();
        let file_end = !next && style.parent.is_none();
        let after = after_mut(block, index, eol);
        let mut rest = line_rest_in(after, file_end).map(str::to_owned);
        match &rest {
            Some(rest) => after.replace_range(..rest.len(), ""),
            None if !next => close_on_own_line(block, style),
            // A node that went on on the old line starts the line after the new node. Text before it that reaches
            // over a line ending, a `\` or a comment, stays as it is.
            None if !after.contains(NEWLINES) => {
                *after = format!("{indent}{}", after.trim_start_matches(is_space));
            }
            None => {}
        }
        if let Some(rest) = &mut rest
            && !ends_line(rest)
        {
            // The end of the file.
            rest.push_str(eol);
        }
        match index.checked_sub(1) {
            Some(prev) if rest.as_deref().is_none_or(is_blank) => end_line(block, prev, rest, style),
            // The line break after `{` is the first node's, as kdl-rs reads it. So is one after a `;` with comments
            // before it: kdl-rs's autoformat loses comments anywhere else.
            _ => {
                start = rest.unwrap_or_else(|| eol.to_owned());
                if index == 0 {
                    let f = block_fmt_mut(block);
                    f.leading.truncate(f.leading.trim_end_matches(is_space).len());
                }
            }
        }
    }

    let escline = open_escline(&node).is_some();
    let shift = Shift::of(&node, &indent, style);
    shift.move_node(&mut node);
    let f = fmt_mut(&mut node, eol);
    // Keep the node's own comments, but not the whitespace it had where it came from.
    let own = shift.apply(f.leading.trim_start(), false);
    f.leading = format!("{gap}{start}{indent}{own}");
    // The comments on the line of a node that ends in `;` go after it, where kdl-rs reads them, for the same reason.
    let rest = (f.terminator == ";" && !is_blank(&f.trailing)).then(|| std::mem::take(&mut f.trailing));
    if is_blank(&f.trailing) {
        f.trailing.clear();
    }
    if let Some(mut rest) = rest {
        if !ends_line(&rest) {
            // It was at the end of a file.
            rest.push_str(eol);
        }
        after_mut(block, index, eol).insert_str(0, &rest);
    } else if escline {
        close_escline(&mut node, eol, &indent);
        fmt_mut(&mut node, eol).trailing = eol.into();
    } else if !ends_own_line(&node) {
        let f = fmt_mut(&mut node, eol);
        if f.terminator.starts_with("//") {
            // A comment at the end of the file ends the node too, and keeps it.
            f.terminator.push_str(eol);
        } else if f.trailing.is_empty() && !ends_line(&f.before_terminator) {
            // Spaces that came before a `}` or a `;` would now end the line.
            split_closing(&mut node);
            fmt_mut(&mut node, eol).terminator = eol.into();
        } else {
            // A `;` after a `\` continuation's line ending is what ends the node, so it stays.
            f.trailing.push_str(eol);
        }
    }
    block.nodes_mut().insert(index, node);
}

/// Takes the node at `index` out of `block`, the inverse of `place`. An emptied children block prints as `{}`.
pub fn unplace(block: &mut KdlDocument, index: usize, style: &Style) -> KdlNode {
    let node = take(block, index, style);
    if style.parent.is_some()
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

/// Takes the node at `index` out of `block`, with its own line ending and its own comments. Whatever followed it keeps
/// starting a line if it did, and the node's gap stays in front of it.
fn take(block: &mut KdlDocument, index: usize, style: &Style) -> KdlNode {
    let (node, gap, line_start) = take_own(block, index, style);
    rejoin_gap(block, index, &gap, line_start, style.eol);
    node
}

/// `take`, but the gap is left out of the block: returns it, and whether it starts a line.
fn take_own(block: &mut KdlDocument, index: usize, style: &Style) -> (KdlNode, String, bool) {
    let nested = style.parent.is_some();
    if nested && is_single_line(block) {
        return (unplace_inline(block, index), String::new(), false);
    }
    let line_start = line_start_at(block, index, nested);
    let gap = split_gap(block, index, nested);
    // The node's own text starts a line after a gap, which ends one.
    let started = line_start || !gap.is_empty();
    let rest = semicolon_rest(block, index, !nested).map(str::to_owned);
    let mut node = block.nodes_mut().remove(index);
    let next = index < block.nodes().len();
    let mut ends = ends_own_line(&node);
    // At the end of the file, nothing follows to keep starting a line.
    let mut at_end = false;
    if let Some(rest) = rest {
        after_mut(block, index, style.eol).replace_range(..rest.len(), "");
        at_end = !ends_line(&rest);
        if !is_blank(&rest) {
            fmt_mut(&mut node, style.eol).trailing.push_str(&rest);
        }
        ends = true;
    }
    let lead = leading(&node);
    if !ends && next && (started || lead.contains(NEWLINES)) {
        // The next node was on the node's line, and takes over its start.
        let start = match (started, index.checked_sub(1)) {
            (true, _) => "",
            (false, Some(prev)) => {
                end_line(block, prev, None, style);
                ""
            }
            (false, None) => blank_rest(lead).unwrap_or(style.eol),
        };
        let next = fmt_mut(block.nodes_mut().get_mut(index).expect("checked"), style.eol);
        next.leading = format!(
            "{start}{}{}",
            indent_of(lead),
            next.leading.trim_start_matches(is_space)
        );
    } else if !ends && !next && started && !block.nodes().is_empty() {
        close_on_own_line(block, style);
    } else if ends && !started && !at_end {
        // What followed the node started a line, and still does. The line break goes to what came before.
        if let Some(prev) = index.checked_sub(1) {
            end_line(block, prev, None, style);
        } else {
            let ending = blank_rest(lead).unwrap_or(style.eol).to_owned();
            after_mut(block, index, style.eol).insert_str(0, &ending);
        }
    }
    (node, gap, line_start)
}

/// Puts `node` in place of the node at `index`, and returns that one. The new node takes the old one's place: in a
/// single-line block, between the same neighbours, otherwise its whitespace and the end of its line, as `reverse` does.
pub fn replace(block: &mut KdlDocument, index: usize, mut node: KdlNode, style: &Style) -> KdlNode {
    let nested = style.parent.is_some();
    if nested && is_single_line(block) && fits_on_a_line(&node) {
        return replace_inline(block, index, node);
    }
    let old = block.nodes().get(index).expect("replace checks the index");
    let starts = line_start_at(block, index, nested) || leading(old).contains(NEWLINES);
    let (slot, old_own) = slot_and_own(block, index, !nested);
    // A node carrying the comments from the end of its old line goes through `place`, which puts them after it.
    let carries_comments = node.format().is_some_and(|f| !is_blank(&f.trailing));
    if starts && !carries_comments && fits_slot(&own_of(&node), &slot, node.children().is_some()) {
        let shift = Shift::of(&node, indent_of(leading(old)), style);
        shift.move_node(&mut node);
        let mut own = own_of(&node);
        own.lead = shift.apply(&own.lead, false);
        let open_end = slot.open_end;
        let after = put_in_slot(&mut node, slot, own);
        if open_end {
            // The comments after the old node's `;` are its own, and go with it.
            let text = after_mut(block, index + 1, style.eol);
            *text = join_comments(&after, text.get(old_own.after.len()..).unwrap_or_default());
        }
        // The gap is in the slot. The old node keeps only its own comments.
        split_gap(block, index, nested);
        let old = block.nodes_mut().get_mut(index).expect("replace checks the index");
        if open_end {
            fmt_mut(old, style.eol).trailing.push_str(&old_own.after);
        }
        return std::mem::replace(old, node);
    }
    let indent = starts.then(|| indent_of(leading(old)).to_owned());
    // The new node goes below the old one's gap, not the next one's.
    let (old, gap, _) = take_own(block, index, style);
    place_at(block, index, node, style, indent, Some(gap));
    old
}

/// A node's place in its block: the whitespace and the gap before it, and how its line goes on after it.
struct Slot {
    /// The comments after the `;` of the node before, which kdl-rs keeps at the start of this node's leading.
    before: String,
    lead: String,
    /// Spaces before the terminator.
    closing: String,
    /// Whether the node there is followed by its terminator, or by the end of the block. A parsed node in the middle
    /// of a block always is: rkdl puts back the terminator kdl-rs drops after a `}` and spaces.
    ends: bool,
    terminator: String,
    trailing: String,
    /// Whether the line goes on after the `;` with only spaces and comments, which the text after the node holds.
    open_end: bool,
    /// Whether the end of the file follows, which ends the line too.
    file_end: bool,
    /// Whether the place starts a line, so the comment lines above a node can go there.
    line_start: bool,
}

/// What belongs to a node apart from its text: the comments right above it, below its gap, with the indent of its line
/// after them, and the comments after it on its line.
struct Own {
    lead: String,
    tail: String,
    /// The comments that end the node's line, with the spaces before them: what's after its `;`, or else the ones
    /// before its terminator.
    after: String,
}

/// Splits the node at `index` from its place. `top` says whether the block is the document's.
fn slot_and_own(block: &KdlDocument, index: usize, top: bool) -> (Slot, Own) {
    let node = block.nodes().get(index).expect("the caller checks the index");
    let prev = index.checked_sub(1);
    let before = prev.and_then(|prev| semicolon_rest(block, prev, top));
    let rest = semicolon_rest(block, index, top);
    let last = index + 1 == block.nodes().len();
    let (mut slot, own) = split_node(node, last, before.map_or(0, |rest| comments_in(rest).len()), rest);
    let text_after = after(block, index);
    slot.file_end = top && last && is_blank(text_after) && !text_after.contains(NEWLINES);
    // The end of the file ends the line after a `;` too.
    slot.open_end |= slot.file_end && slot.terminator == ";";
    // Whether the first place is on the line of a `{` isn't known here: it counts as a line start, as it always did.
    slot.line_start =
        prev.is_none_or(|prev| block.nodes().get(prev).is_some_and(ends_own_line)) || leading(node).contains(NEWLINES);
    (slot, own)
}

/// What's a node's own, as the last node of a block of its own.
fn own_of(node: &KdlNode) -> Own {
    split_node(node, true, 0, None).1
}

/// Splits a node's whitespace from what's its own. `last` says whether it's the last node of its block, `before` how
/// much of its leading is the comments of the node before it, and `rest` is the rest of its line after its `;`.
fn split_node(node: &KdlNode, last: bool, before: usize, rest: Option<&str>) -> (Slot, Own) {
    let (lead, bt, terminator, trailing) = node.format().map_or(("", "", "", ""), |f| {
        (
            f.leading.as_str(),
            f.before_terminator.as_str(),
            f.terminator.as_str(),
            f.trailing.as_str(),
        )
    });
    let (before, lead) = lead.split_at_checked(before).unwrap_or(("", lead));
    // Whether the leading starts a line only changes whether its first line counts as blank. It's whitespace either way,
    // so it's the slot's.
    let lead_rest = lead.get(gap_len(lead, true)..).unwrap_or_default();
    let comments = lead_rest.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let space = lead.strip_suffix(comments).unwrap_or_default();
    let semicolon = terminator == ";";
    // After a `;`, only the comments after it end the line: the ones before it are the node's text.
    let (tail, ending) = bt.split_at(if semicolon {
        bt.trim_end_matches(is_space).len()
    } else {
        closing_comments_at(bt)
    });
    let (closing, after, terminator) = if terminator.starts_with("//") {
        let comment = terminator.split(NEWLINES).next().unwrap_or_default();
        let line_end = terminator.strip_prefix(comment).unwrap_or_default();
        ("", format!("{ending}{comment}"), line_end)
    } else {
        let comments = ending.trim_end_matches(is_space);
        let spaces = ending.strip_prefix(comments).unwrap_or_default();
        let after = if semicolon {
            rest.map_or("", comments_in)
        } else {
            comments
        };
        (spaces, after.to_owned(), terminator)
    };
    (
        Slot {
            before: before.to_owned(),
            lead: space.to_owned(),
            closing: closing.to_owned(),
            ends: last || !terminator.is_empty() || !trailing.is_empty(),
            terminator: terminator.to_owned(),
            trailing: trailing.to_owned(),
            open_end: semicolon && rest.is_some(),
            file_end: false,
            line_start: true,
        },
        Own {
            lead: comments.to_owned(),
            tail: tail.to_owned(),
            after,
        },
    )
}

/// Whether a node with `own` can take `slot` and still read back as itself, with nothing after its `}`.
fn fits_slot(own: &Own, slot: &Slot, has_block: bool) -> bool {
    // A `//` comment has to end a line.
    let ends_line = slot.open_end || eol_at(&slot.terminator).is_some() || slot.file_end && slot.terminator.is_empty();
    let after_fits = !has_line_comment(&own.after) || ends_line;
    slot.ends
        && after_fits
        // Comment lines after a `;` would be the comments on the line of the node before.
        && (slot.line_start || line_rest(&own.lead).is_none())
        && continuation(&own.tail).is_none()
        && (!has_block || slot.closing.is_empty())
}

/// Whether a line of comments ends in a `//` comment.
fn has_line_comment(comments: &str) -> bool {
    line_comment_at(comments) < comments.len()
}

/// `comments` in front of `text`, the rest of their line. A `//` comment would take in the spaces that end the line,
/// so they go.
fn join_comments(comments: &str, text: &str) -> String {
    if has_line_comment(comments) {
        format!("{comments}{}", text.trim_start_matches(is_space))
    } else {
        format!("{comments}{text}")
    }
}

/// Gives `node` the place `slot`. Returns the comments that go after its `;`, at the start of the text after it.
fn put_in_slot(node: &mut KdlNode, slot: Slot, own: Own) -> String {
    let f = fmt_mut(node, "\n");
    f.leading = slot.before + &slot.lead + &own.lead;
    f.trailing = slot.trailing;
    if slot.open_end {
        f.before_terminator = own.tail + &slot.closing;
        f.terminator = slot.terminator;
        return own.after;
    }
    // Where kdl-rs puts them: a `//` comment starts the terminator, and takes the place of the spaces that end the line.
    let (comments, line_comment) = own.after.split_at(line_comment_at(&own.after));
    let closing = if line_comment.is_empty() {
        slot.closing.as_str()
    } else {
        ""
    };
    f.before_terminator = own.tail + comments + closing;
    f.terminator = line_comment.to_owned() + &slot.terminator;
    String::new()
}

/// Reverses the nodes of `block`. The whitespace layout stays by position, as with `permute`. Reversing twice gives
/// back the same text, but for spaces at the end of a line a `//` comment landed on.
pub fn reverse(block: &mut KdlDocument, style: &Style) {
    let count = block.nodes().len();
    let places: Vec<usize> = (0..count).collect();
    let from: Vec<usize> = (0..count).rev().collect();
    if permute_in(block, &places, &from, style.parent.is_none()) {
        return;
    }
    // A comment or `\` that needs a line ending would land where there's none. Moves the first node back to each
    // place in turn instead, last place first. The block never empties, and the node that ends up last takes over the
    // end of the block.
    for to in (1..count).rev() {
        let node = unplace(block, 0, style);
        place(block, to, node, style);
    }
}

/// Moves the node at `from[k]` to `places[k]`, for every `k`: `from` holds the same places in another order. The
/// whitespace layout stays by position: each place keeps the whitespace before it and the end of its line, and the
/// nodes move with their own comments. Changes nothing and returns false if a comment or `\` that needs a line ending
/// would land where there's none, comment lines above a node would land after a `;` on the line, or a block where
/// spaces follow.
pub fn permute(block: &mut KdlDocument, places: &[usize], from: &[usize]) -> bool {
    permute_in(block, places, from, false)
}

/// `permute`, where `top` says whether the block is the document's: then a comment can end the last place.
fn permute_in(block: &mut KdlDocument, places: &[usize], from: &[usize], top: bool) -> bool {
    let count = block.nodes().len();
    let (slots, mut owns): (Vec<Slot>, Vec<Option<Own>>) = (0..count)
        .map(|i| slot_and_own(block, i, top))
        .map(|(slot, own)| (slot, Some(own)))
        .unzip();
    let mut source: Vec<usize> = (0..count).collect();
    for (&to, &from) in places.iter().zip(from) {
        let fits = (
            slots.get(to),
            owns.get(from).and_then(Option::as_ref),
            block.nodes().get(from),
        );
        let (Some(slot), Some(own), Some(node)) = fits else {
            return false;
        };
        if to != from && !fits_slot(own, slot, node.children().is_some()) {
            return false;
        }
        if let Some(s) = source.get_mut(to) {
            *s = from;
        }
    }
    // The comments after the last node's `;`, which kdl-rs keeps in the block's trailing.
    let end = count
        .checked_sub(1)
        .and_then(|last| semicolon_rest(block, last, top))
        .map_or(0, |rest| comments_in(rest).len());
    let mut taken: Vec<Option<KdlNode>> = std::mem::take(block.nodes_mut()).into_iter().map(Some).collect();
    // The comments after the `;` of the node placed last, which go at the start of the next node's leading.
    let mut after = String::new();
    for (to, (mut slot, &from)) in slots.into_iter().zip(&source).enumerate() {
        let mut node = taken
            .get_mut(from)
            .and_then(Option::take)
            .expect("each place moves once");
        let own = owns
            .get_mut(from)
            .and_then(Option::take)
            .expect("each place moves once");
        let before = std::mem::take(&mut after);
        if from == to {
            // Only what came before it changes.
            if node.format().is_some() || !before.is_empty() {
                let f = fmt_mut(&mut node, "\n");
                f.leading = join_comments(&before, f.leading.get(slot.before.len()..).unwrap_or_default());
            }
            if slot.open_end {
                after = own.after;
            }
        } else {
            let lead = join_comments(&before, &slot.lead);
            slot.before.clear();
            slot.lead = lead;
            after = put_in_slot(&mut node, slot, own);
        }
        block.nodes_mut().push(node);
    }
    if end > 0 || !after.is_empty() {
        let f = block_fmt_mut(block);
        f.trailing = join_comments(&after, f.trailing.get(end..).unwrap_or_default());
    }
    true
}

/// Gives the node at `path` an empty children block, `{}`. `eol` is the document's line ending style.
pub fn open_block(doc: &mut KdlDocument, path: &[usize], eol: &'static str) {
    let (&index, parent) = path.split_last().expect("a node's path isn't empty");
    let block = if parent.is_empty() {
        doc
    } else {
        node_at_mut(doc, parent)
            .children_mut()
            .as_mut()
            .expect("a parent has a block")
    };
    let eol = first_eol(block).unwrap_or(eol);
    let node = block.nodes_mut().get_mut(index).expect("the path leads to a node");
    // At the end of a block, the space before the parent's `}` stays after the new `}`, so `semicolon_after_block`
    // can give it to the block.
    let closing = if !parent.is_empty() && ends_open(node) {
        split_closing(node)
    } else {
        String::new()
    };
    open_node_block(node, eol);
    fmt_mut(node, eol).before_terminator.push_str(&closing);
    semicolon_after_block(block, index);
}

/// Gives `node` an empty children block, `{}`.
fn open_node_block(node: &mut KdlNode, eol: &str) {
    let f = fmt_mut(node, eol);
    // Without a block, trailing slashdashed entries sit in before_terminator, which prints after the block. They have
    // to stay in front of it. So does any other whitespace there: right after `}`, kdl-rs drops it when it reads the
    // text back.
    if !f.before_terminator.is_empty() {
        let mut moved = std::mem::take(&mut f.before_terminator);
        // Autoformat ends before_terminator with a line ending. That goes after the block, not before `{`, where it
        // would end the node early.
        if ends_line(&moved) {
            moved.truncate(moved.trim_end().len());
            if f.terminator.is_empty() {
                f.terminator = eol.into();
            }
        }
        if continuation(&moved) == Some(false) {
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
        let style = Style::of(&doc, parent, line_ending(&doc));
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
        let style = Style::of(&doc, parent, line_ending(&doc));
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
        assert_eq!(line_rest(" /* a */ // b\n c"), Some(" /* a */ // b\n"));
        assert_eq!(line_rest("\r\n"), Some("\r\n"));
        assert_eq!(line_rest(" /* a\n */\n"), None);
        assert_eq!(line_rest(" b\n"), None);
        assert_eq!(line_rest(" // b"), Some(" // b"));
        assert_eq!(line_rest(" /* b */"), None);
        assert_eq!(continuation(" \\"), Some(false));
        assert_eq!(continuation(" \\ /* a */ // b"), Some(false));
        assert_eq!(continuation(" \\\r\n  "), Some(true));
        assert_eq!(continuation(" \\\n\n"), None);
        assert_eq!(continuation(" /-\"\\\\\" \\"), Some(false));
        assert_eq!(continuation(" /-\"\\\""), None);
        assert_eq!(continuation(" /* \\ */"), None);
        assert_eq!(string_ranges("/-#\"a\"# /* \" */ \"\\\"\""), vec![2..7, 16..20]);
        assert!(ends_line("a\r\n  "));
        assert!(!ends_line("a;"));
        assert_eq!(indent_of("\r\n    // c\r\n  "), "  ");
        assert_eq!(restyle("a\nb\r\nc", "\r\n"), "a\r\nb\r\nc");
        assert_eq!(restyle("a\r\nb", "\n"), "a\nb");
        let shift = Shift {
            from: "  ".into(),
            to: "    ".into(),
            from_unit: String::new(),
            to_unit: String::new(),
            eol: "\r\n",
        };
        assert_eq!(shift.apply("// c\n  x", false), "// c\r\n    x");
        // Lines in strings stay as they are, line endings included.
        assert_eq!(
            shift.apply("\n  /-\"\"\"\n  a\n  \"\"\"\n  ", false),
            "\r\n    /-\"\"\"\n  a\n  \"\"\"\r\n    "
        );
    }

    #[test]
    fn line_ending_comes_from_the_block_then_the_document() {
        assert_eq!(line_ending(&parse("a\r\nb\r\n")), "\r\n");
        assert_eq!(line_ending(&parse("a {\n}\r\n")), "\n");
        assert_eq!(line_ending(&parse("a")), "\n");
        let doc = parse("a { b }\r\n");
        assert_eq!(Style::of(&doc, Some(&[0]), line_ending(&doc)).eol, "\r\n");
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
        assert_eq!(placed("a; b\n", None, 1, "x"), "a;\nx\nb\n");
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
        let style = Style::of(&doc, None, line_ending(&doc));
        let continued = parse("n 1 \\").nodes()[0].clone();
        place(&mut doc, 0, continued, &style);
        assert_eq!(doc.to_string(), "n 1 \\\n;\na\n");
    }

    fn opened(text: &str, path: &[usize]) -> String {
        let mut doc = parse(text);
        open_block(&mut doc, path, "\n");
        let out = doc.to_string();
        assert_eq!(parse(&out).to_string(), out, "{out:?} doesn't read back as itself");
        out
    }

    #[test]
    fn open_block_keeps_slashdashed_entries_in_front() {
        assert_eq!(opened("n /- k=v\r\n", &[0]), "n /- k=v {}\r\n");
    }

    #[test]
    fn no_whitespace_after_a_new_closing_brace() {
        assert_eq!(opened("c \nd\n", &[0]), "c {}\nd\n");
        assert_eq!(opened("c ", &[0]), "c {}");
        assert_eq!(opened("a { b }\n", &[0, 0]), "a { b {}; }\n");
        assert_eq!(opened("a { b; c }\n", &[0, 0]), "a { b {}; c }\n");
        assert_eq!(opened("a {b}\n", &[0, 0]), "a {b {}}\n");
        assert_eq!(
            placed("a { b; c }\n", Some(&[0]), 2, "x 1 { y }"),
            "a { b; c; x 1 { y }; }\n"
        );
    }

    fn replaced(text: &str, parent: Option<&[usize]>, index: usize, new: &str) -> (String, String) {
        let mut doc = parse(text);
        let style = Style::of(&doc, parent, line_ending(&doc));
        let block = match parent {
            None => &mut doc,
            Some(path) => crate::tree::node_at_mut(&mut doc, path)
                .children_mut()
                .as_mut()
                .unwrap(),
        };
        let old = replace(block, index, node(new), &style);
        (doc.to_string(), old.to_string())
    }

    #[test]
    fn a_replacement_takes_the_old_nodes_place() {
        assert_eq!(replaced("a { b }\n", Some(&[0]), 0, "c").0, "a { c }\n");
        assert_eq!(replaced("a { b; }\n", Some(&[0]), 0, "c").0, "a { c; }\n");
        assert_eq!(replaced("a { b; c }\n", Some(&[0]), 1, "x").0, "a { b; x }\n");
        assert_eq!(replaced("a { b; c }\n", Some(&[0]), 1, "x {}").0, "a { b; x {}; }\n");
        assert_eq!(replaced("a {\n    b\n}\n", Some(&[0]), 0, "c").0, "a {\n    c\n}\n");
        // The space before `}` stays with the block.
        assert_eq!(replaced("a { b }\n", Some(&[0]), 0, "c").1, " b");
    }

    #[test]
    fn the_last_node_of_a_single_line_block_leaves_the_space_before_brace() {
        let mut doc = parse("a { b; c }\n");
        let style = Style::of(&doc, Some(&[0]), line_ending(&doc));
        let block = crate::tree::node_at_mut(&mut doc, &[0])
            .children_mut()
            .as_mut()
            .unwrap();
        let taken = unplace(block, 1, &style);
        assert_eq!(doc.to_string(), "a { b; }\n");
        let style = Style::of(&doc, None, line_ending(&doc));
        place(&mut doc, 1, taken, &style);
        assert_eq!(doc.to_string(), "a { b; }\nc\n");
        // A copy still has the space, and loses it when placed.
        assert_eq!(placed("a\n", None, 1, "c "), "a\nc\n");
    }

    #[test]
    fn an_empty_block_from_the_top_level_lines_up() {
        assert_eq!(
            placed("a {\n    b\n}\n", Some(&[0]), 1, "q {\n}"),
            "a {\n    b\n    q {\n    }\n}\n"
        );
    }

    #[test]
    fn continuations_take_the_new_style() {
        assert_eq!(
            placed("a\r\n", None, 1, "n \\\n    1 \"x\\ny\""),
            "a\r\nn \\\r\n    1 \"x\\ny\"\r\n"
        );
        // A line ending inside a string is the string's.
        assert_eq!(
            placed("a\r\n", None, 1, "n \"\"\"\n  x\n  \"\"\""),
            "a\r\nn \"\"\"\n  x\n  \"\"\"\r\n"
        );
    }

    #[test]
    fn an_emptied_document_uses_the_style_it_was_given() {
        let mut doc = parse("a {\r\n    b\r\n}\r\n");
        let eol = line_ending(&doc);
        let style = Style::of(&doc, None, eol);
        let taken = unplace(&mut doc, 0, &style);
        assert_eq!(doc.to_string(), "");
        let style = Style::of(&doc, None, eol);
        place(&mut doc, 0, taken, &style);
        assert_eq!(doc.to_string(), "a {\r\n    b\r\n}\r\n");
        assert_eq!(unplaced("a\n", None, 0), "");
    }

    #[test]
    fn gaps() {
        assert_eq!(gap_len("// h\n\n// own\n", true), 6);
        assert_eq!(gap_len("// own\n  ", true), 0);
        // The rest of the `{` line, or of a line after a `;`, is never the node's own.
        assert_eq!(gap_len("\n    // own\n    ", false), 1);
        assert_eq!(gap_len(" // a's\n// own\n", false), 8);
        assert_eq!(gap_len("\n\n    ", false), 2);
        assert_eq!(gap_len("  \r\n\r\nx", true), 6);
        assert_eq!(without_closing_blanks("// h\n\n \n", true), "// h\n");
        assert_eq!(without_closing_blanks("\n\n", false), "\n");
        // Blank lines inside a comment or a slashdashed node don't count.
        assert_eq!(gap_len("/* a\n\n b */\n", true), 0);
        assert_eq!(gap_len("// h\n\n/* a /* b\n\n */ c\n\n */\n", true), 6);
        assert_eq!(gap_len("/-x {\n\n}\n/-\n\ny\n/-z \"\"\"\n\n  \"\"\"\n", true), 0);
        assert_eq!(gap_len("/-x #\"\"\"\n\n  }\n  \"\"\"#; /-y \"\\\"}\"\n\n", true), 33);
        assert_eq!(gap_len("/-x /-\n\n  1\n\n// own\n", true), 13);
        // A slashdashed node also ends at the `}` of the block it's last in, or at the end of the text, as a final
        // node in a block or a document does. A node's leading text never ends in one, but the scanner doesn't rely
        // on that.
        assert_eq!(multi_line_pieces("/-x 1 }"), vec![0..6]);
        assert_eq!(multi_line_pieces("/-x {\n\n}"), vec![0..8]);
    }

    const HEADED: &str = "// header\n\n// about a\na\n\nb\n";

    #[test]
    fn the_gap_stays_when_a_node_leaves() {
        assert_eq!(unplaced(HEADED, None, 0), "// header\n\nb\n");
        assert_eq!(unplaced(HEADED, None, 1), "// header\n\n// about a\na\n");
        assert_eq!(unplaced("// header\n\na\n", None, 0), "// header\n");
        assert_eq!(unplaced("a\n\n// note\n\nb\n", None, 1), "a\n\n// note\n");
        assert_eq!(unplaced("a\n\nb\n\n// end\n", None, 1), "a\n\n// end\n");
        assert_eq!(unplaced("// header\n\na; b\n", None, 0), "// header\n\nb\n");
        // Slashdashed nodes count as comments.
        assert_eq!(unplaced("/- x\n\na\nb\n", None, 0), "/- x\n\nb\n");
        assert_eq!(unplaced("/- x\na\nb\n", None, 0), "b\n");
        assert_eq!(
            unplaced("a {\n\n    b\n\n    c\n}\n", Some(&[0]), 0),
            "a {\n\n    c\n}\n"
        );
        assert_eq!(
            unplaced("a {\n    b\n\n    // c\n    c\n}\n", Some(&[0]), 1),
            "a {\n    b\n}\n"
        );
        assert_eq!(
            unplaced("a {\n    // x\n\n    b\n}\n", Some(&[0]), 0),
            "a {\n    // x\n}\n"
        );
        assert_eq!(unplaced("a {\n\n    b\n}\n", Some(&[0]), 0), "a {}\n");
        assert_eq!(
            unplaced("// h\r\n\r\n// a\r\na\r\n\r\nb\r\n", None, 0),
            "// h\r\n\r\nb\r\n"
        );
    }

    #[test]
    fn a_new_node_goes_below_the_gap() {
        assert_eq!(placed(HEADED, None, 0, "x"), "// header\n\nx\n// about a\na\n\nb\n");
        assert_eq!(placed(HEADED, None, 1, "x"), "// header\n\n// about a\na\n\nx\nb\n");
        assert_eq!(
            placed("// header\n\na\n", None, 0, "// x\nx"),
            "// header\n\n// x\nx\na\n"
        );
        assert_eq!(
            placed("a {\n\n    // c\n    b\n}\n", Some(&[0]), 0, "x"),
            "a {\n\n    x\n    // c\n    b\n}\n"
        );
        assert_eq!(placed("// h\r\n\r\na\r\n", None, 0, "x"), "// h\r\n\r\nx\r\na\r\n");
        // An emptied block keeps its gap at the top.
        let mut doc = parse("// header\n\na\n");
        let style = Style::of(&doc, None, "\n");
        unplace(&mut doc, 0, &style);
        place(&mut doc, 0, node("x"), &style);
        assert_eq!(doc.to_string(), "// header\nx\n");
    }

    #[test]
    fn a_replacement_goes_below_the_gap() {
        assert_eq!(
            replaced(HEADED, None, 0, "x"),
            ("// header\n\nx\n\nb\n".into(), "// about a\na\n".into())
        );
        // A node with an open `\` can't take the slot, and still goes below the gap.
        assert_eq!(replaced("// h\n\na\nb\n", None, 0, "x \\").0, "// h\n\nx \\\n;\nb\n");
    }

    #[test]
    fn reordering_keeps_gaps_by_position() {
        let mut doc = parse("// header\n\n// about a\na\n\nb\n");
        assert!(permute(&mut doc, &[0, 1], &[1, 0]));
        assert_eq!(doc.to_string(), "// header\n\nb\n\n// about a\na\n");
    }

    #[test]
    fn a_copy_leaves_the_gap() {
        let doc = parse(HEADED);
        let mut copy = doc.nodes()[0].clone();
        drop_gap(&doc, &[0], &mut copy);
        assert_eq!(leading(&copy), "// about a\n");
    }
    /// `x 1` with a children block `{ y }` that has no format record, which kdl-rs prints with a line ending after
    /// `{`. rkdl never builds one: parsed blocks and `open_node_block`'s have a record.
    fn with_bare_block() -> KdlNode {
        let mut x = node("x 1");
        let mut block = KdlDocument::new();
        block.nodes_mut().push(node("y"));
        x.set_children(block);
        x
    }

    /// The names of a document's nodes and their children's, in order.
    fn names(doc: &KdlDocument) -> Vec<String> {
        let mut out = Vec::new();
        let mut pending: Vec<&KdlNode> = doc.nodes().iter().rev().collect();
        while let Some(n) = pending.pop() {
            out.push(n.name().value().to_owned());
            pending.extend(n.children().into_iter().flat_map(|b| b.nodes().iter().rev()));
        }
        out
    }

    #[test]
    fn a_block_without_a_format_record_is_not_one_line() {
        // kdl-rs starts a line after its `{`, so it doesn't fit in a one-line block, and a block holding it isn't one.
        assert!(with_bare_block().to_string().contains('\n'));
        assert!(!fits_on_a_line(&with_bare_block()));
        let mut doc = parse("a { b }\n");
        let block = node_at_mut(&mut doc, &[0]).children_mut().as_mut().unwrap();
        block.nodes_mut().push(with_bare_block());
        assert!(!is_single_line(block));
    }

    /// A strict xfail for a cargo test: `out` should be `expected`, and isn't yet. Once a fix makes it so, this
    /// fails, and the call becomes an `assert_eq!`.
    fn known_bug(out: &str, expected: &str, task: &str) {
        assert_ne!(out, expected, "{task} fixed this: make it an assert_eq!");
    }

    #[test]
    fn a_node_with_a_block_without_a_format_record_moves() {
        // Placed in a nested block, and in a one-line one (which spreads). It reads back with every node where it
        // went. The module doc also has its children take the new indent and its `}` line up with it, which a block
        // without a record doesn't get yet: today y stays at column 0 and `}` follows it on its line.
        for text in ["a {\n    b\n}\n", "a { b }\n"] {
            let mut x = with_bare_block();
            x.format_mut().unwrap().before_children = " ".into();
            let mut doc = parse(text);
            let style = Style::of(&doc, Some(&[0]), "\n");
            let block = node_at_mut(&mut doc, &[0]).children_mut().as_mut().unwrap();
            place(block, 1, x, &style);
            assert!(!is_single_line(block));
            let out = doc.to_string();
            assert_eq!(names(&parse(&out)), ["a", "b", "x", "y"], "{out:?}");
            known_bug(
                &out,
                "a {\n    b\n    x 1 {\n        y\n    }\n}\n",
                "task 71 (the node-format rewrite)",
            );
        }
    }

    #[test]
    fn emptying_a_block_without_a_format_record() {
        let mut doc = parse("p\n");
        let mut bare = KdlDocument::new();
        bare.nodes_mut().push(node("y"));
        doc.nodes_mut()[0].set_children(bare);
        doc.nodes_mut()[0].format_mut().unwrap().before_children = " ".into();
        let style = Style::of(&doc, Some(&[0]), "\n");
        let block = node_at_mut(&mut doc, &[0]).children_mut().as_mut().unwrap();
        assert_eq!(unplace(block, 0, &style).name().value(), "y");
        assert_eq!(block.nodes().len(), 0);
        assert_eq!(names(&parse(&doc.to_string())), ["p"]);
        // `unplace`'s doc: an emptied children block prints as `{}`. Without a record, kdl-rs prints `{\n}` today.
        known_bug(&doc.to_string(), "p {}\n", "task 71 (`block_fmt_mut` in `unplace`)");
    }

    #[test]
    fn spreading_keeps_a_comment_before_the_first_node() {
        // kdl-rs keeps text before a one-line block's first node in that node's leading, so the block's own leading
        // is blank there. If it isn't, spreading doesn't drop what's in it.
        let mut doc = parse("a { b }\n");
        let style = Style::of(&doc, Some(&[0]), "\n");
        let block = node_at_mut(&mut doc, &[0]).children_mut().as_mut().unwrap();
        block.format_mut().unwrap().leading = " /* c */".into();
        assert_eq!(doc.to_string(), "a { /* c */ b }\n");
        let block = node_at_mut(&mut doc, &[0]).children_mut().as_mut().unwrap();
        place(block, 1, node("x {\n    y\n}"), &style);
        assert_eq!(doc.to_string(), "a { /* c */\n    b\n    x {\n        y\n    }\n}\n");
    }

    #[test]
    fn a_closing_brace_on_its_own_line_stays_there() {
        // The blank line above p's `}` is p's block's trailing text. `}` is on a line of its own already.
        let text = "q {\n    p {\n        a\n\n    }\n}\n";
        let mut doc = parse(text);
        let style = Style::of(&doc, Some(&[0, 0]), "\n");
        let block = node_at_mut(&mut doc, &[0, 0]).children_mut().as_mut().unwrap();
        close_on_own_line(block, &style);
        assert_eq!(doc.to_string(), text);
    }

    #[test]
    fn placing_a_node_keeps_a_comment_after_it() {
        // Only a `;` node carries comments in its trailing text when rkdl moves it, and `place` puts those after the
        // `;`. Any other node's are kept where they are, and the node's line ends after them.
        let mut x = node("x");
        let f = x.format_mut().unwrap();
        f.terminator = String::new();
        f.trailing = " /* c */".into();
        let mut doc = parse("a\n");
        let style = Style::of(&doc, None, "\n");
        place(&mut doc, 1, x, &style);
        assert_eq!(doc.to_string(), "a\nx /* c */\n");
    }
}
