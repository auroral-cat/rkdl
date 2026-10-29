use std::sync::Mutex;

use kdl::{KdlDiagnostic, KdlDocument, KdlError};
use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

create_exception!(rkdl, ParseError, PyValueError);

/// One problem found while parsing. Offsets count code points, not bytes.
#[pyclass(module = "rkdl", frozen, get_all)]
struct Diagnostic {
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
        // kdl-rs spans are byte offsets into the UTF-8 input.
        let start = diag.span.offset().min(input.len());
        let end = (start + diag.span.len()).min(input.len());
        let before = &input[..start];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        Diagnostic {
            message: diag.message.clone().unwrap_or_else(|| "unexpected error".into()),
            label: diag.label.clone(),
            help: diag.help.clone(),
            offset: before.chars().count(),
            length: input[start..end].chars().count(),
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count() + 1,
        }
    }
}

fn parse_error(py: Python<'_>, err: &KdlError) -> PyErr {
    let diagnostics: Vec<Diagnostic> = err
        .diagnostics
        .iter()
        .map(|d| Diagnostic::from_kdl(&err.input, d))
        .collect();
    // kdl-rs's own Display is always "Failed to parse KDL document", so lead with the first diagnostic.
    let message = match diagnostics.first() {
        Some(d) => format!("{}:{}: {}", d.line, d.column, d.message),
        None => err.to_string(),
    };
    let exc = ParseError::new_err(message);
    if let Err(e) = exc.value(py).setattr("diagnostics", diagnostics) {
        return e;
    }
    exc
}

#[pyclass(module = "rkdl", frozen)]
struct Document {
    inner: Mutex<KdlDocument>,
}

#[pymethods]
impl Document {
    fn __str__(&self) -> String {
        self.inner.lock().unwrap().to_string()
    }

    fn __len__(&self) -> usize {
        self.inner.lock().unwrap().nodes().len()
    }

    fn __repr__(&self) -> String {
        format!("<Document with {} nodes>", self.__len__())
    }
}

#[pyfunction]
fn parse(py: Python<'_>, text: &str) -> PyResult<Document> {
    let doc = text.parse::<KdlDocument>().map_err(|e| parse_error(py, &e))?;
    Ok(Document { inner: Mutex::new(doc) })
}

#[pymodule]
mod rkdl {
    #[pymodule_export]
    use super::{parse, Diagnostic, Document, ParseError};

    #[pymodule_init]
    fn init(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
        use pyo3::types::PyModuleMethods;
        m.add("__version__", env!("CARGO_PKG_VERSION"))
    }
}
