use std::sync::{Arc, LazyLock};

use kdl::KdlDocument;
use pyo3::PyTraverseError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;

use crate::document::Document;
use crate::errors::parse_error;
use crate::format::Formatter;
use crate::values::{Type, TypeRegistry};

#[pyclass(module = "rkdl", frozen)]
pub struct Parser {
    types: Arc<TypeRegistry>,
    // A Formatter holds no Python references, so it can't be part of a cycle.
    formatter: Option<Py<Formatter>>,
}

#[pymethods]
impl Parser {
    #[new]
    #[pyo3(signature = (types=None, formatter=None))]
    fn new(py: Python<'_>, types: Option<Vec<Py<Type>>>, formatter: Option<Py<Formatter>>) -> Self {
        let types = match types {
            Some(types) => Arc::new(TypeRegistry::with_user_types(py, types)),
            None => builtin_types(),
        };
        Parser { types, formatter }
    }

    fn parse(&self, py: Python<'_>, text: &str) -> PyResult<Document> {
        let mut kdl = text.parse::<KdlDocument>().map_err(|e| parse_error(py, &e))?;
        // Formatting before the Document exists is the same as doc.fmt() but needs no lock.
        if let Some(formatter) = &self.formatter {
            formatter.get().apply(&mut kdl);
        }
        Ok(Document::new(kdl, TypeRegistry::private_copy(&self.types, py)))
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        TypeRegistry::traverse(&self.types, &visit)
    }
}

/// The registry for documents parsed without user types. It holds no Python references, so they can all share it.
pub(crate) fn builtin_types() -> Arc<TypeRegistry> {
    static BUILTIN: LazyLock<Arc<TypeRegistry>> = LazyLock::new(|| Arc::new(TypeRegistry::builtin()));
    Arc::clone(&BUILTIN)
}

impl Default for Parser {
    fn default() -> Self {
        Parser {
            types: builtin_types(),
            formatter: None,
        }
    }
}

#[pyfunction]
pub fn parse(py: Python<'_>, text: &str) -> PyResult<Document> {
    Parser::default().parse(py, text)
}
