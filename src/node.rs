use std::sync::{Arc, Mutex};

use kdl::{KdlDocument, KdlEntry, KdlIdentifier, KdlNode};
use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use crate::children::{self, Children, Owner};
use crate::document::Document;
use crate::parser::builtin_types;
use crate::tree::{IdTree, NodeId, node_at_mut};
use crate::values::{self, TypeRegistry};

#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Node {
    // Ids are global, so moving the node to another document only swaps `doc`.
    pub(crate) id: NodeId,
    // Locked only to read or swap the pointer, never while a document is locked.
    doc: Mutex<Py<Document>>,
}

pub(crate) fn stale() -> PyErr {
    PyRuntimeError::new_err("node handle is out of step with its document (a bug in rkdl)")
}

impl Node {
    pub(crate) fn new(id: NodeId, doc: Py<Document>) -> Self {
        Node {
            id,
            doc: Mutex::new(doc),
        }
    }

    pub(crate) fn doc(&self, py: Python<'_>) -> Py<Document> {
        self.doc.lock().expect("node pointer poisoned").clone_ref(py)
    }

    pub(crate) fn set_doc(&self, doc: Py<Document>) {
        let old = std::mem::replace(&mut *self.doc.lock().expect("node pointer poisoned"), doc);
        // Dropping the old document can run Python code, so do it after unlocking.
        drop(old);
    }

    /// Runs `f` on this node's KdlNode under its document's lock.
    pub(crate) fn with_node<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut KdlNode) -> R + Send) -> PyResult<R> {
        let id = self.id;
        self.doc(py).get().with_state(py, |s| {
            let path = s.ids.path(id).ok_or_else(stale)?;
            Ok(f(node_at_mut(&mut s.kdl, &path)))
        })
    }
}

/// Wraps one KdlNode in a private document of its own, with fresh ids.
pub(crate) fn detached<'py>(py: Python<'py>, node: KdlNode, types: Arc<TypeRegistry>) -> PyResult<Bound<'py, Node>> {
    let mut kdl = KdlDocument::new();
    kdl.nodes_mut().push(node);
    let ids = IdTree::from_document(&kdl);
    let id = ids.top()[0];
    let home = Bound::new(py, Document::with_ids(ids, kdl, types, true))?;
    Document::node_handle(&home, id)
}

fn entry(value: &Bound<'_, PyAny>, key: Option<String>, types: &TypeRegistry) -> PyResult<KdlEntry> {
    let (value, ty) = values::from_python(value, None, types)?;
    let mut entry = match key {
        Some(key) => KdlEntry::new_prop(key, value),
        None => KdlEntry::new(value),
    };
    if let Some(ty) = ty {
        entry.set_ty(ty);
    }
    Ok(entry)
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
        let types = builtin_types();
        let mut node = KdlNode::new(name);
        if let Some(ty) = r#type {
            node.set_ty(ty);
        }
        for value in args.iter() {
            node.entries_mut().push(entry(&value, None, &types)?);
        }
        for dict in [props, kwargs].into_iter().flatten() {
            for (key, value) in dict.iter() {
                let key: String = key
                    .extract()
                    .map_err(|_| PyTypeError::new_err("property keys must be str"))?;
                node.entries_mut().push(entry(&value, Some(key), &types)?);
            }
        }
        Ok(detached(py, node, types)?.unbind())
    }

    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        self.with_node(py, |n| n.name().value().to_owned())
    }

    // set_name replaces the whole identifier, so the old spelling doesn't keep printing.
    #[setter]
    fn set_name(&self, py: Python<'_>, name: String) -> PyResult<()> {
        self.with_node(py, |n| n.set_name(name))
    }

    #[getter]
    fn r#type(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_node(py, |n| n.ty().map(|t| t.value().to_owned()))
    }

    #[setter]
    fn set_type(&self, py: Python<'_>, ty: Option<String>) -> PyResult<()> {
        self.with_node(py, |n| *n.ty_mut() = ty.map(KdlIdentifier::from))
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
            Some(v) if !v.is_none() => {
                let empty = v.cast::<PyList>().is_ok_and(|l| l.is_empty());
                if !empty {
                    return Err(PyTypeError::new_err(
                        "node.children can only be set to [] or None; edit it with insert, append, pop, remove or del",
                    ));
                }
                children::open_block(&owner, slf.py())
            }
            _ => children::close_block(&owner, slf.py()),
        }
    }

    fn copy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, Node>> {
        let doc = self.doc(py);
        let node = self.with_node(py, |n| n.clone())?;
        detached(py, node, doc.get().types.clone())
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

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let name = self.with_node(py, |n| KdlIdentifier::from(n.name().value()).to_string())?;
        Ok(format!("<Node {name}>"))
    }

    fn __eq__(&self, other: PyRef<'_, Node>) -> bool {
        self.id == other.id
    }

    fn __hash__(&self) -> u64 {
        self.id.key()
    }
}
