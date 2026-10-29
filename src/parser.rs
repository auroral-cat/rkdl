use std::sync::Arc;

use kdl::KdlDocument;
use pyo3::prelude::*;

use crate::document::Document;
use crate::errors::parse_error;
use crate::format::Formatter;
use crate::values::{Type, TypeRegistry};

#[pyclass(module = "rkdl", frozen)]
pub struct Parser {
    types: Arc<TypeRegistry>,
    formatter: Option<Py<Formatter>>,
}

#[pymethods]
impl Parser {
    #[new]
    #[pyo3(signature = (types=None, formatter=None))]
    fn new(types: Option<Vec<Py<Type>>>, formatter: Option<Py<Formatter>>) -> Self {
        // TODO(03): build the registry with TypeRegistry::with_user_types.
        let _ = types;
        Parser {
            types: Arc::new(TypeRegistry::builtin()),
            formatter,
        }
    }

    fn parse(&self, py: Python<'_>, text: &str) -> PyResult<Document> {
        let mut kdl = text.parse::<KdlDocument>().map_err(|e| parse_error(py, &e))?;
        // Formatting before the Document exists is the same as doc.fmt() but needs no lock.
        if let Some(formatter) = &self.formatter {
            formatter.get().apply(&mut kdl);
        }
        Ok(Document::new(kdl, self.types.clone()))
    }
}

impl Default for Parser {
    fn default() -> Self {
        Parser::new(None, None)
    }
}

#[pyfunction]
pub fn parse(py: Python<'_>, text: &str) -> PyResult<Document> {
    Parser::default().parse(py, text)
}
