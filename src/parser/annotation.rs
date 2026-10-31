//! Puts back the text kdl-rs 6.7.1 drops around a node's type annotation.
//!
//! kdl-rs parses the whitespace and comments in `( the-type ) the-node-name` into the node's format
//! (`before_ty_name`, `after_ty_name`, `after_ty`), but 6.7.1 doesn't print them: it writes `(the-type)the-node-name`.
//! Those fields stay the record of the text. A copy is folded into what does print: the annotation's text becomes
//! `before_ty_name`, its spelling, `after_ty_name`, and the name's text becomes `after_ty` and its spelling. An edit
//! to the annotation or the name unfolds first, so it sees the plain spelling, and folds again after.
//!
//! The padding never starts or ends a spelling (an identifier or string can't start with whitespace, `/` or `\`, or
//! end with them), so unfolding finds it again by comparing. Folding only touches a node with an annotation, and
//! unfolding twice, or folding a node that wasn't unfolded, changes nothing.
//!
//! When kdl-rs prints these fields itself (fixed on its main branch after 6.7.1), delete this module and its callers.

use kdl::{KdlDocument, KdlIdentifier, KdlNode};

/// Folds the padding of every node in a freshly parsed document.
pub(crate) fn restore(doc: &mut KdlDocument) {
    let mut pending = vec![doc];
    while let Some(block) = pending.pop() {
        for node in block.nodes_mut() {
            fold(node);
            if let Some(children) = node.children_mut() {
                pending.push(children);
            }
        }
    }
}

/// Runs `edit` on `node` with its annotation and name in their plain spellings, then folds the padding back in.
pub(crate) fn editing<T>(node: &mut KdlNode, edit: impl FnOnce(&mut KdlNode) -> T) -> T {
    unfold(node);
    let out = edit(node);
    fold(node);
    out
}

/// The padding as (inside before the type, inside after it, between `)` and the name).
fn padding(node: &KdlNode) -> Option<(String, String, String)> {
    let f = node.format()?;
    let all = [&f.before_ty_name, &f.after_ty_name, &f.after_ty];
    if all.iter().all(|s| s.is_empty()) {
        return None;
    }
    Some((f.before_ty_name.clone(), f.after_ty_name.clone(), f.after_ty.clone()))
}

fn fold(node: &mut KdlNode) {
    let Some((before, after, after_ty)) = padding(node) else {
        return;
    };
    let Some(ty) = node.ty_mut() else {
        return;
    };
    let inside = !before.is_empty() || !after.is_empty();
    if inside && !folded(ty, &before, &after) {
        let text = ty.to_string();
        ty.set_repr(format!("{before}{text}{after}"));
    }
    let name = node.name_mut();
    if !after_ty.is_empty() && !folded(name, &after_ty, "") {
        let text = name.to_string();
        name.set_repr(format!("{after_ty}{text}"));
    }
}

fn unfold(node: &mut KdlNode) {
    let Some((before, after, after_ty)) = padding(node) else {
        return;
    };
    if let Some(ty) = node.ty_mut() {
        strip(ty, &before, &after);
    }
    // Also without an annotation, in case it went without unfolding.
    strip(node.name_mut(), &after_ty, "");
}

fn folded(ident: &KdlIdentifier, before: &str, after: &str) -> bool {
    ident.repr().is_some_and(|repr| unpadded(repr, before, after).is_some())
}

fn unpadded<'a>(repr: &'a str, before: &str, after: &str) -> Option<&'a str> {
    let inner = repr.strip_prefix(before)?.strip_suffix(after)?;
    // An empty inner text is what was there, not padding: nothing was folded.
    (!inner.is_empty() && (!before.is_empty() || !after.is_empty())).then_some(inner)
}

fn strip(ident: &mut KdlIdentifier, before: &str, after: &str) {
    if let Some(inner) = ident.repr().and_then(|repr| unpadded(repr, before, after)) {
        let inner = inner.to_owned();
        ident.set_repr(inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> String {
        let mut doc: KdlDocument = text.parse().expect("test documents parse");
        restore(&mut doc);
        doc.to_string()
    }

    #[test]
    fn text_around_an_annotation_comes_back() {
        for text in [
            "(dev)  dep 1",
            "(dev)/*hey*/dep 1",
            "( the-type ) the-node-name",
            "(\t\"q t\" /* c */)\t\"q n\"",
            "a { (t)  b }",
            "a {\n    ( t ) \\\n    b\n}\n",
            "(t)n",
            "(t) n; (u)m",
        ] {
            assert_eq!(round_trip(text), text);
        }
    }

    #[test]
    fn edits_see_the_plain_spelling() {
        let mut doc: KdlDocument = "( \"t\" )  \"n\" 1".parse().expect("parses");
        restore(&mut doc);
        let node = &mut doc.nodes_mut()[0];
        editing(node, |n| {
            assert_eq!(n.ty().and_then(KdlIdentifier::repr), Some("\"t\""));
            assert_eq!(n.name().repr(), Some("\"n\""));
            n.set_ty("u");
        });
        assert_eq!(doc.to_string(), "( u )  \"n\" 1");
        // Unfolding twice, and folding twice, change nothing.
        let node = &mut doc.nodes_mut()[0];
        unfold(node);
        unfold(node);
        fold(node);
        fold(node);
        assert_eq!(doc.to_string(), "( u )  \"n\" 1");
    }

    #[test]
    fn an_annotation_dropped_without_unfolding_leaves_the_name_plain() {
        // The padding stays on the node, but folding only touches a node with an annotation, and unfolding strips the
        // name's padding all the same. `(t)  n` without its annotation is `n` (node.rs, `set_type`).
        let mut doc: KdlDocument = "( t )  n 1".parse().expect("parses");
        restore(&mut doc);
        *doc.nodes_mut()[0].ty_mut() = None;
        editing(&mut doc.nodes_mut()[0], |_| ());
        assert_eq!(doc.to_string(), "n 1");
        fold(&mut doc.nodes_mut()[0]);
        assert_eq!(doc.to_string(), "n 1");
    }

    #[test]
    fn text_that_is_all_padding_is_not_folded() {
        assert_eq!(unpadded(" n", " ", ""), Some("n"));
        // An identifier whose whole text is the padding wasn't folded: stripping it would blank the identifier.
        assert_eq!(unpadded("/*c*/", "/*c*/", ""), None);
    }
}
