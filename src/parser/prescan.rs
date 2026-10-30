//! A pass over the text before kdl-rs sees it, refusing input that would make kdl-rs's parser run out of stack.
//!
//! kdl-rs's parser recurses once per block level, once per piece of a block comment (every `*`, every `/`, every
//! stretch of other text), and, after some errors, once per character it skips. The parse runs on a thread with a big
//! stack (see `parser.rs`), and the limits here keep it far inside that.
//!
//! To count block levels the scan has to know which braces are structure, so it reads strings, raw strings, comments
//! and line continuations the way kdl-rs does. That only holds for valid input: kdl-rs's error recovery can read half
//! of a bad string as structure, or skip characters one recursive call at a time. So the scan also checks the rules
//! whose breakage would put its reading and kdl-rs's out of step, and from the first break on it counts every
//! character as possible recursion, against a fixed allowance. Most invalid documents end within the allowance and
//! get kdl-rs's own error report; longer ones get the scan's report of the first break.
//!
//! Costs are in steps of about 4 KB of stack, measured on a debug build (a release build uses a fifth of that): a
//! block level takes 32 KB, a block comment piece 3.4 KB, a nested `/*` 8.9 KB, and a skipped character up to 3.2 KB.
//! All three limits together come to about 140 MB of the parser thread's 256 MB.

/// Block levels, at 8 steps each.
const MAX_DEPTH: usize = 1000;
const LEVEL_STEPS: usize = 8;
/// Steps within one block comment, nested ones included: a piece is 1, a nested `/*` is 3.
const MAX_COMMENT_STEPS: usize = 10_000;
/// Steps from the first break to the end: a `{` is a level, anything else 2.
const MAX_UNCHECKED_STEPS: usize = 18_000;

/// Why the scan refused the text. Offsets are in bytes.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) offset: usize,
    pub(crate) len: usize,
    pub(crate) message: String,
    pub(crate) label: String,
    pub(crate) help: String,
}

/// A refusal, or none. Tests also look at where the scan stopped trusting the text, and the most steps kdl-rs could
/// need, to compare with real stack use.
#[derive(Debug)]
pub(crate) struct Scan {
    pub(crate) refusal: Option<Refusal>,
    #[cfg_attr(not(test), expect(dead_code, reason = "only tests read it"))]
    pub(crate) broke_at: Option<usize>,
    #[cfg_attr(not(test), expect(dead_code, reason = "only tests read it"))]
    pub(crate) peak_steps: usize,
}

pub(crate) fn scan(text: &str) -> Scan {
    let mut lexer = Lexer::new(text);
    let (refusal, broke_at) = match lexer.run() {
        Ok(()) => (None, None),
        Err(Stop::Refuse(refusal)) => (Some(refusal), None),
        Err(Stop::Break { at, from, what }) => (lexer.unchecked(at, from, what), Some(at)),
    };
    Scan {
        refusal,
        broke_at,
        peak_steps: lexer.peak,
    }
}

/// KDL's whitespace that isn't a line ending (the spec's `unicode-space`).
pub(crate) const KDL_SPACES: &[char] = &[
    '\t', ' ', '\u{a0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
    '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{202f}', '\u{205f}', '\u{3000}',
];

/// KDL's line endings, apart from `\r\n`, which reads as one.
const NEWLINES: &[char] = &['\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}'];

fn is_space(c: char) -> bool {
    KDL_SPACES.contains(&c)
}

fn is_newline(c: char) -> bool {
    NEWLINES.contains(&c)
}

fn is_blank(c: char) -> bool {
    is_space(c) || is_newline(c)
}

/// kdl-rs's list, which leaves out U+007F though the spec bans it. A BOM is allowed only as the first character.
fn is_disallowed(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{e}'..='\u{1f}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

fn is_ident_char(c: char) -> bool {
    !(is_blank(c) || is_disallowed(c) || "\\/(){}[];\"#=".contains(c))
}

/// Whether a bare word is an identifier, which can be a property key, rather than a number or keyword.
fn is_identifier(word: &str) -> bool {
    let starts_digit = |s: &str| s.starts_with(|c: char| c.is_ascii_digit());
    let unsigned = word.strip_prefix(['+', '-']).unwrap_or(word);
    let undotted = unsigned.strip_prefix('.').unwrap_or(unsigned);
    !(word.is_empty()
        || word.starts_with('#')
        || starts_digit(unsigned)
        || starts_digit(undotted)
        || matches!(word, "true" | "false" | "null" | "inf" | "-inf" | "nan"))
}

enum Stop {
    Refuse(Refusal),
    /// The text can't be valid KDL `at` this point, so kdl-rs's reading may differ from the scan's `from` there on.
    /// Inside a string that's from the string's start: kdl-rs's error recovery can go back there.
    Break {
        at: usize,
        from: usize,
        what: &'static str,
    },
}

impl Stop {
    fn broken(at: usize, what: &'static str) -> Self {
        Stop::Break { at, from: at, what }
    }

    /// The same stop, for a break inside a string that starts at `start`.
    fn counting_from(self, start: usize) -> Self {
        match self {
            Stop::Break { at, from, what } => Stop::Break {
                at,
                from: from.min(start),
                what,
            },
            Stop::Refuse(refusal) => Stop::Refuse(refusal),
        }
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per thing the next token has to agree with"
)]
struct Lexer<'a> {
    text: &'a str,
    pos: usize,
    /// Whether each open block is slashdashed (`/-{`).
    blocks: Vec<bool>,
    peak: usize,
    in_type: bool,
    type_start: usize,
    type_named: bool,
    /// The next value is a node's name.
    node_start: bool,
    /// The last token, spaces aside, could be a property key.
    key: bool,
    /// The last token was `=`.
    after_eq: bool,
    /// A type annotation just closed.
    typed: bool,
    /// The last token was a value. The next character has to end it: a space, `=`, `)`, `{`, `}`, `;`, a line
    /// ending, a comment or a line continuation.
    after_value: bool,
    /// A block just closed. Its node is over, so a `;`, a line ending, `}` or a `/-` block comes next, or after a
    /// slashdashed block, the node's real one.
    after_block: bool,
    after_dashed_block: bool,
    /// The last token was `/-`.
    dashed: bool,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Lexer {
            text,
            pos: 0,
            blocks: Vec::new(),
            peak: 0,
            in_type: false,
            type_start: 0,
            type_named: false,
            node_start: true,
            key: false,
            after_eq: false,
            typed: false,
            after_value: false,
            after_block: false,
            after_dashed_block: false,
            dashed: false,
        }
    }

    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, s: &str) -> bool {
        let found = self.rest().starts_with(s);
        if found {
            self.pos += s.len();
        }
        found
    }

    /// One line ending, `\r\n` included.
    fn eat_newline(&mut self) -> bool {
        self.eat("\r\n") || (self.peek().is_some_and(is_newline) && self.bump().is_some())
    }

    fn broken<T>(&self, what: &'static str) -> Result<T, Stop> {
        Err(Stop::broken(self.pos, what))
    }

    fn note(&mut self, steps: usize) {
        self.peak = self.peak.max(steps);
    }

    fn run(&mut self) -> Result<(), Stop> {
        self.eat("\u{feff}");
        while let Some(c) = self.peek() {
            // kdl-rs can go back to the start of a type annotation that turns out bad, and read it as something else.
            self.step(c).map_err(|stop| {
                if self.in_type {
                    stop.counting_from(self.type_start)
                } else {
                    stop
                }
            })?;
        }
        Ok(())
    }

    /// Reads one piece of the text, starting with `c`.
    fn step(&mut self, c: char) -> Result<(), Stop> {
        if std::mem::take(&mut self.after_value) {
            let ends_value =
                is_blank(c) || "=){};\\".contains(c) || self.rest().starts_with("/*") || self.rest().starts_with("//");
            if !ends_value {
                return self.broken("Expected a space or a line break after this value");
            }
        }
        let line_ends = is_newline(c) || self.rest().starts_with("//");
        if self.in_type && line_ends {
            return self.broken("Line break inside a type annotation");
        }
        if self.typed && (line_ends || self.rest().starts_with("/-")) {
            return self.broken("Expected a value after the type annotation");
        }
        if is_space(c) {
            self.bump();
        } else if is_newline(c) {
            self.eat_newline();
            // `/-` reaches over line breaks to the next node or entry.
            if !self.dashed {
                self.end_node();
            }
        } else if self.eat("//") {
            // The line ending is left for the next round, where it ends the node.
            self.line_comment();
        } else if self.rest().starts_with("/*") {
            self.block_comment()?;
        } else if self.rest().starts_with("/-") {
            if self.dashed {
                return self.broken("Expected something for `/-` to comment out");
            }
            self.pos += 2;
            self.dashed = true;
            self.key = false;
        } else if c == '\\' {
            self.line_continuation()?;
        } else {
            self.token(c)?;
        }
        Ok(())
    }

    fn end_node(&mut self) {
        self.node_start = true;
        self.key = false;
        self.after_eq = false;
        self.typed = false;
        self.after_block = false;
        self.after_dashed_block = false;
    }

    fn open_block(&mut self, dashed: bool) -> Result<(), Stop> {
        self.bump();
        self.blocks.push(dashed);
        self.note(self.blocks.len() * LEVEL_STEPS);
        if self.blocks.len() > MAX_DEPTH {
            return Err(Stop::Refuse(Refusal {
                offset: self.pos - 1,
                len: 1,
                message: "Document nests too deeply".into(),
                label: format!("block {} levels deep", self.blocks.len()),
                help: format!(
                    "rkdl reads at most {MAX_DEPTH} levels of blocks: kdl-rs's parser recurses once per level and \
                     would run out of stack"
                ),
            }));
        }
        self.end_node();
        Ok(())
    }

    fn close_block(&mut self) -> Result<(), Stop> {
        let Some(dashed) = self.blocks.pop() else {
            return self.broken("Unexpected `}`: no block is open");
        };
        self.bump();
        self.end_node();
        self.node_start = false;
        self.after_block = true;
        self.after_dashed_block = dashed;
        Ok(())
    }

    fn token(&mut self, c: char) -> Result<(), Stop> {
        let key = std::mem::take(&mut self.key);
        let dashed = std::mem::take(&mut self.dashed);
        let after_dashed_block = std::mem::take(&mut self.after_dashed_block);
        if std::mem::take(&mut self.after_block)
            && !matches!(c, '}' | ';')
            && !(c == '{' && (dashed || after_dashed_block))
        {
            return self.broken("Expected `;` or a line break after this block");
        }
        if self.typed && "{}();=".contains(c) {
            return self.broken("Expected a value after the type annotation");
        }
        if dashed && "});=".contains(c) {
            return self.broken("Expected something for `/-` to comment out");
        }
        match c {
            '{' | '}' | ';' | '=' if self.in_type => self.broken("Expected `)` to close the type annotation"),
            '{' => self.open_block(dashed),
            '}' => self.close_block(),
            ';' => {
                self.bump();
                self.end_node();
                Ok(())
            }
            '(' => {
                if self.in_type {
                    return self.broken("Unexpected `(` inside a type annotation");
                }
                self.type_start = self.pos;
                self.bump();
                self.in_type = true;
                self.type_named = false;
                Ok(())
            }
            ')' => {
                if !self.in_type {
                    return self.broken("Unexpected `)`: no type annotation is open");
                }
                if !self.type_named {
                    return self.broken("Expected a type name");
                }
                self.bump();
                self.in_type = false;
                self.typed = true;
                Ok(())
            }
            '=' => {
                if !key {
                    return self.broken("Unexpected `=`: it goes between a property's key and its value");
                }
                self.bump();
                self.after_eq = true;
                Ok(())
            }
            '"' => {
                let start = self.pos;
                self.value_start(true)?;
                self.bump();
                let read = if self.eat("\"\"") {
                    self.multi_line_string()
                } else {
                    self.string()
                };
                read.map_err(|stop| stop.counting_from(start))
            }
            '#' if self.rest().starts_with("##") || self.rest().starts_with("#\"") => {
                let start = self.pos;
                self.value_start(true)?;
                self.raw_string().map_err(|stop| stop.counting_from(start))
            }
            c if is_ident_char(c) || c == '#' => {
                let rest = self.rest();
                let tail = rest.get(c.len_utf8()..).unwrap_or_default();
                let len = rest.len() - tail.trim_start_matches(is_ident_char).len();
                self.value_start(is_identifier(rest.get(..len).unwrap_or_default()))?;
                self.pos += len;
                Ok(())
            }
            _ => self.broken("Unexpected character"),
        }
    }

    /// Bookkeeping for a name, key or value. `keyable`: the token can be a property key.
    fn value_start(&mut self, keyable: bool) -> Result<(), Stop> {
        self.after_value = true;
        if self.in_type {
            if self.type_named {
                return self.broken("Expected `)`: a type annotation holds one name");
            }
            self.type_named = true;
            return Ok(());
        }
        let name = std::mem::take(&mut self.node_start);
        let after_eq = std::mem::take(&mut self.after_eq);
        let typed = std::mem::take(&mut self.typed);
        self.key = keyable && !name && !after_eq && !typed;
        Ok(())
    }

    fn line_comment(&mut self) {
        while self.peek().is_some_and(|c| !is_newline(c)) {
            self.bump();
        }
    }

    /// `/* ... */`, nested ones included. kdl-rs recurses once per piece and doesn't unwind until the outermost
    /// comment closes, so every piece counts.
    fn block_comment(&mut self) -> Result<(), Stop> {
        let start = self.pos;
        let base = self.blocks.len() * LEVEL_STEPS;
        let mut steps = 0;
        let mut open = 0usize;
        while !self.rest().is_empty() {
            if open > 0 && self.eat("*/") {
                open -= 1;
                if open == 0 {
                    return Ok(());
                }
                steps += 1;
            } else if self.eat("/*") {
                open += 1;
                steps += 3;
            } else if self.eat("*") || self.eat("/") {
                steps += 1;
            } else {
                while self.peek().is_some_and(|c| c != '*' && c != '/') {
                    self.bump();
                }
                steps += 1;
            }
            self.note(base + steps);
            if steps > MAX_COMMENT_STEPS {
                return Err(Stop::Refuse(Refusal {
                    offset: start,
                    len: 2,
                    message: "Block comment too long".into(),
                    label: "this comment".into(),
                    help: format!(
                        "rkdl reads block comments of up to {MAX_COMMENT_STEPS} pieces (each `*`, each `/`, and the \
                         text between them): kdl-rs's parser recurses once per piece and would run out of stack. \
                         Line comments (`//`) have no limit."
                    ),
                }));
            }
        }
        // kdl-rs reads an unclosed comment again as something else.
        Err(Stop::broken(start, "Unclosed block comment"))
    }

    /// `\`, then spaces and block comments, then a line ending, a line comment or the end of the text.
    fn line_continuation(&mut self) -> Result<(), Stop> {
        self.bump();
        loop {
            match self.peek() {
                Some(c) if is_space(c) => {
                    self.bump();
                }
                Some('/') if self.rest().starts_with("/*") => self.block_comment()?,
                _ => break,
            }
        }
        if self.eat("//") {
            self.line_comment();
            self.eat_newline();
        } else if !self.eat_newline() && self.peek().is_some() {
            return self.broken("Expected a line break after `\\`");
        }
        Ok(())
    }

    /// The rest of a `\` escape in a quoted string, the `\` already read.
    fn escape(&mut self) -> Result<(), Stop> {
        let at = self.pos.saturating_sub(1);
        let invalid = Err(Stop::broken(at, "Invalid escape in a string"));
        match self.bump() {
            Some('\\' | '"' | 'b' | 'f' | 'n' | 'r' | 't' | 's') => Ok(()),
            Some('u') => {
                if !self.eat("{") {
                    return invalid;
                }
                let digits_at = self.pos;
                while self.pos - digits_at < 6 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                    self.bump();
                }
                let digits = self.text.get(digits_at..self.pos).unwrap_or_default();
                let valid = u32::from_str_radix(digits, 16).ok().and_then(char::from_u32).is_some();
                if valid && self.eat("}") { Ok(()) } else { invalid }
            }
            Some(c) if is_blank(c) => {
                while self.peek().is_some_and(is_blank) {
                    self.bump();
                }
                Ok(())
            }
            _ => invalid,
        }
    }

    /// A `"` string, the opening quote already read.
    fn string(&mut self) -> Result<(), Stop> {
        let start = self.pos - 1;
        loop {
            let at = self.pos;
            match self.bump() {
                // kdl-rs reads an unclosed string again as something else.
                None => return Err(Stop::broken(start, "Unclosed string")),
                Some('"') => return Ok(()),
                Some('\\') => self.escape()?,
                Some(c) if is_newline(c) => return Err(Stop::broken(at, "Line break in a single-line string")),
                Some(c) if is_disallowed(c) => return Err(Stop::broken(at, "Character not allowed in KDL")),
                Some(_) => {}
            }
        }
    }

    /// A `"""` string, the opening quotes already read.
    fn multi_line_string(&mut self) -> Result<(), Stop> {
        let start = self.pos - 3;
        if !self.eat_newline() {
            return self.broken("Expected a line break after `\"\"\"`");
        }
        self.multi_line_body(start, "\"\"\"", true)
    }

    /// A `#"` raw string, at its first `#`.
    fn raw_string(&mut self) -> Result<(), Stop> {
        let start = self.pos;
        let hashes = self.rest().len() - self.rest().trim_start_matches('#').len();
        self.pos += hashes;
        let close = format!("\"{}", "#".repeat(hashes));
        if !self.eat("\"") {
            return self.broken("Expected `\"` after the `#`s of a raw string");
        }
        // `#"""` without a line break after it starts a single-line string whose text starts with `""`.
        if self.rest().starts_with("\"\"") {
            let quotes = self.pos;
            self.pos += 2;
            if self.eat_newline() {
                return self.multi_line_body(start, &format!("\"\"{close}"), false);
            }
            self.pos = quotes;
        }
        let body = self.pos;
        loop {
            if self.rest().starts_with(&close) {
                if self.text.get(body..self.pos) == Some("\"") {
                    return Err(Stop::broken(start, "A single-line raw string can't be just `\"`"));
                }
                self.pos += close.len();
                return Ok(());
            }
            match self.peek() {
                None => return Err(Stop::broken(start, "Unclosed string")),
                Some(c) if is_newline(c) => return self.broken("Line break in a single-line raw string"),
                Some(c) if is_disallowed(c) => return self.broken("Character not allowed in KDL"),
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    /// Where an escaped run of whitespace (`\` and then spaces and line breaks) starting at `at` ends, if one does.
    fn escaped_whitespace(&self, at: usize) -> Option<usize> {
        let rest = self.text.get(at..)?.strip_prefix('\\')?;
        let run = rest.len() - rest.trim_start_matches(is_blank).len();
        (run > 0).then_some(at + 1 + run)
    }

    /// The spaces at the start of the line at `at`, and where the line's leading whitespace ends. In a quoted string
    /// that whitespace can include escaped runs, which reach over line breaks; the spaces stop at the first one.
    fn leading_whitespace(&self, at: usize, quoted: bool) -> (&'a str, usize) {
        let rest = self.text.get(at..).unwrap_or_default();
        let spaces = rest.len() - rest.trim_start_matches(is_space).len();
        let mut end = at + spaces;
        while let Some(next) = self.escaped_whitespace(end).filter(|_| quoted) {
            end = next;
        }
        (rest.get(..spaces).unwrap_or_default(), end)
    }

    /// A multi-line string's lines, from just after the opening line break through the closing quotes, the way kdl-rs
    /// reads them, `start` being where the string starts. The closing quotes are the first that start a line, after its leading whitespace. Their line's
    /// spaces are the prefix every other line starts with, unless it's blank. `quoted` strings have escapes, and an
    /// escaped run of whitespace joins lines; raw strings have neither.
    fn multi_line_body(&mut self, start: usize, close: &str, quoted: bool) -> Result<(), Stop> {
        let mut lines = Vec::new();
        let prefix = loop {
            let line = self.pos;
            let (spaces, end) = self.leading_whitespace(line, quoted);
            if self.text.get(end..).is_some_and(|r| r.starts_with(close)) {
                self.pos = end + close.len();
                break spaces;
            }
            lines.push(line);
            loop {
                let at = self.pos;
                match self.peek() {
                    None => return Err(Stop::broken(start, "Unclosed string")),
                    Some(c) if is_newline(c) => break,
                    Some(c) if is_disallowed(c) => return Err(Stop::broken(at, "Character not allowed in KDL")),
                    Some(_) if self.rest().starts_with(close) => {
                        return Err(Stop::broken(at, "Closing quotes inside a multi-line string"));
                    }
                    Some('\\') if quoted => {
                        self.bump();
                        self.escape()?;
                    }
                    Some(_) => {
                        self.bump();
                    }
                }
            }
            self.eat_newline();
        };
        let end = self.pos;
        for line in lines {
            let (spaces, after) = self.leading_whitespace(line, true);
            if after > line + spaces.len() {
                // kdl-rs's handling of a body line that starts with escaped whitespace is subtle enough that the scan
                // doesn't follow it.
                return Err(Stop::broken(
                    line,
                    "Escaped whitespace starting a multi-line string line",
                ));
            }
            let text = self.text.get(line..).unwrap_or_default();
            let blank = text.get(spaces.len()..).is_some_and(|r| r.starts_with(is_newline));
            if !text.starts_with(prefix) && !blank {
                return Err(Stop::broken(
                    line,
                    "Line doesn't start with the indentation of the closing quotes",
                ));
            }
        }
        self.pos = end;
        Ok(())
    }

    /// From a break on, every character counts. None if the rest of the text fits the allowance.
    fn unchecked(&mut self, at: usize, from: usize, what: &'static str) -> Option<Refusal> {
        let rest = self.text.get(from..).unwrap_or_default();
        let steps: usize = rest.chars().map(|c| if c == '{' { LEVEL_STEPS } else { 2 }).sum();
        self.note(self.blocks.len() * LEVEL_STEPS + steps);
        (steps > MAX_UNCHECKED_STEPS).then(|| Refusal {
            offset: at,
            len: self
                .text
                .get(at..)
                .and_then(|r| r.chars().next())
                .map_or(0, char::len_utf8),
            message: what.into(),
            label: "here".into(),
            help: "rkdl stopped at the first error: after an error, kdl-rs's parser can't be trusted not to run out \
                   of stack on a document this long"
                .into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn kdl_rs() -> PathBuf {
        std::env::var_os("RKDL_KDL_RS").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("refs/kdl-rs"),
            PathBuf::from,
        )
    }

    /// Every file kdl-rs parses must scan without a break, or the scan would treat valid documents as suspect.
    #[test]
    fn documents_kdl_rs_parses_never_break() {
        let root = kdl_rs();
        let dirs = [root.join("tests/test_cases/input"), root.join("examples")];
        let mut checked = 0;
        for dir in dirs.iter().filter_map(|d| std::fs::read_dir(d).ok()) {
            for path in dir.flatten().map(|e| e.path()) {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if path.extension().is_some_and(|e| e == "kdl") && text.parse::<kdl::KdlDocument>().is_ok() {
                    let scan = scan(&text);
                    assert_eq!(scan.broke_at, None, "{}", path.display());
                    assert!(scan.refusal.is_none(), "{}", path.display());
                    checked += 1;
                }
            }
        }
        // Without the kdl-rs checkout there's nothing to check; the Python tests say so loudly.
        if root.is_dir() {
            assert!(checked > 100);
        }
    }

    fn short(text: &str) -> String {
        text.chars().take(40).collect()
    }

    fn nested(n: usize) -> String {
        "a{".repeat(n) + &"}".repeat(n)
    }

    #[test]
    fn depth_limit() {
        let at_limit = scan(&nested(MAX_DEPTH));
        assert!(at_limit.refusal.is_none());
        assert_eq!(at_limit.peak_steps, MAX_DEPTH * LEVEL_STEPS);
        let refusal = scan(&nested(MAX_DEPTH + 1)).refusal.unwrap();
        assert_eq!(refusal.offset, 2 * MAX_DEPTH + 1);
        assert_eq!(refusal.message, "Document nests too deeply");
        // Unclosed blocks count the same.
        assert!(scan(&"a{".repeat(MAX_DEPTH + 1)).refusal.is_some());
    }

    #[test]
    fn braces_in_strings_and_comments_dont_count() {
        let many = "{".repeat(MAX_DEPTH + 1);
        for text in [
            format!("a \"{many}\""),
            format!("a #\"{many}\"#"),
            format!("a ##\"\"#{many}\"##"),
            format!("a \"\"\"\n  {many}\n  \"\"\""),
            format!("a #\"\"\"\n  {many}\n  \"\"\"#"),
            format!("a // {many}"),
            format!("a /* {many} */"),
            format!("a \"\\u{{10ffff}}\"{}", "; b {}".repeat(MAX_DEPTH + 1)),
        ] {
            let scan = scan(&text);
            assert!(scan.refusal.is_none() && scan.broke_at.is_none(), "{:?}", short(&text));
        }
    }

    #[test]
    fn block_comment_pieces_count() {
        let stars = |n: usize| scan(&format!("/*{}*/", "*".repeat(n))).refusal;
        assert!(stars(MAX_COMMENT_STEPS - 10).is_none());
        assert_eq!(stars(MAX_COMMENT_STEPS + 1).unwrap().message, "Block comment too long");
        assert!(scan(&"/*".repeat(MAX_COMMENT_STEPS / 3 + 1)).refusal.is_some());
        // Text between the stars is one piece per stretch.
        assert!(scan(&format!("/*{}*/", "word ".repeat(100_000))).refusal.is_none());
    }

    /// Text kdl-rs's error recovery may read differently from the scan, each followed by enough to go past the
    /// allowance.
    #[test]
    fn breaks() {
        let tail = "a{".repeat(MAX_UNCHECKED_STEPS / LEVEL_STEPS + 1);
        for (text, at) in [
            ("a \"\\q \"", 3),
            ("a \"x\ny\"", 4),
            ("a \"\"\"x\n\"\"\"", 5),
            ("a \"\"\"\n x\n  \"\"\"", 6),
            ("}", 0),
            ("a )", 2),
            ("a =1", 2),
            ("a 1=2", 3),
            ("a (t)k=2", 6),
            ("a k=v=w", 5),
            ("a {} b", 5),
            ("a \"x\"y", 5),
            ("a 1\"x\"", 3),
            ("a (t u)", 5),
            ("a \\ b", 4),
            ("a #\"\"\"#", 2),
            ("a \u{1}", 2),
            ("a (t)(t)b", 5),
            ("a (t)\n1", 5),
            ("a /-}", 4),
            ("a /-/-1", 4),
        ] {
            let text = format!("{text}\n{tail}");
            let scan = scan(&text);
            assert_eq!(scan.broke_at, Some(at), "{:?}", short(&text));
            assert!(scan.refusal.is_some(), "{:?}", short(&text));
        }
    }

    /// kdl-rs reads what follows an unclosed string or comment again, as something else.
    #[test]
    fn unclosed_strings_and_comments_break() {
        let tail = "a{".repeat(MAX_UNCHECKED_STEPS / LEVEL_STEPS + 1);
        for open in ["\"", "#\"", "##\"\"\"\n", "\"\"\"\n", "/*"] {
            let text = format!("a {open}{tail}");
            let scan = scan(&text);
            assert_eq!(scan.broke_at, Some(2), "{:?}", short(&text));
            assert!(scan.refusal.is_some(), "{:?}", short(&text));
        }
    }

    /// kdl-rs's recovery from an error in a string or type annotation can go back to its start, so the braces before
    /// the error count too.
    #[test]
    fn breaks_in_strings_and_types_count_from_their_start() {
        let braces = "{".repeat(MAX_UNCHECKED_STEPS / LEVEL_STEPS + 1);
        for (text, at) in [
            (format!("a \"{braces}\\q\""), 3 + braces.len()),
            (format!("a \"\"\"\n{braces} \"\"\"\n\"\"\""), 7 + braces.len()),
            (format!("a #\"\"\"\n {braces}\nb\n \"\"\"#"), 9 + braces.len()),
            (format!("a (\"{braces}\" b)"), 6 + braces.len()),
        ] {
            let scan = scan(&text);
            assert_eq!(scan.broke_at, Some(at), "{:?}", short(&text));
            assert!(scan.refusal.is_some(), "{:?}", short(&text));
        }
    }

    #[test]
    fn a_short_rest_after_a_break_is_left_to_kdl_rs() {
        let scan = scan("a \"\\q\" {}\nb");
        assert_eq!(scan.broke_at, Some(3));
        assert!(scan.refusal.is_none());
    }

    #[test]
    fn valid_text_the_scan_must_follow() {
        for text in [
            "a k=v k2 = \"v\" (t)1 k3=(t)#true",
            "(t)a; b {\n c\n} /-{ d }; e",
            "a \\\n  b \\ // comment\n  c",
            "a /*\n multi /* nested */ */ b",
            "a /-\n 1 k=2",
            "a \"\"\"\n    x \\\"\"\" y\n\n    \"\"\"",
            "a #\"\"\"x\"\"\"#",
            "a \"\\u{41} \\s\"",
            "\u{feff}a",
            "a {\n}\n;b",
            "a {}; b {} /- {}",
        ] {
            let scan = scan(text);
            assert_eq!(scan.broke_at, None, "{:?}", short(text));
        }
    }
}
