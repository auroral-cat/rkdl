pub(crate) mod prescan;

use std::sync::Arc;

use kdl::{KdlDiagnostic, KdlDocument, KdlError};
use miette::Severity;
use pyo3::PyTraverseError;
use pyo3::exceptions::PyRuntimeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use crate::document::Document;
use crate::errors::parse_error;
use crate::format::Formatter;
use crate::node::{self, Node, Spec};
use crate::values::{Type, TypeRegistry};

#[pyclass(module = "rkdl", frozen)]
pub struct Parser {
    types: Py<TypeRegistry>,
    // A Formatter holds no Python references, so it can't be part of a cycle.
    formatter: Option<Py<Formatter>>,
}

#[pymethods]
impl Parser {
    #[new]
    #[pyo3(signature = (types=None, formatter=None))]
    fn new(py: Python<'_>, types: Option<Vec<Py<Type>>>, formatter: Option<Py<Formatter>>) -> PyResult<Self> {
        let types = match types {
            Some(types) => Py::new(py, TypeRegistry::with_user_types(py, types))?,
            None => TypeRegistry::shared_builtin(py)?,
        };
        Ok(Parser { types, formatter })
    }

    fn parse(&self, py: Python<'_>, text: &str) -> PyResult<Document> {
        let formatter = self.formatter.as_ref().map(Py::get);
        let kdl = py
            .detach(|| on_big_stack(|| parse_text(text, formatter)))?
            .map_err(|e| parse_error(py, &e))?;
        Ok(Document::new(kdl, self.types.clone_ref(py)))
    }

    /// A new detached node that reads and writes through this parser's types, so its values can be of custom types.
    /// Takes the same arguments as `Node(...)`.
    #[pyo3(signature = (name, *args, r#type=None, props=None, children=None, **kwargs))]
    fn node<'py>(
        &self,
        name: String,
        args: &Bound<'py, PyTuple>,
        r#type: Option<&Bound<'py, PyAny>>,
        props: Option<&Bound<'py, PyDict>>,
        children: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, Node>> {
        let spec = Spec {
            ty: r#type,
            props,
            kwargs,
            children,
        };
        node::build(name, args, &spec, self.types.clone_ref(args.py()), "\n")
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.types)
    }
}

fn parse_text(text: &str, formatter: Option<&Formatter>) -> Result<KdlDocument, KdlError> {
    if let Some(refusal) = prescan::scan(text).refusal {
        return Err(refused(text, refusal));
    }
    let mut kdl = text.parse::<KdlDocument>()?;
    // Formatting before the Document exists is the same as doc.fmt() but needs no lock.
    if let Some(formatter) = formatter {
        formatter.apply(&mut kdl);
    }
    Ok(kdl)
}

/// The prescan's refusal as a kdl-rs error, so it reaches Python as a `ParseError` like any other.
fn refused(text: &str, refusal: prescan::Refusal) -> KdlError {
    let input = Arc::new(text.to_owned());
    KdlError {
        input: Arc::clone(&input),
        diagnostics: vec![KdlDiagnostic {
            input,
            span: (refusal.offset, refusal.len).into(),
            message: Some(refusal.message),
            label: Some(refusal.label),
            help: Some(refusal.help),
            severity: Severity::Error,
        }],
    }
}

/// Reserved address space, committed only as it's used. kdl-rs's parser needs about 32 KB per block level on a debug
/// build and 6 KB on a release build; the prescan's limits keep it under 140 MB and 30 MB.
const STACK_SIZE: usize = 256 << 20;

/// Runs `f` on a new thread with a big stack, since kdl-rs's parser recurses per nesting level.
fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> PyResult<T> {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .name("rkdl-parse".into())
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, f)
            .map_err(|e| PyRuntimeError::new_err(format!("couldn't start the parser thread: {e}")))?;
        Ok(thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    })
}

#[pyfunction]
pub fn parse(py: Python<'_>, text: &str) -> PyResult<Document> {
    let parser = Parser {
        types: TypeRegistry::shared_builtin(py)?,
        formatter: None,
    };
    parser.parse(py, text)
}
