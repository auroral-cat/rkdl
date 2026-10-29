use std::sync::{Arc, Mutex, MutexGuard};

use kdl::KdlDocument;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyWeakrefReference;

use crate::children::{self, Children, Owner};
use crate::format::Formatter;
use crate::handles::{self, Garbage, Home};
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

    /// Fills a private home made by `empty_home`. Only for a home no other thread can reach yet, which is why
    /// taking its lock while another document is locked can't deadlock.
    pub(crate) fn fill(&self, kdl: KdlDocument, ids: IdTree) {
        let mut state = self.lock();
        *state = DocState { kdl, ids };
        state.check();
    }

    pub(crate) fn empty_home(types: Arc<TypeRegistry>) -> Self {
        let kdl = KdlDocument::new();
        Document::with_ids(IdTree::from_document(&kdl), kdl, types, true)
    }

    /// The one handle for `id`, so `doc["a"] is doc["a"]`. Every way of getting a Node goes through here. None means
    /// the node isn't in `doc` any more: the caller's view of the document is out of date and it should look again.
    pub(crate) fn node_handle<'py>(doc: &Bound<'py, Document>, id: NodeId) -> PyResult<Option<Bound<'py, Node>>> {
        let py = doc.py();
        loop {
            if let Some(node) = handles::get::<Node>(py, id.key()) {
                return Ok(Some(node));
            }
            // Python objects are made before locking; the lock only decides whether to publish this one.
            let home = Home::new(Arc::new(doc.clone().unbind()));
            let node = Bound::new(py, Node::new(id, home.clone()))?;
            let weak = PyWeakrefReference::new(&node)?.unbind();
            let (found, published, garbage) = doc.get().with_state(py, |s| {
                let mut garbage = Garbage::default();
                if !s.ids.contains(id) {
                    garbage.weakrefs.push(weak);
                    return (false, false, garbage);
                }
                let published = handles::intern_locked(id.key(), weak, &home, &mut garbage);
                (true, published, garbage)
            });
            drop(garbage);
            match (found, published) {
                (false, _) => return Ok(None),
                (true, true) => return Ok(Some(node)),
                // Another thread published a handle first, or one is being freed right now.
                (true, false) => py.detach(std::thread::yield_now),
            }
        }
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
