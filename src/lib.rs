mod annotated;
mod args;
mod children;
mod document;
mod edit;
mod entry;
mod errors;
mod format;
mod handles;
mod index;
mod iso;
mod layout;
mod node;
mod parser;
mod positions;
mod props;
mod spelling;
mod tree;
mod v1;
mod values;

/// Helpers for cargo tests that build rkdl's objects directly, for states no Python input reaches.
#[cfg(test)]
mod testing {
    use pyo3::prelude::*;

    use crate::document::Document;
    use crate::tree::{IdTree, NodeId};
    use crate::values::TypeRegistry;

    /// Runs `f` with the interpreter, which a cargo test has to start itself.
    #[expect(
        clippy::disallowed_methods,
        reason = "the lint keeps code that may hold a document lock from attaching to Python, which can deadlock; a \
                  test attaches once at its top, holding no lock, to get the token a Python caller would pass in"
    )]
    pub(crate) fn with_python<R>(f: impl FnOnce(Python<'_>) -> R) -> R {
        Python::initialize();
        Python::attach(f)
    }

    /// A private one-node document, empty, as `node.detach()` makes before filling it.
    pub(crate) fn empty_doc(py: Python<'_>) -> Py<Document> {
        let types = TypeRegistry::shared_builtin(py).expect("the built-in types");
        Py::new(py, Document::empty_home(types, None)).expect("allocates")
    }

    /// The id of a node in no document: one from a tree that was built and dropped. A handle with this id and a
    /// home in some document is out of step with it, the state `stale()` reports.
    pub(crate) fn stray_id() -> NodeId {
        let mut kdl = kdl::KdlDocument::new();
        kdl.nodes_mut().push(kdl::KdlNode::new("x"));
        *IdTree::from_document(&kdl).top().first().expect("one node")
    }
}

#[pyo3::pymodule]
mod _rkdl {
    #[pymodule_export]
    use crate::annotated::Value;
    #[pymodule_export]
    use crate::args::Args;
    #[pymodule_export]
    use crate::children::Children;
    #[pymodule_export]
    use crate::document::Document;
    #[pymodule_export]
    use crate::entry::{Entries, Entry};
    #[pymodule_export]
    use crate::errors::{Diagnostic, ParseError, ReadError};
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
