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
        let mut kdl = text.parse::<KdlDocument>().map_err(|e| parse_error(py, &e))?;
        // Formatting before the Document exists is the same as doc.fmt() but needs no lock.
        if let Some(formatter) = &self.formatter {
            formatter.get().apply(&mut kdl);
        }
        Ok(Document::new(kdl, self.types.clone_ref(py)))
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.types)
    }
}

#[pyfunction]
pub fn parse(py: Python<'_>, text: &str) -> PyResult<Document> {
    let parser = Parser {
        types: TypeRegistry::shared_builtin(py)?,
        formatter: None,
    };
    parser.parse(py, text)
}
