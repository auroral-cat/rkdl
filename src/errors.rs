use kdl::{KdlDiagnostic, KdlError};
use std::fmt;
use std::sync::Arc;

use miette::{GraphicalReportHandler, GraphicalTheme, LabeledSpan, NamedSource, Severity, SourceCode};
use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

create_exception!(
    rkdl,
    ParseError,
    PyValueError,
    concat!(
        "The text isn't valid KDL.\n",
        "\n",
        "`str(error)` is a report that points at each problem in the text. The attributes give the first problem's\n",
        "position, counted in characters.\n",
        "\n",
        "Attributes:\n",
        "    line: The first problem's line, starting at 1.\n",
        "    column: The first problem's column, starting at 1.\n",
        "    offset: The first problem's offset from the start of the text, starting at 0.\n",
        "    diagnostics: Every problem found, in order.",
    )
);

// It can be built and pickled, so a `ParseError` holding some can cross a process boundary.
/// One problem found while parsing.
///
/// Positions count characters, not bytes. Diagnostics compare equal when all their fields do, and can be pickled.
#[pyclass(module = "rkdl", frozen, get_all, eq, hash)]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// What's wrong.
    message: String,
    /// A short note on the problem's span of text, if there is one.
    label: Option<String>,
    /// A hint on how to fix it, if there is one.
    help: Option<String>,
    /// Where the problem starts, in characters from the start of the text.
    offset: usize,
    /// How many characters the problem spans.
    length: usize,
    /// The line the problem starts on, starting at 1.
    line: usize,
    /// The column the problem starts at, starting at 1.
    column: usize,
}

#[pymethods]
impl Diagnostic {
    #[new]
    #[pyo3(signature = (message, label, help, offset, length, line, column))]
    fn new(
        message: String,
        label: Option<String>,
        help: Option<String>,
        offset: usize,
        length: usize,
        line: usize,
        column: usize,
    ) -> Self {
        Diagnostic {
            message,
            label,
            help,
            offset,
            length,
            line,
            column,
        }
    }

    fn __getnewargs__(&self) -> (String, Option<String>, Option<String>, usize, usize, usize, usize) {
        let d = self.clone();
        (d.message, d.label, d.help, d.offset, d.length, d.line, d.column)
    }

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
    parse_error_in(py, err, None)
}

/// `parse_error` for text read from `path`, which the report names.
pub(crate) fn parse_error_in(py: Python<'_>, err: &KdlError, path: Option<&str>) -> PyErr {
    let diagnostics: Vec<Diagnostic> = err
        .diagnostics
        .iter()
        .map(|d| Diagnostic::from_kdl(&err.input, d))
        .collect();
    let first = diagnostics.first().map(|d| (d.line, d.column, d.offset));
    let report = match path {
        Some(path) => {
            let (line, column) = first.map_or((1, 1), |f| (f.0, f.1));
            render(&InFile::new(err, path, line, column))
        }
        None => render(err),
    };
    let exc = ParseError::new_err(report);
    let value = exc.value(py);
    let set = value
        .setattr("line", first.map(|f| f.0))
        .and_then(|()| value.setattr("column", first.map(|f| f.1)))
        .and_then(|()| value.setattr("offset", first.map(|f| f.2)))
        .and_then(|()| value.setattr("diagnostics", diagnostics))
        .and_then(|()| value.setattr("path", path));
    match set {
        Ok(()) => exc,
        Err(e) => e,
    }
}

/// Exception text ends up in log files, so render without colour or terminal hyperlinks.
fn render(err: &dyn miette::Diagnostic) -> String {
    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor()).with_links(false);
    let mut out = String::new();
    match handler.render_report(&mut out, err) {
        Ok(()) => out.trim_end().to_owned(),
        Err(_) => err.to_string(),
    }
}

/// A kdl-rs error with its source named after a file, so the report's first line reads `config.kdl:4:9: ...` and each
/// snippet's header is `[config.kdl:4:9]`, as miette prints named sources.
#[derive(Debug)]
struct InFile {
    title: String,
    source: NamedSource<Arc<String>>,
    diagnostics: Vec<Located>,
}

#[derive(Debug)]
struct Located {
    diagnostic: KdlDiagnostic,
    source: NamedSource<Arc<String>>,
}

impl InFile {
    fn new(err: &KdlError, path: &str, line: usize, column: usize) -> Self {
        let source = NamedSource::new(path, Arc::clone(&err.input));
        InFile {
            title: format!("{path}:{line}:{column}: {err}"),
            diagnostics: err
                .diagnostics
                .iter()
                .map(|d| Located {
                    diagnostic: d.clone(),
                    source: source.clone(),
                })
                .collect(),
            source,
        }
    }
}

impl fmt::Display for InFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.title)
    }
}

impl std::error::Error for InFile {}

impl miette::Diagnostic for InFile {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn miette::Diagnostic> + 'a>> {
        Some(Box::new(self.diagnostics.iter().map(|d| d as &dyn miette::Diagnostic)))
    }
}

impl fmt::Display for Located {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostic.fmt(f)
    }
}

impl std::error::Error for Located {}

// Everything but the source is kdl-rs's.
impl miette::Diagnostic for Located {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn severity(&self) -> Option<Severity> {
        miette::Diagnostic::severity(&self.diagnostic)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        miette::Diagnostic::help(&self.diagnostic)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        miette::Diagnostic::labels(&self.diagnostic)
    }
}
