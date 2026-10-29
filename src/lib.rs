mod children;
mod document;
mod errors;
mod format;
mod node;
mod parser;
mod tree;
mod values;

#[pyo3::pymodule]
mod _rkdl {
    #[pymodule_export]
    use crate::children::Children;
    #[pymodule_export]
    use crate::document::Document;
    #[pymodule_export]
    use crate::errors::{Diagnostic, ParseError};
    #[pymodule_export]
    use crate::format::Formatter;
    #[pymodule_export]
    use crate::node::Node;
    #[pymodule_export]
    use crate::parser::{Parser, parse};
    #[pymodule_export]
    use crate::values::{_debug_from_python, _debug_to_python, Type};

    #[pymodule_init]
    fn init(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
        use pyo3::types::PyModuleMethods;
        m.add("__version__", env!("CARGO_PKG_VERSION"))
    }
}
