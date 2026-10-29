use std::sync::{Arc, Mutex, MutexGuard};

use kdl::KdlDocument;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;

use crate::children::{self, Children, Owner};
use crate::format::Formatter;
use crate::node::Node;
use crate::tree::{IdTree, NodeId};
use crate::values::TypeRegistry;

pub(crate) struct DocState {
    pub(crate) kdl: KdlDocument,
    pub(crate) ids: IdTree,
}

impl DocState {
    fn check(&self) {
        if cfg!(debug_assertions) {
            self.ids.check_shape(&self.kdl);
        }
    }
}

#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Document {
    state: Mutex<DocState>,
    pub(crate) types: Arc<TypeRegistry>,
    /// A detached node's private one-node document. Never handed to Python as a Document.
    pub(crate) home: bool,
    // A weakref.WeakValueDictionary from id key to Node. Python's own dictionary means no second Rust lock to order
    // against the GIL.
    cache: PyOnceLock<Py<PyAny>>,
}

impl Document {
    pub(crate) fn new(kdl: KdlDocument, types: Arc<TypeRegistry>) -> Self {
        Document::with_ids(IdTree::from_document(&kdl), kdl, types, false)
    }

    pub(crate) fn with_ids(ids: IdTree, kdl: KdlDocument, types: Arc<TypeRegistry>, home: bool) -> Self {
        Document {
            state: Mutex::new(DocState { kdl, ids }),
            types,
            home,
            cache: PyOnceLock::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, DocState> {
        // A panic while locked means the two trees may disagree, so a poisoned document stays unusable.
        self.state.lock().expect("document state poisoned by an earlier panic")
    }

    /// The only way to touch DocState. The closure can't hold a Python token, so it can't call into Python while
    /// the lock is held.
    pub(crate) fn with_state<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut DocState) -> R + Send) -> R {
        py.detach(|| {
            let mut state = self.lock();
            let result = f(&mut state);
            state.check();
            result
        })
    }

    fn cache<'py>(&self, py: Python<'py>) -> PyResult<&Bound<'py, PyAny>> {
        self.cache
            .get_or_try_init(py, || {
                py.import("weakref")?
                    .getattr("WeakValueDictionary")?
                    .call0()
                    .map(Bound::unbind)
            })
            .map(|cache| cache.bind(py))
    }

    /// The one handle for `id`, so `doc["a"] is doc["a"]`. Every way of getting a Node goes through here.
    pub(crate) fn node_handle<'py>(slf: &Bound<'py, Document>, id: NodeId) -> PyResult<Bound<'py, Node>> {
        let py = slf.py();
        let cache = slf.get().cache(py)?;
        let found = cache.call_method1("get", (id.key(),))?;
        if !found.is_none() {
            return Ok(found.cast_into::<Node>()?);
        }
        let node = Bound::new(py, Node::new(id, slf.clone().unbind()))?;
        cache.set_item(id.key(), &node)?;
        Ok(node)
    }

    /// After a subtree moved from `from` to `to`, points the live handles in it at their new document.
    pub(crate) fn move_handles(
        py: Python<'_>,
        from: &Bound<'_, Document>,
        to: &Bound<'_, Document>,
        ids: &[NodeId],
    ) -> PyResult<()> {
        let old = from.get().cache(py)?;
        let new = to.get().cache(py)?;
        for id in ids {
            let found = old.call_method1("get", (id.key(),))?;
            if found.is_none() {
                continue;
            }
            let node = found.cast_into::<Node>()?;
            node.get().set_doc(to.clone().unbind());
            new.set_item(id.key(), &node)?;
            old.del_item(id.key())?;
        }
        Ok(())
    }
}

/// Locks two documents for an edit that spans both, such as moving a node. They're locked in address order so two
/// threads moving nodes in opposite directions can't deadlock. If `a` and `b` are the same document it's locked once
/// and `f` gets `None` for `b`.
pub(crate) fn lock_both<R: Send>(
    py: Python<'_>,
    a: &Document,
    b: &Document,
    f: impl FnOnce(&mut DocState, Option<&mut DocState>) -> R + Send,
) -> R {
    py.detach(|| {
        if std::ptr::eq(a, b) {
            let mut state = a.lock();
            let result = f(&mut state, None);
            state.check();
            return result;
        }
        let (mut sa, mut sb) = if (a as *const Document) < (b as *const Document) {
            let sa = a.lock();
            (sa, b.lock())
        } else {
            let sb = b.lock();
            (a.lock(), sb)
        };
        let result = f(&mut sa, Some(&mut sb));
        sa.check();
        sb.check();
        result
    })
}

#[pymethods]
impl Document {
    fn __str__(&self, py: Python<'_>) -> String {
        self.with_state(py, |s| s.kdl.to_string())
    }

    fn __len__(&self, py: Python<'_>) -> usize {
        self.with_state(py, |s| s.kdl.nodes().len())
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        format!("<Document with {} nodes>", self.__len__(py))
    }

    // autoformat only rewrites format strings, never structure, so the IdTree stays valid.
    #[pyo3(signature = (formatter=None))]
    fn fmt(&self, py: Python<'_>, formatter: Option<&Formatter>) {
        self.with_state(py, |s| match formatter {
            Some(f) => f.apply(&mut s.kdl),
            None => Formatter::default().apply(&mut s.kdl),
        });
    }

    #[getter]
    fn children(slf: &Bound<'_, Self>) -> Children {
        Children::new(Owner::Doc(slf.clone().unbind()))
    }

    #[setter]
    fn set_children(&self, _value: Option<Bound<'_, PyAny>>) -> PyResult<()> {
        Err(PyTypeError::new_err(
            "can't assign to Document.children; edit it with insert, append, pop, remove or del",
        ))
    }

    fn __getitem__<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, Node>> {
        children::lookup_one(&Owner::Doc(slf.clone().unbind()), key)
    }

    #[pyo3(signature = (key, default=None))]
    fn get<'py>(
        slf: &Bound<'py, Self>,
        key: &Bound<'py, PyAny>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        children::lookup_get(&Owner::Doc(slf.clone().unbind()), key, default)
    }

    fn get_all<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, Node>>> {
        children::lookup(&Owner::Doc(slf.clone().unbind()), key, false)
    }
}
