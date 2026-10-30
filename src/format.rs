use kdl::{FormatConfig, KdlDocument, KdlEntry};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::parser::prescan::KDL_SPACES;
use crate::values;

/// Far deeper than any real file. A huge level makes kdl-rs loop until allocation fails, which aborts the process.
const MAX_INDENT_LEVEL: usize = 1000;

/// Autoformat settings, mirroring kdl-rs's `FormatConfig`.
#[pyclass(module = "rkdl", frozen)]
pub struct Formatter {
    #[pyo3(get)]
    indent: String,
    #[pyo3(get)]
    indent_level: usize,
    #[pyo3(get)]
    no_comments: bool,
    #[pyo3(get)]
    entry_autoformat_keep: bool,
}

#[pymethods]
impl Formatter {
    #[new]
    #[pyo3(signature = (indent="    ".to_string(), indent_level=0, no_comments=false, entry_autoformat_keep=false))]
    fn new(indent: String, indent_level: usize, no_comments: bool, entry_autoformat_keep: bool) -> PyResult<Self> {
        // autoformat writes the indent as is, so anything but spaces would change what the document says.
        if let Some(c) = indent.chars().find(|c| !KDL_SPACES.contains(c)) {
            return Err(PyValueError::new_err(format!(
                "indent must be KDL whitespace, such as spaces or tabs, not {c:?}"
            )));
        }
        if indent_level > MAX_INDENT_LEVEL {
            return Err(PyValueError::new_err(format!(
                "indent_level must be at most {MAX_INDENT_LEVEL}, not {indent_level}"
            )));
        }
        Ok(Formatter {
            indent,
            indent_level,
            no_comments,
            entry_autoformat_keep,
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        // Python's repr of the indent, so the quoting and escapes match what a user would type.
        let indent = PyString::new(py, &self.indent).repr()?;
        Ok(format!(
            "Formatter(indent={indent}, indent_level={}, no_comments={}, entry_autoformat_keep={})",
            self.indent_level,
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

    pub(crate) fn apply(&self, doc: &mut KdlDocument) {
        let mut config = FormatConfig::builder()
            .indent(&self.indent)
            .indent_level(self.indent_level)
            .no_comments(self.no_comments)
            .build();
        // The builder has no setter for this flag, and FormatConfig is non_exhaustive so `..` can't build one from
        // outside kdl, but the field itself is public.
        config.entry_autoformate_keep = self.entry_autoformat_keep;
        doc.autoformat_config(&config);
        requote(doc);
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
