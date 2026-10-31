//! Writing a new value, name, key or type in the style of the text it replaces: a quoted string stays quoted, a raw
//! one raw, a hex number hex. Every spelling made here is parsed back before it's used; one that doesn't read back as
//! the new value is dropped for the canonical one.

use kdl::{KdlEntry, KdlIdentifier, KdlValue};

use crate::values::{escape_into, quoted, reads_back_as, same_value};

/// The text to print for `value` where `old` was printed before, or None to print the canonical text.
pub(crate) fn value(old: &str, value: &KdlValue) -> Option<String> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "keywords and null have one spelling each, whatever other values kdl-rs grows"
    )]
    let candidate = match value {
        KdlValue::String(s) => return string(old, s),
        KdlValue::Integer(i) => integer(old, *i)?,
        KdlValue::Float(f) if f.is_finite() => float(old, *f)?,
        _ => return None,
    };
    // `integer` and `float` spell only signs, prefixes, digits and exponents, never a property or an annotation.
    KdlEntry::parse(&candidate)
        .is_ok_and(|e| same_value(e.value(), value))
        .then_some(candidate)
}

/// A name, key or type annotation `new`, spelled like `old` if `old` was quoted.
pub(crate) fn identifier(old: Option<&KdlIdentifier>, new: String) -> KdlIdentifier {
    let spelled = old.and_then(KdlIdentifier::repr).and_then(|repr| string(repr, &new));
    let mut ident = crate::values::identifier(new);
    if let Some(repr) = spelled {
        ident.set_repr(repr);
    }
    ident
}

// Strings.

/// How a string was written.
enum Style {
    Quoted,
    Raw {
        hashes: usize,
    },
    MultiLine {
        hashes: Option<usize>,
        indent: String,
        eol: &'static str,
    },
}

impl Style {
    /// None for a bare identifier, a keyword or a number, which have nothing to keep.
    fn of(text: &str) -> Option<Style> {
        let hashes = text.len() - text.trim_start_matches('#').len();
        let rest = text.get(hashes..)?;
        let raw = (hashes > 0).then_some(hashes);
        if let Some(body) = rest.strip_prefix("\"\"\"") {
            // The closing line's whitespace is the indentation every line of the string carries.
            let closing = body.trim_end_matches('#').strip_suffix("\"\"\"")?;
            let indent = closing.rsplit(['\n', '\r']).next()?;
            let eol = if body.contains("\r\n") { "\r\n" } else { "\n" };
            return Some(Style::MultiLine {
                hashes: raw,
                indent: indent.to_owned(),
                eol,
            });
        }
        if !rest.starts_with('"') {
            return None;
        }
        Some(match raw {
            Some(hashes) => Style::Raw { hashes },
            None => Style::Quoted,
        })
    }
}

/// `s` in the style of the string literal `old`, or None if `old` wasn't one. A multi-line string stays multi-line
/// while the new value has a line break, and a raw one stays raw while it can be.
fn string(old: &str, s: &str) -> Option<String> {
    let multi = s.contains('\n');
    let candidate = match Style::of(old)? {
        Style::Raw { hashes } if !multi => raw(s, hashes),
        Style::MultiLine { hashes, indent, eol } if multi => multi_line(s, hashes, &indent, eol),
        Style::MultiLine {
            hashes: Some(hashes), ..
        } => raw(s, hashes),
        Style::Quoted | Style::Raw { .. } | Style::MultiLine { hashes: None, .. } => return Some(quoted(s)),
    };
    // A raw string can't escape what it can't hold, and a multi-line one loses lines of only whitespace.
    Some(if reads_back_as(&candidate, s) {
        candidate
    } else {
        quoted(s)
    })
}

/// The fewest hashes from `at_least` up that `s` can't end early. Past `s.len()` hashes, none can be in `s`.
fn hashes_for(s: &str, close: &str, at_least: usize) -> usize {
    let enough = at_least.max(s.len() + 1);
    (at_least..enough)
        .find(|&n| !s.contains(&format!("{close}{}", "#".repeat(n))))
        .unwrap_or(enough)
}

fn raw(s: &str, at_least: usize) -> String {
    let hashes = "#".repeat(hashes_for(s, "\"", at_least.max(1)));
    format!("{hashes}\"{s}\"{hashes}")
}

fn multi_line(s: &str, raw: Option<usize>, indent: &str, eol: &str) -> String {
    let hashes = "#".repeat(raw.map_or(0, |n| hashes_for(s, "\"\"\"", n)));
    let mut out = format!("{hashes}\"\"\"{eol}");
    for line in s.split('\n') {
        if !line.is_empty() {
            out.push_str(indent);
        }
        match raw {
            Some(_) => out.push_str(line),
            None => escape_line(line, &mut out),
        }
        out.push_str(eol);
    }
    out.push_str(indent);
    out.push_str("\"\"\"");
    out.push_str(&hashes);
    out
}

/// One line of a multi-line string, escaped. Quotes are escaped only where three in a row would close the string.
fn escape_line(line: &str, out: &mut String) {
    let mut quotes = 0;
    for c in line.chars() {
        if c == '"' {
            quotes += 1;
            if quotes == 3 {
                out.push_str("\\\"");
                quotes = 0;
            } else {
                out.push('"');
            }
            continue;
        }
        quotes = 0;
        escape_into(c, out);
    }
}

// Numbers.

/// A number literal taken apart: `-0x1F_90` is sign `-`, prefix `0x`, digits `1F_90`.
struct Number<'a> {
    plus: bool,
    prefix: &'a str,
    digits: &'a str,
}

impl Number<'_> {
    fn of(text: &str) -> Option<Number<'_>> {
        let (plus, unsigned) = match text.as_bytes().first()? {
            b'+' => (true, text.get(1..)?),
            b'-' => (false, text.get(1..)?),
            _ => (false, text),
        };
        if !unsigned.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let prefix = ["0x", "0o", "0b"]
            .into_iter()
            .find(|p| unsigned.starts_with(p))
            .unwrap_or("");
        Some(Number {
            plus,
            prefix,
            digits: unsigned.get(prefix.len()..)?,
        })
    }

    fn is_integer(&self) -> bool {
        !self.prefix.is_empty() || !self.digits.contains(['.', 'e', 'E'])
    }
}

/// `i` in the radix, digit case and digit grouping of the integer literal `old`.
fn integer(old: &str, i: i128) -> Option<String> {
    let old = Number::of(old)?;
    if !old.is_integer() {
        return None;
    }
    let n = i.unsigned_abs();
    let digits = match old.prefix {
        "0x" if old.digits.contains(|c: char| c.is_ascii_uppercase()) => format!("{n:X}"),
        "0x" => format!("{n:x}"),
        "0o" => format!("{n:o}"),
        "0b" => format!("{n:b}"),
        _ => n.to_string(),
    };
    let digits = match group_size(old.digits) {
        Some(size) => grouped(&digits, size),
        None => digits,
    };
    let sign = match (i < 0, old.plus) {
        (true, _) => "-",
        (false, true) => "+",
        (false, false) => "",
    };
    Some(format!("{sign}{}{digits}", old.prefix))
}

/// The size of the digit groups in `digits`, if it's grouped regularly from the right, like `1_000_000` or `dead_beef`.
fn group_size(digits: &str) -> Option<usize> {
    let groups: Vec<&str> = digits.split('_').collect();
    let (first, rest) = groups.split_first()?;
    let size = rest.first()?.len();
    let regular = rest.iter().all(|g| g.len() == size) && (1..=size).contains(&first.len());
    regular.then_some(size)
}

fn grouped(digits: &str, size: usize) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / size);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(size) {
            out.push('_');
        }
        out.push(c);
    }
    out
}

/// `f` in exponent form if the decimal literal `old` had an exponent, with the same `e` or `E`.
fn float(old: &str, f: f64) -> Option<String> {
    let old = Number::of(old)?;
    if old.is_integer() {
        return None;
    }
    let (_, exponent) = old.digits.split_once(['e', 'E'])?;
    let text = format!("{f:e}");
    let (mantissa, power) = text.split_once('e')?;
    let e = if old.digits.contains('E') { "E" } else { "e" };
    let plus = if exponent.starts_with('+') && !power.starts_with('-') {
        "+"
    } else {
        ""
    };
    let sign = if old.plus && f.is_sign_positive() { "+" } else { "" };
    Some(format!("{sign}{mantissa}{e}{plus}{power}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spelled(old: &str, new: impl Into<KdlValue>) -> Option<String> {
        value(old, &new.into())
    }

    #[test]
    fn strings_keep_their_quoting() {
        assert_eq!(spelled("\"a\"", "b").as_deref(), Some("\"b\""));
        assert_eq!(spelled("a", "b"), None);
        assert_eq!(spelled("#\"a\\b\"#", "c\\d").as_deref(), Some("#\"c\\d\"#"));
        assert_eq!(spelled("#\"a\"#", "x\"#y").as_deref(), Some("##\"x\"#y\"##"));
        assert_eq!(spelled("#\"a\"#", "x\ny").as_deref(), Some("\"x\\ny\""));
        assert_eq!(
            spelled("\"\"\"\n  a\n  \"\"\"", "b\nc").as_deref(),
            Some("\"\"\"\n  b\n  c\n  \"\"\"")
        );
        assert_eq!(spelled("\"\"\"\n  a\n  \"\"\"", "b").as_deref(), Some("\"b\""));
        assert_eq!(spelled("123", "b"), None);
    }

    #[test]
    fn integers_keep_radix_and_grouping() {
        assert_eq!(spelled("0x10", 255).as_deref(), Some("0xff"));
        assert_eq!(spelled("0xAB", 255).as_deref(), Some("0xFF"));
        assert_eq!(spelled("-0b1", 5).as_deref(), Some("0b101"));
        assert_eq!(spelled("1_000", 1_234_567).as_deref(), Some("1_234_567"));
        assert_eq!(spelled("12_34", 123_456).as_deref(), Some("12_34_56"));
        assert_eq!(spelled("123_4", 5).as_deref(), Some("5"));
        assert_eq!(spelled("+1", 2).as_deref(), Some("+2"));
        assert_eq!(spelled("1.5", 2), None);
    }

    #[test]
    fn floats_keep_exponent_form() {
        assert_eq!(spelled("1.0e10", 2.5e-7).as_deref(), Some("2.5e-7"));
        assert_eq!(spelled("1E+3", 2.0e5).as_deref(), Some("2E+5"));
        assert_eq!(spelled("1.5", 2.5), None);
        assert_eq!(spelled("1e3", f64::NAN), None);
    }
}
