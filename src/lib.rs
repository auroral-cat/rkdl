mod args;
mod children;
mod document;
mod edit;
mod entry;
mod errors;
mod format;
mod handles;
mod index;
mod layout;
mod node;
mod parser;
mod props;
mod spelling;
mod tree;
mod values;

#[pyo3::pymodule]
mod _rkdl {
    #[pymodule_export]
    use crate::args::Args;
    #[pymodule_export]
    use crate::children::Children;
    #[pymodule_export]
    use crate::document::Document;
    #[pymodule_export]
    use crate::entry::{Entries, Entry};
    #[pymodule_export]
    use crate::errors::{Diagnostic, ParseError};
    #[pymodule_export]
    use crate::format::Formatter;
    #[pymodule_export]
    use crate::node::Node;
    #[pymodule_export]
    use crate::parser::{Parser, parse};
    #[pymodule_export]
    use crate::props::Props;
    #[pymodule_export]
    use crate::values::{_debug_assertions, _debug_from_python, _debug_reserved_types, _debug_to_python, Type};

    #[pymodule_init]
    fn init(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
        use pyo3::types::PyModuleMethods;
        m.add("__version__", env!("CARGO_PKG_VERSION"))
    }
}
