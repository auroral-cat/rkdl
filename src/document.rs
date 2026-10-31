use std::sync::{Arc, Mutex, MutexGuard};

use kdl::KdlDocument;
use pyo3::PyTraverseError;
use pyo3::exceptions::PyTypeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;

use pyo3::types::{PyDict, PyTuple};

use crate::children::{self, At, Children, Owner};
use crate::format::Formatter;
use crate::handles;
use crate::layout;
use crate::node::{self, Node, Spec};
use crate::parser;
use crate::tree::{IdTree, NodeId};
use crate::values::TypeRegistry;

pub(crate) struct DocState {
    pub(crate) kdl: KdlDocument,
    pub(crate) ids: IdTree,
    /// The line ending style for edits where the text around them has none: the first line ending in the text the
    /// document was parsed from, or `\n`. A detached node's home takes it from the document the node left.
    pub(crate) eol: &'static str,
}

impl DocState {
    fn check(&self) {
        if cfg!(debug_assertions) {
            self.ids.check_shape(&self.kdl);
        }
    }
}

impl Drop for DocState {
    /// Takes the tree apart one block at a time. Dropping it whole would recurse once per level, on whichever thread
    /// happens to let go of the document.
    fn drop(&mut self) {
        let mut pending = vec![std::mem::take(&mut self.kdl)];
        while let Some(mut block) = pending.pop() {
            pending.extend(
                block
                    .nodes_mut()
                    .iter_mut()
                    .filter_map(|node| node.children_mut().take()),
            );
        }
    }
}

/// A KDL document: its top-level nodes, plus every bit of formatting and every comment in the text.
///
/// `str(doc)` gives the text back, with your edits. `Document()` is an empty document. To read custom types, parse
/// an empty string with a `Parser` instead.
#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Document {
    state: Mutex<DocState>,
    pub(crate) types: Py<TypeRegistry>,
    /// A detached node's private one-node document. Never handed to Python as a Document.
    pub(crate) home: bool,
    /// The file `load` read the text from. A detached node's home has the path of the document the node left, for
    /// the errors that name where its entries were.
    pub(crate) path: Option<Arc<str>>,
}

impl Document {
    /// A document that wasn't parsed from text, so nothing in it has a position.
    pub(crate) fn new(kdl: KdlDocument, types: Py<TypeRegistry>) -> Self {
        let ids = IdTree::from_document(&kdl);
        Document::parsed(kdl, ids, types, None)
    }

    /// A document parsed from text: `ids` holds the positions, and `path` names the file the text came from.
    pub(crate) fn parsed(kdl: KdlDocument, ids: IdTree, types: Py<TypeRegistry>, path: Option<&str>) -> Self {
        let eol = layout::line_ending(&kdl);
        Document::build(kdl, ids, types, false, eol, path.map(Arc::from))
    }

    /// A private home for a detached node, empty until `fill` puts the node in. It starts empty so the caller can
    /// create its Python object before locking anything. `path` is the file the node came from, if any.
    pub(crate) fn empty_home(types: Py<TypeRegistry>, path: Option<Arc<str>>) -> Self {
        let kdl = KdlDocument::new();
        let ids = IdTree::from_document(&kdl);
        Document::build(kdl, ids, types, true, "\n", path)
    }

    fn build(
        kdl: KdlDocument,
        ids: IdTree,
        types: Py<TypeRegistry>,
        home: bool,
        eol: &'static str,
        path: Option<Arc<str>>,
    ) -> Self {
        Document {
            state: Mutex::new(DocState { kdl, ids, eol }),
            types,
            home,
            path,
        }
    }

    fn lock(&self) -> MutexGuard<'_, DocState> {
        // A panic while locked means the two trees may disagree, so a poisoned document stays unusable.
        self.state.lock().expect("document state poisoned by an earlier panic")
    }

    /// The only way to touch `DocState`. The closure can't hold a Python token, so it can't call into Python while
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
    /// taking its lock while another document is locked can't deadlock. `eol` is the line ending style of the
    /// document the node came from.
    pub(crate) fn fill(&self, kdl: KdlDocument, ids: IdTree, eol: &'static str) {
        let mut state = self.lock();
        *state = DocState { kdl, ids, eol };
        state.check();
    }

    /// The one handle for `id`. Every way of getting a Node goes through here. See `handles::publish`.
    pub(crate) fn node_handle<'py>(doc: &Bound<'py, Document>, id: NodeId) -> PyResult<Option<Bound<'py, Node>>> {
        handles::publish(doc, id.key(), |home| Node::new(id, home), |s| s.ids.contains(id))
    }
}

/// Locks several distinct documents for an edit that spans them, such as moving nodes into a block. They're locked
/// in address order, so two threads moving nodes in opposite directions can't deadlock, and `f` gets their states in
/// the order given.
pub(crate) fn lock_all<R: Send>(
    py: Python<'_>,
    docs: &[&Document],
    f: impl FnOnce(&mut [&mut DocState]) -> R + Send,
) -> R {
    py.detach(|| {
        let mut order: Vec<usize> = (0..docs.len()).collect();
        order.sort_by_key(|&i| docs.get(i).map(|&d| std::ptr::from_ref(d)));
        let mut guards: Vec<Option<MutexGuard<'_, DocState>>> = docs.iter().map(|_| None).collect();
        for i in order {
            if let (Some(doc), Some(guard)) = (docs.get(i), guards.get_mut(i)) {
                *guard = Some(doc.lock());
            }
        }
        let mut states: Vec<&mut DocState> = guards.iter_mut().flatten().map(|guard| &mut **guard).collect();
        let result = f(&mut states);
        for state in &states {
            state.check();
        }
        result
    })
}

#[pymethods]
impl Document {
    #[new]
    fn py_new(py: Python<'_>) -> PyResult<Self> {
        Ok(Document::new(KdlDocument::new(), TypeRegistry::shared_builtin(py)?))
    }

    // The copy has the same types.
    /// Returns a copy that shares nothing with this document.
    ///
    /// The copy wasn't parsed or loaded, so its nodes and entries have no positions and its `path` is None.
    fn copy(&self, py: Python<'_>) -> PyResult<Document> {
        let (kdl, eol) = self.with_state(py, |s| {
            let deep = parser::too_deep_for_the_stack(s.kdl.nodes());
            parser::recursing(deep, || s.kdl.clone()).map(|kdl| (kdl, s.eol))
        })?;
        let ids = IdTree::from_document(&kdl);
        Ok(Document::build(kdl, ids, self.types.clone_ref(py), false, eol, None))
    }

    /// The file `load` read the document from, or None if it didn't come from a named file.
    ///
    /// With a node's or entry's `line` and `column`, it makes a message like `config.kdl:12:5: unknown key`.
    #[getter]
    fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    fn __copy__(&self, py: Python<'_>) -> PyResult<Document> {
        self.copy(py)
    }

    // A copy shares nothing with the document, so `copy.deepcopy`'s memo has nothing to record.
    fn __deepcopy__(&self, py: Python<'_>, _memo: &Bound<'_, PyAny>) -> PyResult<Document> {
        self.copy(py)
    }

    /// Makes a detached node that reads and writes values through this document's types.
    ///
    /// Takes the same arguments as `Node`. The node isn't added to the document.
    #[pyo3(signature = (name, *args, r#type=None, props=None, children=None, **kwargs))]
    fn node<'py>(
        &self,
        name: String,
        args: &Bound<'py, PyTuple>,
        r#type: Option<&Bound<'py, PyAny>>,
        props: Option<&Bound<'py, PyDict>>,
        children: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, Node>> {
        let py = args.py();
        let spec = Spec {
            ty: r#type,
            props,
            kwargs,
            children,
        };
        let eol = self.with_state(py, |s| s.eol);
        node::build(name, args, &spec, self.types.clone_ref(py), eol)
    }

    // The top-level nodes.
    fn __iter__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        children::iter_children(slf.py(), &Owner::Doc(slf.clone().unbind()))
    }

    // Whether a top-level node matches a name or a `(type, name)` key, as `doc[key]` would find it.
    fn __contains__(slf: &Bound<'_, Self>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        children::contains(&Owner::Doc(slf.clone().unbind()), key)
    }

    /// Iterates over every node in the document, at any depth, depth first in file order.
    ///
    /// Editing the document while iterating is safe: the nodes are found up front.
    ///
    /// Args:
    ///     key: Only nodes that match: a name, or a `(type, name)` tuple. None means every node.
    #[pyo3(signature = (key=None))]
    fn iter<'py>(slf: &Bound<'py, Self>, key: Option<&Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let none = slf.py().None().into_bound(slf.py());
        children::descendants(&Owner::Doc(slf.clone().unbind()), key.unwrap_or(&none))
    }

    fn __str__(&self, py: Python<'_>) -> PyResult<String> {
        self.with_state(py, |s| {
            let deep = parser::too_deep_for_the_stack(s.kdl.nodes());
            parser::recursing(deep, || s.kdl.to_string())
        })
    }

    fn __len__(&self, py: Python<'_>) -> usize {
        self.with_state(py, |s| s.kdl.nodes().len())
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        format!("<Document with {} nodes>", self.__len__(py))
    }

    // autoformat only rewrites format strings, never structure, so the IdTree stays valid.
    /// Reformats the whole document in place, with kdl-rs's autoformatter.
    ///
    /// Args:
    ///     formatter: The settings. None means `Formatter()`.
    #[pyo3(signature = (formatter=None))]
    fn fmt(&self, py: Python<'_>, formatter: Option<Bound<'_, Formatter>>) -> PyResult<()> {
        let formatter = match formatter {
            Some(f) => f,
            None => Formatter::default(py)?,
        };
        let formatter = formatter.get();
        self.with_state(py, |s| {
            let deep = parser::too_deep_for_the_stack(s.kdl.nodes());
            let eol = s.eol;
            parser::recursing(deep, || formatter.apply(&mut s.kdl, eol))
        })
    }

    /// The top-level nodes, as a live list.
    ///
    /// Assigning a list of detached nodes replaces them all. If any of the nodes can't go in, nothing changes.
    #[getter]
    fn children(slf: &Bound<'_, Self>) -> Children {
        Children::new(Owner::Doc(slf.clone().unbind()))
    }

    // Replaces the top-level nodes with detached nodes, all or nothing. Assigning the document's own view does
    // nothing, so `doc.children += nodes` works.
    #[setter]
    fn set_children(slf: &Bound<'_, Self>, value: Option<Bound<'_, PyAny>>) -> PyResult<()> {
        let value =
            value.ok_or_else(|| PyTypeError::new_err("a document always has children; use doc.children.clear()"))?;
        if children::is_view_of(&value, slf.as_any()) {
            return Ok(());
        }
        let nodes = children::nodes_in(&value, "doc.children")?;
        children::splice(slf.py(), &Owner::Doc(slf.clone().unbind()), &At::All, &nodes, false)
    }

    fn __getitem__<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, Node>> {
        children::lookup_one(&Owner::Doc(slf.clone().unbind()), key)
    }

    /// Returns the first top-level node that matches a key, or `default`.
    ///
    /// Args:
    ///     key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
    ///     default: What to return if no node matches.
    #[pyo3(signature = (key, default=None))]
    fn get<'py>(
        slf: &Bound<'py, Self>,
        key: &Bound<'py, PyAny>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        children::lookup_get(&Owner::Doc(slf.clone().unbind()), key, default)
    }

    /// Returns every top-level node that matches a key, in order.
    ///
    /// Args:
    ///     key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
    fn get_all<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, Node>>> {
        children::lookup(&Owner::Doc(slf.clone().unbind()), key, false)
    }

    // The registry is the document's only strong reference; it holds its handles weakly. There's no `__clear__`: the
    // registry never changes, and every cycle through it passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.types)
    }
}
