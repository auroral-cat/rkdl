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
//!
//! kdl-rs also reads what's inside a slashdashed block (`/-{`, or the block of a `/-` node) twice: once trying it as
//! one thing, again after backtracking. Nested ones double again per level, so the time grows as the text inside
//! times 2 to the number of slashdashed levels around it. The scan adds that up and refuses text that would take
//! kdl-rs more than twice as long as reading it once, plus a fixed allowance.
//!
//! The scan also records where each block closes, which `blocks.rs` uses to find the text kdl-rs drops after a `}`.

/// Block levels, at 8 steps each.
pub(crate) const MAX_DEPTH: usize = 1000;
const LEVEL_STEPS: usize = 8;
/// Steps within one block comment, nested ones included: a piece is 1, a nested `/*` is 3.
const MAX_COMMENT_STEPS: usize = 10_000;
/// Steps from the first break to the end: a `{` is a level, anything else 2.
const MAX_UNCHECKED_STEPS: usize = 18_000;
/// Extra bytes kdl-rs may read because of nested slashdashed blocks, beyond the length of the document: a byte inside
/// `k` of them costs `2^k - 1`. Empty blocks reach it at 13 levels; the slowest text at the limit takes about 0.2 s on
/// a release build.
const MAX_DASHED_WORK: u64 = 64 << 10;

/// Why the scan refused the text. Offsets are in bytes.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) offset: usize,
    pub(crate) len: usize,
    pub(crate) message: String,
    pub(crate) label: String,
    pub(crate) help: String,
}

/// A refusal, or none, and where the scan stopped trusting the text. Tests also look at the most steps kdl-rs could
/// need, to compare with real stack use.
#[derive(Debug)]
pub(crate) struct Scan {
    pub(crate) refusal: Option<Refusal>,
    /// Where each block's `}` is, by depth: `closes[0]` for the blocks of top-level nodes, in text order. Complete only
    /// if the scan didn't break.
    pub(crate) closes: Vec<Vec<usize>>,
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
        closes: lexer.closes,
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
pub(crate) const NEWLINES: &[char] = &['\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}'];

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
    blocks: Vec<Block>,
    closes: Vec<Vec<usize>>,
    /// Where the `/-` of each open slashdashed block is, outermost first.
    dashed_blocks: Vec<usize>,
    /// The extra bytes kdl-rs reads because of nested slashdashed blocks so far.
    dashed_work: u64,
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
    /// Where the last `/-` was.
    dash_at: usize,
    /// Where the `/-` of the current node is, if it's slashdashed.
    node_dashed: Option<usize>,
}

struct Block {
    /// `/-{`
    dashed: bool,
    /// The block is slashdashed, by `/-{` or as the block of a `/-` node.
    commented: bool,
    /// `node_dashed` of the node the block belongs to, for after the block.
    node_dashed: Option<usize>,
}

/// What kdl-rs reads beyond one pass for a byte inside `levels` slashdashed blocks.
fn dashed_cost(levels: usize) -> u64 {
    u32::try_from(levels)
        .ok()
        .and_then(|k| 1u64.checked_shl(k))
        .filter(|&n| n != 1 << 63)
        .map_or(u64::MAX, |n| n - 1)
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Lexer {
            text,
            pos: 0,
            blocks: Vec::new(),
            closes: Vec::new(),
            dashed_blocks: Vec::new(),
            dashed_work: 0,
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
            dash_at: 0,
            node_dashed: None,
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
            let start = self.pos;
            // kdl-rs can go back to the start of a type annotation that turns out bad, and read it as something else.
            self.step(c).map_err(|stop| {
                if self.in_type {
                    stop.counting_from(self.type_start)
                } else {
                    stop
                }
            })?;
            let read = (self.pos - start) as u64;
            let cost = read.saturating_mul(dashed_cost(self.dashed_blocks.len()));
            self.dashed_work = self.dashed_work.saturating_add(cost);
            if self.dashed_work > self.dashed_allowance() {
                let at = self.dashed_blocks.last().copied().unwrap_or(start);
                return Err(Stop::Refuse(Refusal {
                    offset: at,
                    len: 2,
                    message: "Slashdashed blocks nest too deeply".into(),
                    label: format!("{} slashdashed blocks deep", self.dashed_blocks.len()),
                    help: DASHED_HELP.into(),
                }));
            }
        }
        Ok(())
    }

    fn dashed_allowance(&self) -> u64 {
        (self.text.len() as u64).saturating_add(MAX_DASHED_WORK)
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
            self.dash_at = self.pos;
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
        self.node_dashed = None;
    }

    fn open_block(&mut self, dashed: bool) -> Result<(), Stop> {
        self.bump();
        let commented = if dashed { Some(self.dash_at) } else { self.node_dashed };
        if let Some(at) = commented {
            self.dashed_blocks.push(at);
        }
        self.blocks.push(Block {
            dashed,
            commented: commented.is_some(),
            node_dashed: self.node_dashed,
        });
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
        let Some(block) = self.blocks.pop() else {
            return self.broken("Unexpected `}`: no block is open");
        };
        let depth = self.blocks.len();
        if self.closes.len() <= depth {
            self.closes.resize_with(depth + 1, Vec::new);
        }
        if let Some(closes) = self.closes.get_mut(depth) {
            closes.push(self.pos);
        }
        if block.commented {
            self.dashed_blocks.pop();
        }
        self.bump();
        self.end_node();
        self.node_start = false;
        self.after_block = true;
        self.after_dashed_block = block.dashed;
        self.node_dashed = block.node_dashed;
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
        if dashed && self.node_start && !self.in_type {
            self.node_dashed = Some(self.dash_at);
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

    /// A multi-line string's body, from just after the opening line break through the closing quotes, `start` being
    /// where the string starts. The string ends at the first closing quotes. In a quoted string the lines are split and
    /// dedented after whitespace escapes (`\` and then spaces and line breaks) are taken out, as the spec says, so an
    /// escape can join lines and add to or take from a line's indentation. Then the closing quotes' line has to be
    /// spaces up to them, and every other line has to start with those spaces, unless it's all spaces. Raw strings
    /// have no escapes.
    fn multi_line_body(&mut self, start: usize, close: &str, quoted: bool) -> Result<(), Stop> {
        // Where each line that isn't all spaces starts, and the spaces it starts with.
        let mut lines = Vec::new();
        let mut line = self.pos;
        let mut indent = String::new();
        let mut blank = true;
        loop {
            let at = self.pos;
            match self.peek() {
                None => return Err(Stop::broken(start, "Unclosed string")),
                Some(_) if self.rest().starts_with(close) => {
                    if !blank {
                        return Err(Stop::broken(at, "Closing quotes inside a multi-line string"));
                    }
                    self.pos += close.len();
                    break;
                }
                Some(c) if is_newline(c) => {
                    self.eat_newline();
                    let spaces = std::mem::take(&mut indent);
                    if !blank {
                        lines.push((line, spaces));
                    }
                    line = self.pos;
                    blank = true;
                }
                Some(c) if is_disallowed(c) => return Err(Stop::broken(at, "Character not allowed in KDL")),
                Some('\\') if quoted => {
                    self.bump();
                    if self.peek().is_some_and(is_blank) {
                        while self.peek().is_some_and(is_blank) {
                            self.bump();
                        }
                    } else {
                        self.escape()?;
                        blank = false;
                    }
                }
                Some(c) => {
                    self.bump();
                    if blank && is_space(c) {
                        indent.push(c);
                    } else {
                        blank = false;
                    }
                }
            }
        }
        // `indent` is the closing quotes' line's now.
        for (line, spaces) in lines {
            if !spaces.starts_with(&indent) {
                return Err(Stop::broken(
                    line,
                    "Line doesn't start with the indentation of the closing quotes",
                ));
            }
        }
        Ok(())
    }

    /// From a break on, every character counts, and every `/-` might open one more slashdashed block. None if the rest
    /// of the text fits the allowances.
    fn unchecked(&mut self, at: usize, from: usize, what: &'static str) -> Option<Refusal> {
        let rest = self.text.get(from..).unwrap_or_default();
        let steps: usize = rest.chars().map(|c| if c == '{' { LEVEL_STEPS } else { 2 }).sum();
        self.note(self.blocks.len() * LEVEL_STEPS + steps);
        let mut levels = self.dashed_blocks.len();
        let mut work = self.dashed_work;
        for (i, c) in rest.char_indices() {
            if rest.get(i..).is_some_and(|r| r.starts_with("/-")) {
                levels += 1;
            }
            work = work.saturating_add((c.len_utf8() as u64).saturating_mul(dashed_cost(levels)));
        }
        let help = if steps > MAX_UNCHECKED_STEPS {
            "rkdl stopped at the first error: after an error, kdl-rs's parser can't be trusted not to run out of stack \
             on a document this long"
        } else if work > self.dashed_allowance() {
            "rkdl stopped at the first error: after an error, kdl-rs's parser can't be trusted not to take too long \
             over the slashdashes (`/-`) that follow"
        } else {
            return None;
        };
        Some(Refusal {
            offset: at,
            len: self
                .text
                .get(at..)
                .and_then(|r| r.chars().next())
                .map_or(0, char::len_utf8),
            message: what.into(),
            label: "here".into(),
            help: help.into(),
        })
    }
}

const DASHED_HELP: &str = "kdl-rs's parser reads what's inside a slashdashed block twice, so its time doubles with every \
                           level of `/-` blocks around the text. rkdl refuses text that would take it too long: nest \
                           fewer slashdashed blocks, or delete the text they comment out.";

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The kdl-org/kdl checkout: the `tests/kdl-spec` submodule, or `RKDL_SPEC`, as for the Python tests.
    fn spec() -> PathBuf {
        std::env::var_os("RKDL_SPEC").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/kdl-spec"),
            PathBuf::from,
        )
    }

    /// Every spec case and example that kdl-rs parses must scan without a break, or the scan would treat valid
    /// documents as suspect.
    #[test]
    fn spec_documents_never_break() {
        let root = spec();
        let dirs = [root.join("tests/test_cases/input"), root.join("examples")];
        let files: Vec<PathBuf> = dirs
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flat_map(|dir| dir.flatten().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "kdl"))
            .collect();
        if files.is_empty() {
            // A plain `cargo test` can't skip, so this says why it checked nothing. The Python tests skip loudly too.
            eprintln!(
                "SKIPPED: no KDL spec cases under {}: run `git submodule update --init`, or point RKDL_SPEC at a \
                 kdl-org/kdl checkout",
                root.display()
            );
            return;
        }
        let mut checked = 0;
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.parse::<kdl::KdlDocument>().is_ok() {
                let scan = scan(&text);
                assert_eq!(scan.broke_at, None, "{}", path.display());
                assert!(scan.refusal.is_none(), "{}", path.display());
                checked += 1;
            }
        }
        assert!(checked > 100, "only {checked} spec documents parsed");
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
        // So does each `/` that doesn't open a comment.
        let slashes = |n: usize| scan(&format!("/* {} */", "/".repeat(n))).refusal;
        assert!(slashes(MAX_COMMENT_STEPS - 10).is_none());
        assert!(slashes(MAX_COMMENT_STEPS).is_some());
        // Text between the stars is one piece per stretch.
        assert!(scan(&format!("/*{}*/", "word ".repeat(100_000))).refusal.is_none());
    }

    /// Text kdl-rs's error recovery may read differently from the scan, each followed by enough to go past the
    /// allowance.
    #[test]
    fn breaks() {
        let tail = "a{".repeat(MAX_UNCHECKED_STEPS / LEVEL_STEPS + 1);
        for (text, at, what) in [
            ("a \"\\q \"", 3, "Invalid escape in a string"),
            // `\u` takes braces round its digits, and the digits have to make a Unicode scalar value.
            ("a \"\\u41}\"", 3, "Invalid escape in a string"),
            ("a \"\\u{d800}\"", 3, "Invalid escape in a string"),
            ("a \"\\u{1F600\"", 3, "Invalid escape in a string"),
            ("a \"\"\"\n  \\q\n  \"\"\"", 8, "Invalid escape in a string"),
            ("a \"x\ny\"", 4, "Line break in a single-line string"),
            ("a \"\"\"x\n\"\"\"", 5, "Expected a line break after `\"\"\"`"),
            (
                "a \"\"\"\n x\n  \"\"\"",
                6,
                "Line doesn't start with the indentation of the closing quotes",
            ),
            ("}", 0, "Unexpected `}`: no block is open"),
            ("a )", 2, "Unexpected `)`: no type annotation is open"),
            (
                "a =1",
                2,
                "Unexpected `=`: it goes between a property's key and its value",
            ),
            (
                "a 1=2",
                3,
                "Unexpected `=`: it goes between a property's key and its value",
            ),
            (
                "a (t)k=2",
                6,
                "Unexpected `=`: it goes between a property's key and its value",
            ),
            (
                "a k=v=w",
                5,
                "Unexpected `=`: it goes between a property's key and its value",
            ),
            ("a {} b", 5, "Expected `;` or a line break after this block"),
            // A node has one block of children; only slashdashed ones can come before it.
            ("a {} {}", 5, "Expected `;` or a line break after this block"),
            ("a \"x\"y", 5, "Expected a space or a line break after this value"),
            ("a 1\"x\"", 3, "Expected a space or a line break after this value"),
            ("a (t u)", 5, "Expected `)`: a type annotation holds one name"),
            ("a (t \"u\")1", 5, "Expected `)`: a type annotation holds one name"),
            ("a (t #\"u\"#)1", 5, "Expected `)`: a type annotation holds one name"),
            ("a (t\n)1", 4, "Line break inside a type annotation"),
            ("a (t // c\n)1", 5, "Line break inside a type annotation"),
            ("a (t{}", 4, "Expected `)` to close the type annotation"),
            ("a ((t))1", 3, "Unexpected `(` inside a type annotation"),
            ("a \\ b", 4, "Expected a line break after `\\`"),
            ("a ##true", 4, "Expected `\"` after the `#`s of a raw string"),
            ("a #\"\"\"#", 2, "A single-line raw string can't be just `\"`"),
            ("a \u{1}", 2, "Unexpected character"),
            // Direction overrides can make text read differently from how it parses, so the spec bans them
            // everywhere, strings included.
            ("a \"\u{202e}\"", 3, "Character not allowed in KDL"),
            ("a #\"\u{202e}\"#", 4, "Character not allowed in KDL"),
            ("a \"\"\"\n  \u{202e}\n  \"\"\"", 8, "Character not allowed in KDL"),
            ("a (t)(t)b", 5, "Expected a value after the type annotation"),
            ("a (t)\n1", 5, "Expected a value after the type annotation"),
            ("a /-}", 4, "Expected something for `/-` to comment out"),
            ("a /-/-1", 4, "Expected something for `/-` to comment out"),
        ] {
            let text = format!("{text}\n{tail}");
            let scan = scan(&text);
            assert_eq!(scan.broke_at, Some(at), "{:?}", short(&text));
            assert_eq!(
                scan.refusal.map(|r| r.message).as_deref(),
                Some(what),
                "{:?}",
                short(&text)
            );
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
    fn slashdashed_blocks_cost_by_the_text_inside() {
        let refused = |text: String| scan(&text).refusal.map(|r| (r.message, r.offset));
        let dashed = |n: usize, inside: &str| "a /-{".repeat(n) + inside + &"}".repeat(n);
        assert_eq!(refused(dashed(12, "")), None);
        let message = "Slashdashed blocks nest too deeply".to_owned();
        // Pointing at the `/-` of the innermost level open when the allowance ran out.
        assert_eq!(refused(dashed(14, "")), Some((message, 5 * 12 + 2)));
        // One level costs at most the text again, however long. Two cost it three times over.
        assert_eq!(refused(dashed(1, &"b\n".repeat(1_000_000))), None);
        assert_eq!(refused(dashed(2, &"b\n".repeat(10_000))), None);
        assert!(refused(dashed(2, &"b\n".repeat(100_000))).is_some());
        // The block of a slashdashed node counts, ones inside it don't unless they're slashdashed too.
        assert!(refused("/- a { ".repeat(14) + &"}".repeat(14)).is_some());
        assert!(refused("/- a { b { ".repeat(14)).is_some());
        assert_eq!(refused("/- a { }\nb { ".repeat(14)), None);
        assert_eq!(refused(format!("/- a /-{{}} {{ {} }}", "b { ".repeat(500))), None);
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
            "a \\ /* comment */\n  b",
            "a /*\n multi /* nested */ */ b",
            "a /* and/or */ b",
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

    /// The spec dedents a multi-line string after taking out its whitespace escapes, so an escape can stand in a
    /// line's indentation, join lines, or come before the closing quotes. In raw strings `\` is just text.
    #[test]
    fn whitespace_escapes() {
        let follows = |text: &str| scan(text).broke_at.is_none();
        for text in [
            "a \"x\\  y\"",
            "a \"x\\\n   y\"",
            "a \"x\\ \r\n\t\"",
            "a \"\"\"\n  \\  x\n  \"\"\"",
            "a \"\"\"\n  x\\\n  y\n  \"\"\"",
            "a \"\"\"\n  x \\\ny\n  \"\"\"",
            "a \"\"\"\n  x\n  \\   \"\"\"",
            "a \"\"\"\n  x\n  \\\n  \"\"\"",
            "a \"\"\"\n  \\\n  x\n  \"\"\"",
            "a \"\"\"\n \\ \t x\n \"\"\"",
            "a #\"\"\"\n  \\  x\n  \"\"\"#",
            "a #\"x\\  y\"#",
        ] {
            assert!(text.parse::<kdl::KdlDocument>().is_ok(), "{text:?}");
            assert!(follows(text), "{text:?}");
        }
        for text in [
            // The escape takes the line break before the closing quotes.
            "a \"\"\"\n  x\\\n  \"\"\"",
            // `x` without the two spaces in front.
            "a \"\"\"\n\\  x\n  \"\"\"",
            // The escape takes the line break and the indentation after it.
            "a \"\"\"\n\\\n  x\n  \"\"\"",
            // Raw: the line starts with `\`, not the closing quotes' two spaces.
            "a #\"\"\"\n\\  x\n  \"\"\"#",
        ] {
            assert!(text.parse::<kdl::KdlDocument>().is_err(), "{text:?}");
            assert!(!follows(text), "{text:?}");
        }
    }

    /// Every string kdl-rs takes, from a few characters that make escapes, quotes and line breaks, scans without a
    /// break.
    #[test]
    fn strings_kdl_rs_takes_scan_without_a_break() {
        let alphabet = [" ", "\\", "\n", "x", "\"", "\r", "\t"];
        let mut bodies = vec![String::new()];
        let mut longest = vec![String::new()];
        for _ in 0..5 {
            longest = longest
                .iter()
                .flat_map(|b| alphabet.iter().map(move |a| format!("{b}{a}")))
                .collect();
            bodies.extend(longest.iter().cloned());
        }
        for body in bodies {
            for text in [
                format!("a \"{body}\""),
                format!("a \"\"\"\n{body}\"\"\""),
                format!("a #\"\"\"\n{body}\"\"\"#"),
            ] {
                if text.parse::<kdl::KdlDocument>().is_ok() {
                    assert_eq!(scan(&text).broke_at, None, "{text:?}");
                }
            }
        }
    }
}
