use std::sync::Arc;

use kdl::{KdlDocument, KdlEntry, KdlNode};
use pyo3::PyTraverseError;
use pyo3::exceptions::PyTypeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use crate::args::Args;
use crate::children::{self, Children, Owner};
use crate::document::Document;
use crate::entry::{Entries, new_entry};
use crate::handles::{self, Fail, Home, stale};
use crate::props::Props;
use crate::tree::{IdTree, NodeId, node_at, node_at_mut};
use crate::values::{self, TypeRegistry};

#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Node {
    // Ids are global, so moving the node to another document only repoints `home`.
    pub(crate) id: NodeId,
    pub(crate) home: Arc<Home>,
}

impl Node {
    pub(crate) fn new(id: NodeId, home: Arc<Home>) -> Self {
        Node { id, home }
    }

    /// Runs `f` on this node's `KdlNode` under its document's lock.
    pub(crate) fn with_node<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut KdlNode) -> R + Send) -> PyResult<R> {
        let id = self.id;
        let mut f = Some(f);
        handles::on_home(py, &self.home, |doc| {
            doc.get().with_state(py, |s| {
                let path = s.ids.path(id).ok_or_else(|| Fail::Missing(stale()))?;
                let f = f.take().expect("runs once it finds the node");
                Ok(f(node_at_mut(&mut s.kdl, &path)))
            })
        })
    }
}

/// Wraps one `KdlNode` in a private document of its own, with fresh ids, that reads and writes through `types` and
/// writes line endings in the style `eol`.
pub(crate) fn detached<'py>(
    py: Python<'py>,
    node: KdlNode,
    types: Py<TypeRegistry>,
    eol: &'static str,
) -> PyResult<Bound<'py, Node>> {
    let mut kdl = KdlDocument::new();
    kdl.nodes_mut().push(node);
    let ids = IdTree::from_document(&kdl);
    let &[id] = ids.top() else {
        return Err(stale());
    };
    let home = Bound::new(py, Document::empty_home(types))?;
    home.get().fill(kdl, ids, eol);
    // Nothing else can reach a new home, so the node is still there.
    Document::node_handle(&home, id)?.ok_or_else(stale)
}

fn entry(value: &Bound<'_, PyAny>, key: Option<String>, types: &TypeRegistry) -> PyResult<KdlEntry> {
    let (value, ty) = values::from_python(value, None, types)?;
    Ok(new_entry(key, value, ty))
}

#[pymethods]
impl Node {
    #[new]
    #[pyo3(signature = (name, *args, r#type=None, props=None, **kwargs))]
    fn py_new(
        py: Python<'_>,
        name: String,
        args: &Bound<'_, PyTuple>,
        r#type: Option<String>,
        props: Option<&Bound<'_, PyDict>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<Node>> {
        let types = TypeRegistry::shared_builtin(py)?;
        let mut node = KdlNode::new(values::identifier(name));
        if let Some(ty) = r#type {
            node.set_ty(values::identifier(ty));
        }
        for value in args.iter() {
            node.entries_mut().push(entry(&value, None, types.get())?);
        }
        for dict in [props, kwargs].into_iter().flatten() {
            for (key, value) in dict.iter() {
                #[expect(
                    clippy::map_err_ignore,
                    reason = "this message says which argument was wrong; PyO3's doesn't"
                )]
                let key: String = key
                    .extract()
                    .map_err(|_| PyTypeError::new_err("property keys must be str"))?;
                node.entries_mut().push(entry(&value, Some(key), types.get())?);
            }
        }
        Ok(detached(py, node, types, "\n")?.unbind())
    }

    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        self.with_node(py, |n| n.name().value().to_owned())
    }

    // set_name replaces the whole identifier, so the old spelling doesn't keep printing.
    #[setter]
    fn set_name(&self, py: Python<'_>, name: String) -> PyResult<()> {
        self.with_node(py, |n| n.set_name(values::identifier(name)))
    }

    #[getter]
    fn r#type(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_node(py, |n| n.ty().map(|t| t.value().to_owned()))
    }

    #[setter]
    fn set_type(&self, py: Python<'_>, ty: Option<String>) -> PyResult<()> {
        self.with_node(py, |n| *n.ty_mut() = ty.map(values::identifier))
    }

    #[getter]
    fn args(slf: &Bound<'_, Self>) -> Args {
        Args::new(slf.clone().unbind())
    }

    #[getter]
    fn props(slf: &Bound<'_, Self>) -> Props {
        Props::new(slf.clone().unbind())
    }

    #[getter]
    fn entries(slf: &Bound<'_, Self>) -> Entries {
        Entries::new(slf.clone().unbind())
    }

    #[getter]
    fn children(slf: &Bound<'_, Self>) -> PyResult<Option<Children>> {
        let has_block = slf.get().with_node(slf.py(), |n| n.children().is_some())?;
        Ok(has_block.then(|| Children::new(Owner::Node(slf.clone().unbind()))))
    }

    #[setter]
    fn set_children(slf: &Bound<'_, Self>, value: Option<Bound<'_, PyAny>>) -> PyResult<()> {
        let owner = Owner::Node(slf.clone().unbind());
        match value {
            // `node.children += nodes` extends the view, then assigns it back.
            Some(v) if children::is_view_of(&v, slf.as_any()) => Ok(()),
            Some(v) if !v.is_none() => {
                let empty = v.cast::<PyList>().is_ok_and(PyListMethods::is_empty);
                if !empty {
                    return Err(PyTypeError::new_err(
                        "node.children can only be set to [] or None; edit it with insert, append, pop, remove or del",
                    ));
                }
                children::open_block(slf.py(), &owner)
            }
            _ => children::close_block(slf.py(), &owner),
        }
    }

    fn copy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, Node>> {
        let ((node, eol), types) = handles::on_home(py, &self.home, |doc| {
            let node = doc.get().with_state(py, |s| {
                let path = s.ids.path(self.id).ok_or_else(|| Fail::Missing(stale()))?;
                Ok::<_, Fail>((node_at(&s.kdl, &path).clone(), s.eol))
            })?;
            Ok((node, doc.get().types.clone_ref(py)))
        })?;
        detached(py, node, types, eol)
    }

    fn __getitem__<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, Node>> {
        children::lookup_one(&Owner::Node(slf.clone().unbind()), key)
    }

    #[pyo3(signature = (key, default=None))]
    fn get<'py>(
        slf: &Bound<'py, Self>,
        key: &Bound<'py, PyAny>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        children::lookup_get(&Owner::Node(slf.clone().unbind()), key, default)
    }

    fn get_all<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, Node>>> {
        children::lookup(&Owner::Node(slf.clone().unbind()), key, false)
    }

    fn __str__(&self, py: Python<'_>) -> PyResult<String> {
        self.with_node(py, |n| {
            let text = n.to_string();
            // The leading text is the indentation and comments above the node, which belong to its surroundings.
            let leading = n.format().map_or("", |f| f.leading.as_str());
            text.strip_prefix(leading).unwrap_or(&text).trim().to_owned()
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let name = self.with_node(py, |n| values::identifier(n.name().value()).to_string())?;
        Ok(format!("<Node {name}>"))
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "PyO3 extracts the other operand as a PyRef"
    )]
    fn __eq__(&self, other: PyRef<'_, Node>) -> bool {
        self.id == other.id
    }

    fn __hash__(&self) -> u64 {
        self.id.key()
    }

    // The home is the node's only strong reference. No `__clear__`: a cycle through a node also passes a Type (behind
    // the document's only strong reference), which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.home.traverse(&visit)
    }
}
