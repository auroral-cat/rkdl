use kdl::{KdlDiagnostic, KdlError};
use miette::{GraphicalReportHandler, GraphicalTheme};
use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

create_exception!(rkdl, ParseError, PyValueError);

/// One problem found while parsing. Offsets count code points, not bytes.
#[pyclass(module = "rkdl", frozen, get_all)]
pub struct Diagnostic {
    message: String,
    label: Option<String>,
    help: Option<String>,
    offset: usize,
    length: usize,
    line: usize,
    column: usize,
}

#[pymethods]
impl Diagnostic {
    fn __repr__(&self) -> String {
        format!("<Diagnostic {}:{} {:?}>", self.line, self.column, self.message)
    }
}

impl Diagnostic {
    fn from_kdl(input: &str, diag: &KdlDiagnostic) -> Self {
        // kdl-rs spans are byte offsets into the UTF-8 input. Rounding them down to a character boundary means a span
        // that lands inside a character gives a slightly-off position instead of a panic.
        let start = input.floor_char_boundary(diag.span.offset());
        let end = input.floor_char_boundary(start.saturating_add(diag.span.len()));
        let before = input.get(..start).unwrap_or_default();
        let spanned = input.get(start..end).unwrap_or_default();
        let last_line = before.rsplit('\n').next().unwrap_or_default();
        Diagnostic {
            message: diag.message.clone().unwrap_or_else(|| "unexpected error".into()),
            label: diag.label.clone(),
            help: diag.help.clone(),
            offset: before.chars().count(),
            length: spanned.chars().count(),
            line: before.matches('\n').count() + 1,
            column: last_line.chars().count() + 1,
        }
    }
}

pub(crate) fn parse_error(py: Python<'_>, err: &KdlError) -> PyErr {
    let diagnostics: Vec<Diagnostic> = err
        .diagnostics
        .iter()
        .map(|d| Diagnostic::from_kdl(&err.input, d))
        .collect();
    let first = diagnostics.first().map(|d| (d.line, d.column, d.offset));
    let exc = ParseError::new_err(render(err));
    let value = exc.value(py);
    let set = value
        .setattr("line", first.map(|f| f.0))
        .and_then(|()| value.setattr("column", first.map(|f| f.1)))
        .and_then(|()| value.setattr("offset", first.map(|f| f.2)))
        .and_then(|()| value.setattr("diagnostics", diagnostics));
    match set {
        Ok(()) => exc,
        Err(e) => e,
    }
}

/// Exception text ends up in log files, so render without colour or terminal hyperlinks.
fn render(err: &KdlError) -> String {
    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor()).with_links(false);
    let mut out = String::new();
    match handler.render_report(&mut out, err) {
        Ok(()) => out.trim_end().to_owned(),
        Err(_) => err.to_string(),
    }
}
