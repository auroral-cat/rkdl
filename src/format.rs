use kdl::{FormatConfig, KdlDocument, KdlEntry};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::layout;
use crate::parser::prescan::KDL_SPACES;
use crate::values;

/// Settings for `Document.fmt`, which mirror kdl-rs's `FormatConfig`.
///
/// Args:
///     indent: The text for one level of indentation. Only KDL whitespace, such as spaces or tabs.
///     no_comments: Remove comments.
///     entry_autoformat_keep: Leave each argument and property as written, with the space in front of it, and only
///         reformat the nodes around them.
///
/// Raises:
///     ValueError: `indent` holds something other than whitespace.
// No `indent_level`, though FormatConfig has one: it indents the whole output, and `Document.fmt` always formats a
// whole file, where indented top-level nodes have no use. kdl-rs 6.7.1 also indents the first top-level node twice.
#[pyclass(module = "rkdl", frozen)]
pub struct Formatter {
    /// The text for one level of indentation.
    #[pyo3(get)]
    indent: String,
    /// Whether comments are removed.
    #[pyo3(get)]
    no_comments: bool,
    /// Whether arguments and properties are left as written.
    #[pyo3(get)]
    entry_autoformat_keep: bool,
}

#[pymethods]
impl Formatter {
    #[new]
    #[pyo3(
        signature = (indent="    ".to_string(), no_comments=false, entry_autoformat_keep=false),
        text_signature = "(indent='    ', no_comments=False, entry_autoformat_keep=False)"
    )]
    fn new(indent: String, no_comments: bool, entry_autoformat_keep: bool) -> PyResult<Self> {
        // autoformat writes the indent as is, so anything but spaces would change what the document says.
        if let Some(c) = indent.chars().find(|c| !KDL_SPACES.contains(c)) {
            return Err(PyValueError::new_err(format!(
                "indent must be KDL whitespace, such as spaces or tabs, not {c:?}"
            )));
        }
        Ok(Formatter {
            indent,
            no_comments,
            entry_autoformat_keep,
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        // Python's repr of the indent, so the quoting and escapes match what a user would type.
        let indent = PyString::new(py, &self.indent).repr()?;
        Ok(format!(
            "Formatter(indent={indent}, no_comments={}, entry_autoformat_keep={})",
            py_bool(self.no_comments),
            py_bool(self.entry_autoformat_keep),
        ))
    }
}

impl Formatter {
    /// `Formatter()`, made through Python so the defaults are written only in the constructor's signature.
    pub(crate) fn default(py: Python<'_>) -> PyResult<Bound<'_, Formatter>> {
        Ok(py.get_type::<Formatter>().call0()?.cast_into()?)
    }

    /// Formats `doc`, keeping its line ending style `eol`: autoformat writes `\n`.
    pub(crate) fn apply(&self, doc: &mut KdlDocument, eol: &str) {
        let mut config = FormatConfig::builder()
            .indent(&self.indent)
            .no_comments(self.no_comments)
            .build();
        // The builder has no setter for this flag, and FormatConfig is non_exhaustive so `..` can't build one from
        // outside kdl, but the field itself is public.
        config.entry_autoformate_keep = self.entry_autoformat_keep;
        doc.autoformat_config(&config);
        requote(doc);
        layout::restyle_document(doc, eol);
    }
}

/// autoformat clears the text rkdl wrote for names, keys, types and string values, and kdl-rs's own spelling of
/// some strings doesn't parse back (see `values::fixed_repr`). Puts the fixed spelling back where it's needed.
fn requote(doc: &mut KdlDocument) {
    // A work list rather than recursion, so nesting depth isn't limited by the stack.
    let mut pending = vec![doc];
    while let Some(block) = pending.pop() {
        for node in block.nodes_mut() {
            values::fix_identifier(node.name_mut());
            if let Some(ty) = node.ty_mut() {
                values::fix_identifier(ty);
            }
            for entry in node.entries_mut() {
                requote_entry(entry);
            }
            if let Some(children) = node.children_mut() {
                pending.push(children);
            }
        }
    }
}

fn requote_entry(entry: &mut KdlEntry) {
    if let Some(name) = entry.name_mut() {
        values::fix_identifier(name);
    }
    if let Some(ty) = entry.ty_mut() {
        values::fix_identifier(ty);
    }
    let text = match entry.format() {
        Some(f) => f.value_repr.clone(),
        None => entry.value().to_string(),
    };
    if let Some(repr) = values::fixed_repr(&text, entry.value()) {
        values::set_value_repr(entry, repr);
    }
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}
