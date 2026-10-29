use kdl::{FormatConfig, KdlDocument};
use pyo3::prelude::*;
use pyo3::types::PyString;

/// Autoformat settings, mirroring kdl-rs's FormatConfig.
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
    fn new(indent: String, indent_level: usize, no_comments: bool, entry_autoformat_keep: bool) -> Self {
        Formatter {
            indent,
            indent_level,
            no_comments,
            entry_autoformat_keep,
        }
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

impl Default for Formatter {
    fn default() -> Self {
        Formatter::new("    ".to_string(), 0, false, false)
    }
}

impl Formatter {
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
    }
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}
