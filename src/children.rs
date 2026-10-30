use std::sync::Arc;

use kdl::{KdlDocument, KdlNode};
use pyo3::PyTraverseError;
use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::document::{DocState, Document, lock_both};
use crate::handles::{self, Fail, Garbage, Home, stale};
use crate::index::{insert_at, not_in, pick_one, pick_slice, position};
use crate::layout::{self, Style};
use crate::node::Node;
use crate::tree::{IdTree, NodeId, node_at, node_at_mut};

/// Whose children a view or lookup is over. A node's view follows the node when it moves between documents.
pub(crate) enum Owner {
    Doc(Py<Document>),
    Node(Py<Node>),
}

impl Owner {
    /// Runs `op` on the owner's document and parent node (None for the top level), again if the owner moved while
    /// `op` waited for the lock.
    fn on_doc<R>(
        &self,
        py: Python<'_>,
        mut op: impl FnMut(&Py<Document>, Option<NodeId>) -> Result<R, Fail>,
    ) -> PyResult<R> {
        match self {
            Owner::Doc(doc) => handles::on_home(py, &Home::new(doc.clone_ref(py)), |doc| op(doc, None)),
            Owner::Node(node) => {
                let node = node.get();
                handles::on_home(py, &node.home, |doc| op(doc, Some(node.id)))
            }
        }
    }

    fn as_py(&self) -> &Py<PyAny> {
        match self {
            Owner::Doc(doc) => doc.as_any(),
            Owner::Node(node) => node.as_any(),
        }
    }

    /// Reads the owner's document under its lock.
    fn read<R: Send>(
        &self,
        py: Python<'_>,
        f: impl Fn(&DocState, Option<NodeId>) -> Result<R, Fail> + Sync,
    ) -> PyResult<(Py<Document>, R)> {
        self.on_doc(py, |doc, parent| {
            let result = doc.get().with_state(py, |s| f(s, parent))?;
            Ok((doc.clone_ref(py), result))
        })
    }

    fn ids(&self, py: Python<'_>) -> PyResult<(Py<Document>, Vec<NodeId>)> {
        self.read(py, |s, parent| {
            Ok(block_ids(&s.ids, parent)?.ok_or_else(no_block)?.to_vec())
        })
    }

    /// Handles for the nodes `choose` picks from the block. If one of them moves away before its handle exists, the
    /// block is read again.
    fn chosen<'py>(
        &self,
        py: Python<'py>,
        choose: impl Fn(&[NodeId]) -> PyResult<Vec<NodeId>>,
    ) -> PyResult<Vec<Bound<'py, Node>>> {
        loop {
            let (doc, ids) = self.ids(py)?;
            if let Some(found) = handles::publish_all(doc.bind(py), &choose(&ids)?, Document::node_handle)? {
                return Ok(found);
            }
        }
    }
}

fn no_block() -> PyErr {
    PyValueError::new_err("node has no children block")
}

/// The ids in the parent's block, or None if it has no block.
fn block_ids(ids: &IdTree, parent: Option<NodeId>) -> Result<Option<&[NodeId]>, Fail> {
    match parent {
        None => Ok(Some(ids.top())),
        Some(p) if !ids.contains(p) => Err(Fail::Missing(stale())),
        Some(p) => Ok(ids.children(p)),
    }
}

// Only called after block_ids has checked the parent.
fn block<'a>(kdl: &'a KdlDocument, ids: &IdTree, parent: Option<NodeId>) -> &'a KdlDocument {
    match parent {
        None => kdl,
        Some(p) => node_at(kdl, &ids.path(p).expect("checked by block_ids"))
            .children()
            .expect("checked by block_ids"),
    }
}

fn block_mut<'a>(kdl: &'a mut KdlDocument, ids: &IdTree, parent: Option<NodeId>) -> &'a mut KdlDocument {
    match parent {
        None => kdl,
        Some(p) => node_at_mut(kdl, &ids.path(p).expect("checked by block_ids"))
            .children_mut()
            .as_mut()
            .expect("checked by block_ids"),
    }
}

/// How the parent's block lays out its nodes. Only called after `block_ids` has checked the parent.
fn style(s: &DocState, parent: Option<NodeId>) -> Style {
    let path = parent.map(|p| s.ids.path(p).expect("checked by block_ids"));
    Style::of(&s.kdl, path.as_deref())
}

// Structural edits.

fn check_insertable(target: &Py<Document>, source: &Py<Document>) -> PyResult<()> {
    if !source.get().home {
        return Err(PyValueError::new_err(
            "node is already in a document; detach it first (pop, remove, del) or insert node.copy()",
        ));
    }
    // A detached node's home holds only that node's subtree, so the target block is inside the node.
    if source.as_ptr() == target.as_ptr() {
        return Err(PyValueError::new_err("can't insert a node inside itself"));
    }
    Ok(())
}

/// A private document for a node about to be taken out. It's made before locking, so the edit itself needs no
/// Python.
struct NewHome {
    doc: Py<Document>,
    /// For `handles::repoint`, which runs locked and detached, where it can't make references of its own.
    shared: Arc<Py<Document>>,
}

impl NewHome {
    /// The node keeps reading and writing through the types of the document it left.
    fn new(py: Python<'_>, from: &Py<Document>) -> PyResult<NewHome> {
        let doc = Py::new(py, Document::empty_home(from.get().types.clone_ref(py)))?;
        Ok(NewHome {
            shared: Arc::new(doc.clone_ref(py)),
            doc,
        })
    }
}

/// Moves the node `id` out of the parent's block into `to`, along with its handles.
fn detach(
    s: &mut DocState,
    parent: Option<NodeId>,
    id: NodeId,
    style: &Style,
    to: &NewHome,
    garbage: &mut Garbage,
) -> Result<(), Fail> {
    let index = block_ids(&s.ids, parent)?
        .ok_or_else(no_block)?
        .iter()
        .position(|&n| n == id)
        .expect("picked from this block");
    let node = layout::unplace(block_mut(&mut s.kdl, &s.ids, parent), index, style);
    let subtree = s.ids.take_subtree(id);
    let keys = subtree.keys();
    let mut kdl = KdlDocument::new();
    kdl.nodes_mut().push(node);
    to.doc.get().fill(kdl, IdTree::from_subtree(subtree));
    handles::repoint(keys, &to.shared, garbage);
    Ok(())
}

/// Where `insert` puts a node.
#[derive(Clone, Copy)]
enum At {
    /// Like `list.insert(index, node)`.
    Insert(isize),
    /// Like `list[index] = node`: the node there is detached first.
    Replace(isize),
}

fn insert(py: Python<'_>, owner: &Owner, at: At, node: &Bound<'_, Node>) -> PyResult<()> {
    let id = node.get().id;
    let home = &node.get().home;
    let garbage = owner.on_doc(py, |target, parent| {
        let source = home.doc(py);
        check_insertable(target, &source)?;
        let here = Arc::new(target.clone_ref(py));
        let replaced_home = match at {
            At::Replace(_) => Some(NewHome::new(py, target)?),
            At::Insert(_) => None,
        };
        lock_both(py, target.get(), source.get(), |t, s| -> Result<Garbage, Fail> {
            let s = s.expect("checked that the documents differ");
            let ids = block_ids(&t.ids, parent)?.ok_or_else(no_block)?;
            let (index, replaced) = match at {
                At::Insert(index) => (insert_at(ids.len(), index), None),
                At::Replace(index) => {
                    let i = position(ids.len(), index, "children")?;
                    (i, ids.get(i).copied())
                }
            };
            if s.ids.top() != [id] {
                // Another thread inserted it first; the retry will find it attached. `on_doc` follows the target's
                // home, not the node's, so this applies the `Fail::Missing` rule to the node's home itself.
                return Err(if home.moved_from(&source) {
                    Fail::Changed
                } else {
                    Fail::Err(stale())
                });
            }
            let style = style(t, parent);
            let mut garbage = Garbage::default();
            if let (Some(old), Some(to)) = (replaced, &replaced_home) {
                detach(t, parent, old, &style, to, &mut garbage)?;
            }
            let kdl_node = s.kdl.nodes_mut().remove(0);
            let subtree = s.ids.take_subtree(id);
            let keys = subtree.keys();
            layout::place(block_mut(&mut t.kdl, &t.ids, parent), index, kdl_node, &style);
            t.ids.put_subtree(parent, index, subtree);
            handles::repoint(keys, &here, &mut garbage);
            Ok(garbage)
        })
    })?;
    garbage.release(py);
    Ok(())
}

/// Takes the nodes `pick` chooses out of the block, each into a private document of its own, and returns their
/// handles. `close` also removes the block itself, and is a no-op if there is none.
fn take_out<'py>(
    py: Python<'py>,
    owner: &Owner,
    pick: impl Fn(&[NodeId]) -> PyResult<Vec<NodeId>>,
    close: bool,
) -> PyResult<Vec<Bound<'py, Node>>> {
    let taken = owner.on_doc(py, |doc, parent| {
        let snapshot = doc.get().with_state(py, |s| match block_ids(&s.ids, parent)? {
            Some(ids) => Ok::<_, Fail>(Some(ids.to_vec())),
            None if close => Ok(None),
            None => Err(no_block().into()),
        })?;
        let Some(snapshot) = snapshot else {
            return Ok(Vec::new());
        };
        let picked = pick(&snapshot)?;
        let homes = picked
            .iter()
            .map(|_| NewHome::new(py, doc))
            .collect::<PyResult<Vec<_>>>()?;

        let garbage = doc.get().with_state(py, |s| -> Result<Garbage, Fail> {
            match block_ids(&s.ids, parent)? {
                Some(ids) if *ids == snapshot[..] => {}
                Some(_) => return Err(Fail::Changed),
                None if close => return Err(Fail::Changed),
                None => return Err(no_block().into()),
            }
            let style = style(s, parent);
            let mut garbage = Garbage::default();
            for (&id, to) in picked.iter().zip(&homes) {
                detach(s, parent, id, &style, to, &mut garbage)?;
            }
            if let (true, Some(p)) = (close, parent) {
                node_at_mut(&mut s.kdl, &s.ids.path(p).expect("checked by block_ids")).clear_children();
                s.ids.close_block(p);
            }
            Ok(garbage)
        })?;
        garbage.release(py);
        Ok(picked
            .into_iter()
            .zip(homes)
            .map(|(id, home)| (id, home.doc))
            .collect::<Vec<_>>())
    })?;

    // Nothing else can reach the new homes yet, so each node is still in its own.
    taken
        .into_iter()
        .map(|(id, home)| Document::node_handle(home.bind(py), id)?.ok_or_else(stale))
        .collect()
}

fn take_all(ids: &[NodeId]) -> PyResult<Vec<NodeId>> {
    Ok(ids.to_vec())
}

/// `node.children = []`: an empty block, detaching any children it had.
pub(crate) fn open_block(py: Python<'_>, owner: &Owner) -> PyResult<()> {
    let had_block = owner.on_doc(py, |doc, parent| {
        let id = parent.expect("only nodes open blocks");
        doc.get().with_state(py, |s| -> Result<bool, Fail> {
            let path = s.ids.path(id).ok_or_else(|| Fail::Missing(stale()))?;
            let eol = layout::line_ending(&s.kdl);
            let node = node_at_mut(&mut s.kdl, &path);
            if node.children().is_some() {
                return Ok(true);
            }
            layout::open_block(node, eol);
            s.ids.open_block(id);
            Ok(false)
        })
    })?;
    if had_block {
        take_out(py, owner, take_all, false)?;
    }
    Ok(())
}

/// `node.children = None`: removes the block, detaching any children it had.
pub(crate) fn close_block(py: Python<'_>, owner: &Owner) -> PyResult<()> {
    take_out(py, owner, take_all, true)?;
    Ok(())
}

/// Reverses the block in place.
fn reverse(py: Python<'_>, owner: &Owner) -> PyResult<()> {
    owner.on_doc(py, |doc, parent| {
        doc.get().with_state(py, |s| -> Result<(), Fail> {
            let ids = block_ids(&s.ids, parent)?.ok_or_else(no_block)?.to_vec();
            let style = style(s, parent);
            // Moves the first node back to each place in turn, last place first. The block never empties, and the
            // node that ends up last takes over the end of the block, so the block keeps its layout.
            for (&id, to) in ids.iter().zip((1..ids.len()).rev()) {
                let node = layout::unplace(block_mut(&mut s.kdl, &s.ids, parent), 0, &style);
                let subtree = s.ids.take_subtree(id);
                layout::place(block_mut(&mut s.kdl, &s.ids, parent), to, node, &style);
                s.ids.put_subtree(parent, to, subtree);
            }
            Ok(())
        })
    })
}

// Lookups.

enum Key {
    Name(String),
    Typed(Option<String>, String),
}

impl Key {
    fn extract(key: &Bound<'_, PyAny>) -> PyResult<Key> {
        if let Ok(name) = key.extract::<String>() {
            return Ok(Key::Name(name));
        }
        if let Ok((ty, name)) = key.extract::<(Option<String>, String)>() {
            return Ok(Key::Typed(ty, name));
        }
        Err(PyTypeError::new_err("a node key is a name or a (type, name) tuple"))
    }

    fn matches(&self, node: &KdlNode) -> bool {
        match self {
            Key::Name(name) => node.name().value() == name,
            Key::Typed(ty, name) => node.name().value() == name && node.ty().map(|t| t.value()) == ty.as_deref(),
        }
    }
}

pub(crate) fn lookup<'py>(owner: &Owner, key: &Bound<'py, PyAny>, first: bool) -> PyResult<Vec<Bound<'py, Node>>> {
    let py = key.py();
    let key = Key::extract(key)?;
    loop {
        let (doc, found) = owner.read(py, |s, parent| {
            // No block means no children to match.
            let Some(ids) = block_ids(&s.ids, parent)? else {
                return Ok(Vec::new());
            };
            let matching = ids
                .iter()
                .zip(block(&s.kdl, &s.ids, parent).nodes())
                .filter(|(_, n)| key.matches(n));
            Ok(matching
                .take(if first { 1 } else { usize::MAX })
                .map(|(&id, _)| id)
                .collect())
        })?;
        if let Some(found) = handles::publish_all(doc.bind(py), &found, Document::node_handle)? {
            return Ok(found);
        }
    }
}

pub(crate) fn lookup_one<'py>(owner: &Owner, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, Node>> {
    lookup(owner, key, true)?
        .pop()
        .ok_or_else(|| PyKeyError::new_err(key.clone().unbind()))
}

pub(crate) fn lookup_get<'py>(
    owner: &Owner,
    key: &Bound<'py, PyAny>,
    default: Option<Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    match lookup(owner, key, true)?.pop() {
        Some(node) => Ok(node.into_any()),
        None => Ok(default.unwrap_or_else(|| key.py().None().into_bound(key.py()))),
    }
}

// The view.

/// A live list of child nodes.
#[pyclass(module = "rkdl", frozen)]
pub struct Children {
    owner: Owner,
}

impl Children {
    pub(crate) fn new(owner: Owner) -> Self {
        Children { owner }
    }
}

/// Whether `value` is a children view of `owner`, a document or node.
pub(crate) fn is_view_of(value: &Bound<'_, PyAny>, owner: &Bound<'_, PyAny>) -> bool {
    value
        .cast::<Children>()
        .is_ok_and(|view| view.get().owner.as_py().is(owner))
}

/// Where `node` is among `ids`.
fn find_node(ids: &[NodeId], node: &Bound<'_, Node>) -> PyResult<usize> {
    ids.iter()
        .position(|&n| n == node.get().id)
        .ok_or_else(|| not_in(node.as_any(), "children"))
}

/// The id `index` points at.
fn pick_id(ids: &[NodeId], index: isize) -> PyResult<Vec<NodeId>> {
    Ok(vec![*pick_one(ids, index, "children")?])
}

fn slice_ids(ids: &[NodeId], slice: &Bound<'_, PySlice>) -> PyResult<Vec<NodeId>> {
    Ok(pick_slice(ids, slice)?.into_iter().copied().collect())
}

#[pymethods]
impl Children {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(self
            .owner
            .read(py, |s, parent| {
                Ok(block_ids(&s.ids, parent)?.ok_or_else(no_block)?.len())
            })?
            .1)
    }

    fn __getitem__<'py>(&self, index: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = index.py();
        if let Ok(slice) = index.cast::<PySlice>() {
            let found = self.owner.chosen(py, |ids| slice_ids(ids, slice))?;
            return Ok(PyList::new(py, found)?.into_any());
        }
        let index: isize = index.extract()?;
        let mut found = self.owner.chosen(py, |ids| pick_id(ids, index))?;
        Ok(found.pop().expect("chose one node").into_any())
    }

    fn __setitem__(&self, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
        insert(node.py(), &self.owner, At::Replace(index), node)
    }

    fn __delitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = index.py();
        match index.cast::<PySlice>() {
            Ok(slice) => take_out(py, &self.owner, |ids| slice_ids(ids, slice), false)?,
            Err(_) => {
                let index: isize = index.extract()?;
                take_out(py, &self.owner, |ids| pick_id(ids, index), false)?
            }
        };
        Ok(())
    }

    // Snapshots the handles first, so editing the list while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let found = self.owner.chosen(py, take_all)?;
        Ok(PyList::new(py, found)?.try_iter()?.into_any())
    }

    fn __reversed__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut found = self.owner.chosen(py, take_all)?;
        found.reverse();
        Ok(PyList::new(py, found)?.try_iter()?.into_any())
    }

    fn __contains__(&self, node: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Ok(node) = node.cast::<Node>() else {
            return Ok(false);
        };
        Ok(self.owner.ids(node.py())?.1.contains(&node.get().id))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let found = self.owner.chosen(py, take_all)?;
        Ok(format!("Children({})", PyList::new(py, found)?.repr()?))
    }

    fn __iadd__(&self, nodes: &Bound<'_, PyAny>) -> PyResult<()> {
        self.extend(nodes)
    }

    fn index(&self, node: &Bound<'_, Node>) -> PyResult<usize> {
        find_node(&self.owner.ids(node.py())?.1, node)
    }

    fn count(&self, node: &Bound<'_, PyAny>) -> PyResult<usize> {
        Ok(usize::from(self.__contains__(node)?))
    }

    fn insert(&self, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
        insert(node.py(), &self.owner, At::Insert(index), node)
    }

    fn append(&self, node: &Bound<'_, Node>) -> PyResult<()> {
        insert(node.py(), &self.owner, At::Insert(isize::MAX), node)
    }

    fn extend(&self, nodes: &Bound<'_, PyAny>) -> PyResult<()> {
        // Collected first, so `children.extend(children)` ends (by raising: those nodes are already attached).
        let nodes: Vec<_> = nodes.try_iter()?.collect::<PyResult<_>>()?;
        nodes.iter().try_for_each(|node| self.append(node.cast::<Node>()?))
    }

    #[pyo3(signature = (index=-1))]
    fn pop<'py>(&self, py: Python<'py>, index: isize) -> PyResult<Bound<'py, Node>> {
        let mut taken = take_out(py, &self.owner, |ids| pick_id(ids, index), false)?;
        Ok(taken.pop().expect("took one node"))
    }

    fn remove(&self, node: &Bound<'_, Node>) -> PyResult<()> {
        take_out(
            node.py(),
            &self.owner,
            |ids| find_node(ids, node).map(|_| vec![node.get().id]),
            false,
        )?;
        Ok(())
    }

    fn reverse(&self, py: Python<'_>) -> PyResult<()> {
        reverse(py, &self.owner)
    }

    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        take_out(py, &self.owner, take_all, false)?;
        Ok(())
    }

    // No `__clear__`, as for the other views: any cycle through the owner passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(self.owner.as_py())
    }
}
