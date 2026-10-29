use std::sync::{Arc, OnceLock};

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
    fn new(py: Python<'_>, types: Option<Vec<Py<Type>>>, formatter: Option<Py<Formatter>>) -> PyResult<Self> {
        let types = match types {
            Some(types) => Arc::new(TypeRegistry::with_user_types(py, types)?),
            None => builtin_types(),
        };
        Ok(Parser { types, formatter })
    }

    fn parse(&self, py: Python<'_>, text: &str) -> PyResult<Document> {
        let kdl = text.parse::<KdlDocument>().map_err(|e| parse_error(py, &e))?;
        // TODO(05): run doc.fmt(formatter) when self.formatter is set.
        let _ = &self.formatter;
        Ok(Document::new(kdl, self.types.clone()))
    }
}

// Documents parsed without user types all share one registry.
fn builtin_types() -> Arc<TypeRegistry> {
    static BUILTIN: OnceLock<Arc<TypeRegistry>> = OnceLock::new();
    BUILTIN.get_or_init(|| Arc::new(TypeRegistry::builtin())).clone()
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
