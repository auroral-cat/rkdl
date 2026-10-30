use kdl::{KdlDiagnostic, KdlError};
use std::fmt;
use std::sync::Arc;

use miette::{
    GraphicalReportHandler, GraphicalTheme, LabeledSpan, MietteError, MietteSpanContents, NamedSource, Severity,
    SourceCode, SourceSpan, SpanContents,
};
use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::positions::locate_one;

create_exception!(
    rkdl,
    ParseError,
    PyValueError,
    concat!(
        "The text isn't valid KDL.\n",
        "\n",
        "`str(error)` is a report that points at each problem in the text. The attributes give the first problem's\n",
        "position, counted as `node.line` and `node.column` count: lines break at every KDL newline, and columns count\n",
        "characters, not bytes, after any byte order mark.\n",
        "\n",
        "Attributes:\n",
        "    line: The first problem's line, starting at 1.\n",
        "    column: The first problem's column, starting at 1.\n",
        "    offset: The first problem's offset from the start of the text, starting at 0.\n",
        "    diagnostics: Every problem found, in order.\n",
        "    path: The file `load` read the text from, or None if it didn't come from a named file.",
    )
);

create_exception!(
    rkdl,
    ReadError,
    PyValueError,
    concat!(
        "A value in the document doesn't fit its type annotation, such as `(u8)300`.\n",
        "\n",
        "`str(error)` is one line for whoever wrote the file, starting as `ParseError`'s first line does when the entry\n",
        "has a position: `config.kdl:2:9: server prop retries: 300 doesn't fit (u8): out of range 0..=255`. A custom\n",
        "Type's decode exception is the `__cause__`. Writing a value that doesn't fit raises a plain `ValueError`\n",
        "instead: that's about the program's value, not the file.\n",
        "\n",
        "Attributes:\n",
        "    path: The file the document was loaded from, or None if it didn't come from a named file.\n",
        "    line: The entry's line, as `entry.line` counts it, or None if it has no position.\n",
        "    column: The entry's column, as `entry.column` counts it, or None if it has no position.\n",
        "    key: The entry's index in `node.args` (an int) or its property key (a str).",
    )
);

// It can be built and pickled, so a `ParseError` holding some can cross a process boundary.
/// One problem found while parsing.
///
/// Positions count characters, not bytes, and lines and columns count as `ParseError`'s do. Diagnostics compare
/// equal when all their fields do, and can be pickled.
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
    /// Lines and columns count as `node.line` and `node.column` do.
    fn from_kdl(input: &str, diag: &KdlDiagnostic) -> Self {
        // kdl-rs spans are byte offsets into the UTF-8 input. Rounding them down to a character boundary means a span
        // that lands inside a character gives a slightly-off position instead of a panic.
        let start = input.floor_char_boundary(diag.span.offset());
        let end = input.floor_char_boundary(start.saturating_add(diag.span.len()));
        let at = locate_one(input, start);
        Diagnostic {
            message: diag.message.clone().unwrap_or_else(|| "unexpected error".into()),
            label: diag.label.clone(),
            help: diag.help.clone(),
            offset: input.get(..start).unwrap_or_default().chars().count(),
            length: input.get(start..end).unwrap_or_default().chars().count(),
            line: at.line as usize,
            column: at.column as usize,
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
    let report = report(err, path, first.map(|f| (f.0, f.1)));
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

/// The exception's text: a summary line naming the file and the first problem's position, then each problem as its own
/// miette report.
///
/// Rendering kdl-rs's `KdlError` whole would show its diagnostics as miette's "related" reports, each behind a bare
/// `Error: ` line. Rendering them one by one gives every problem the same shape, with only a blank line between them.
/// The summary has no `×`, so a traceback's last line reads `rkdl.ParseError: config.kdl:4:9: ...`.
fn report(err: &KdlError, path: Option<&str>, first: Option<(usize, usize)>) -> String {
    let at = match (path, first) {
        (Some(path), Some((line, column))) => format!("{path}:{line}:{column}: "),
        (Some(path), None) => format!("{path}: "),
        (None, Some((line, column))) => format!("{line}:{column}: "),
        (None, None) => String::new(),
    };
    let source = Source::new(err, path);
    let mut parts = vec![format!("{at}{err}")];
    parts.extend(err.diagnostics.iter().map(|diagnostic| {
        render(&Located {
            diagnostic,
            source: &source,
        })
    }));
    parts.join("\n\n")
}

/// Exception text ends up in log files, so render without colour or terminal hyperlinks.
fn render(diagnostic: &dyn miette::Diagnostic) -> String {
    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor()).with_links(false);
    let mut out = String::new();
    match handler.render_report(&mut out, diagnostic) {
        Ok(()) => out.trim_end().to_owned(),
        Err(_) => diagnostic.to_string(),
    }
}

/// The parsed text, named after its file if it came from one, so each snippet's header is `[config.kdl:4:9]` as miette
/// prints named sources.
///
/// miette's own text sources count lines at `\r` and `\n` only, and columns in bytes, a byte order mark included. Snippet
/// headers count lines and columns as `ParseError.line` and `node.line` do instead, so they agree with the summary.
struct Source {
    text: Arc<String>,
    name: Option<NamedSource<Arc<String>>>,
}

impl Source {
    fn new(err: &KdlError, path: Option<&str>) -> Self {
        Source {
            text: Arc::clone(&err.input),
            name: path.map(|path| NamedSource::new(path, Arc::clone(&err.input))),
        }
    }
}

impl SourceCode for Source {
    fn read_span<'a>(
        &'a self,
        span: &SourceSpan,
        context_lines_before: usize,
        context_lines_after: usize,
    ) -> Result<Box<dyn SpanContents<'a> + 'a>, MietteError> {
        let inner = match &self.name {
            Some(named) => named.read_span(span, context_lines_before, context_lines_after)?,
            None => self.text.read_span(span, context_lines_before, context_lines_after)?,
        };
        let at = locate_one(&self.text, inner.span().offset());
        let (line, column) = (at.line as usize - 1, at.column as usize - 1);
        let (data, span, lines) = (inner.data(), *inner.span(), inner.line_count());
        Ok(Box::new(match inner.name() {
            Some(name) => MietteSpanContents::new_named(name.to_owned(), data, span, line, column, lines),
            None => MietteSpanContents::new(data, span, line, column, lines),
        }))
    }
}

/// One kdl-rs diagnostic with the text it points into. kdl-rs's diagnostics don't carry the text: the `KdlError` holding
/// them does.
struct Located<'a> {
    diagnostic: &'a KdlDiagnostic,
    source: &'a Source,
}

impl fmt::Debug for Located<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.diagnostic, f)
    }
}

impl fmt::Display for Located<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.diagnostic, f)
    }
}

impl std::error::Error for Located<'_> {}

// Everything but the source is kdl-rs's.
impl miette::Diagnostic for Located<'_> {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(self.source)
    }

    fn severity(&self) -> Option<Severity> {
        miette::Diagnostic::severity(self.diagnostic)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        miette::Diagnostic::help(self.diagnostic)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        miette::Diagnostic::labels(self.diagnostic)
    }
}
