//! Spotting KDL v1 spellings where a parse error points, so the error can say the file looks like v1 and what the v2
//! form is. kdl-rs reports them as whatever v2 rule they break first ("Expected identifier string" for a bare
//! `true`), which says nothing about versions. This only reads the text at the error's span: it's no v1 parser.

use kdl::KdlEntry;

/// The v2 form to put in front of a v2 spelling longer than this is left out, so the help stays one short line.
const SHOWN: usize = 40;

/// The help for an error whose span starts at byte `start` of `input`, if the text there is a v1-only spelling.
pub(crate) fn hint(input: &str, start: usize) -> Option<String> {
    let (before, rest) = input.split_at_checked(start)?;
    let (v2, general) = keyword(before, rest)
        .or_else(|| raw_string(rest))
        .or_else(|| escaped_slash(rest))?;
    // The v2 form is shown only if it's short and kdl-rs reads it as a value.
    Some(match v2 {
        Some(v2) if v2.chars().count() <= SHOWN && KdlEntry::parse(&v2).is_ok() => {
            format!("this looks like KDL v1: in KDL v2 it's {v2}")
        }
        _ => format!("this looks like KDL v1: {general}"),
    })
}

type Found = (Option<String>, &'static str);

/// `true`, `false` and `null` as a value: v2 spells them `#true`, `#false`, `#null`. In v1 they couldn't be node
/// names, property keys or type annotations either, so those positions get no hint.
fn keyword(before: &str, rest: &str) -> Option<Found> {
    let (word, after) = ["true", "false", "null"].into_iter().find_map(|w| {
        let after = rest.strip_prefix(w)?;
        (!after.starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_')).then_some((w, after))
    })?;
    let after = after.trim_start_matches([' ', '\t']);
    if after.starts_with(['=', ')']) || !after_node_name(before) {
        return None;
    }
    Some((
        Some(format!("#{word}")),
        "in KDL v2 true, false and null are #true, #false and #null",
    ))
}

/// Whether `before`, the text up to a token, ends inside a node, past its name: the token isn't a node name or a type
/// annotation. A line continuation before the token reads as a new line, so it gets no hint.
fn after_node_name(before: &str) -> bool {
    let mut before = before.trim_end_matches([' ', '\t']);
    loop {
        match before.chars().next_back() {
            None | Some('\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}' | ';' | '{' | '(') => {
                return false;
            }
            // An annotated token, `(t)true`: what counts is what's before the annotation.
            Some(')') => match before.rfind('(').and_then(|open| before.get(..open)) {
                Some(outside) => before = outside.trim_end_matches([' ', '\t']),
                None => return false,
            },
            Some(_) => return true,
        }
    }
}

/// A v1 raw string, `r"..."` or `r#"..."#`: v2 drops the `r` and needs at least one `#`.
fn raw_string(rest: &str) -> Option<Found> {
    let hashes = rest.strip_prefix('r')?;
    let quoted = hashes.trim_start_matches('#');
    let n = hashes.len() - quoted.len();
    let body = quoted.strip_prefix('"')?;
    let v2 = body.split_once(&format!("\"{}", "#".repeat(n))).map(|(inner, _)| {
        let fence = "#".repeat(n.max(1));
        format!("{fence}\"{inner}\"{fence}")
    });
    Some((v2, "in KDL v2 raw strings are written #\"...\"#, without the r"))
}

/// A quoted string with a `\/` escape, which v2 dropped: `/` is written as it is.
fn escaped_slash(rest: &str) -> Option<Found> {
    let inner = rest.strip_prefix('"')?;
    if inner.starts_with("\"\"") {
        return None;
    }
    let mut v2 = String::from('"');
    let mut found = false;
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                v2.push('"');
                break;
            }
            '\\' => match chars.next() {
                Some('/') => {
                    found = true;
                    v2.push('/');
                }
                Some(next) => {
                    v2.push('\\');
                    v2.push(next);
                }
                None => break,
            },
            _ => v2.push(c),
        }
    }
    found.then_some((Some(v2), "in KDL v2 / isn't escaped: write \\/ as /"))
}

#[cfg(test)]
mod tests {
    use super::hint;

    fn at(input: &str, token: &str) -> Option<String> {
        hint(input, input.find(token).unwrap())
    }

    #[test]
    fn keywords_as_values() {
        assert_eq!(
            at("node true", "true").as_deref(),
            Some("this looks like KDL v1: in KDL v2 it's #true")
        );
        assert!(at("node a=null", "null").is_some());
        assert!(at("node (t)false", "false").is_some());
    }

    #[test]
    fn keywords_elsewhere() {
        assert_eq!(at("true 1", "true"), None);
        assert_eq!(at("node true=1", "true"), None);
        assert_eq!(at("node (true)1", "true"), None);
        assert_eq!(at("node { true; }", "true"), None);
        assert_eq!(at("node truex", "true"), None);
    }

    #[test]
    fn raw_strings() {
        assert_eq!(
            at(r#"node r"a\b""#, "r").as_deref(),
            Some(r##"this looks like KDL v1: in KDL v2 it's #"a\b"#"##)
        );
        assert_eq!(
            at("node r##\"x\"#\"##", "r").as_deref(),
            Some("this looks like KDL v1: in KDL v2 it's ##\"x\"#\"##")
        );
        assert!(at("node r\"open", "r").unwrap().contains("without the r"));
        assert!(at("node r#\"two\nlines\"#", "r").unwrap().contains("without the r"));
    }

    #[test]
    fn escaped_slashes() {
        assert_eq!(
            at(r#"node "a\/b""#, "\"").as_deref(),
            Some(r#"this looks like KDL v1: in KDL v2 it's "a/b""#)
        );
        assert_eq!(at(r#"node "a\\b""#, "\""), None);
    }
}
