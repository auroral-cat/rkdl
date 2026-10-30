use std::sync::Arc;

use kdl::{KdlDocument, KdlNode};
use pyo3::PyTraverseError;
use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::document::{DocState, Document, lock_all};
use crate::handles::{self, DocRef, Fail, Garbage, Home, stale};
use crate::index::{Index, Splice, insert_at, not_in, pick_one, pick_slice, position, search_range};
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
        handles::retry(|| {
            let (doc, ids) = self.ids(py)?;
            handles::publish_all(doc.bind(py), &choose(&ids)?, Document::node_handle)
        })
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
    Style::of(&s.kdl, path.as_deref(), s.eol)
}

// Structural edits.

fn attached() -> PyErr {
    PyValueError::new_err("node is already in a document; detach it first (pop, remove, del) or insert node.copy()")
}

/// Checks, under the lock of the node's private home `source`, that the node `id` is still the detached node there,
/// its only top-level node. A node inside a detached node is in a document like any other.
fn check_detached(id: NodeId, home: &Home, source_doc: &DocRef, source: &DocState) -> Result<(), Fail> {
    if source.ids.top() == [id] {
        return Ok(());
    }
    if source.ids.contains(id) {
        return Err(attached().into());
    }
    // Another thread inserted it first; the retry will find it attached. `on_doc` follows the target's home, not the
    // node's, so this applies the `Fail::Missing` rule to the node's home itself.
    Err(if home.moved_from(source_doc) {
        Fail::Changed
    } else {
        Fail::Err(stale())
    })
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
    rehome(s, id, node, to, garbage);
    Ok(())
}

/// Moves the node `id`, already taken out of the `KdlDocument` as `node`, into `to` along with its handles.
fn rehome(s: &mut DocState, id: NodeId, node: KdlNode, to: &NewHome, garbage: &mut Garbage) {
    let subtree = s.ids.take_subtree(id);
    let keys = subtree.keys();
    let mut kdl = KdlDocument::new();
    kdl.nodes_mut().push(node);
    to.doc.get().fill(kdl, IdTree::from_subtree(subtree), s.eol);
    handles::repoint(keys, &to.shared, garbage);
}

/// Which of a block's nodes a splice replaces, by list rules.
pub(crate) enum At<'py> {
    /// `list.insert(index, node)`, or `extend` with several.
    Insert(isize),
    /// `list[index] = node`.
    Replace(isize),
    /// `list[slice] = nodes`.
    Slice(Bound<'py, PySlice>),
    /// `list[:] = nodes`.
    All,
}

impl At<'_> {
    fn splice(&self, len: usize, new: usize) -> PyResult<Splice> {
        Ok(match self {
            At::Insert(index) => Splice {
                picked: Vec::new(),
                insert_at: insert_at(len, *index),
            },
            At::Replace(index) => Splice {
                picked: vec![position(len, *index, "children")?],
                insert_at: 0,
            },
            At::Slice(slice) => Splice::of(len, slice, new)?,
            At::All => Splice::all(len),
        })
    }
}

fn inside_itself() -> PyErr {
    PyValueError::new_err("can't insert a node inside itself")
}

/// Why nodes whose homes aren't all distinct from each other and from the target can't go in, or `Fail::Changed`
/// if, against the odds, nothing is wrong. A detached node's home holds only that node's subtree, so a target block
/// in a node's home is inside the node, and two nodes sharing a home can't both be detached.
fn shared_home(py: Python<'_>, target: &Py<Document>, nodes: &[(NodeId, &Home)], sources: &[DocRef]) -> Option<Fail> {
    for (i, source) in sources.iter().enumerate() {
        let (id, home) = *nodes.get(i)?;
        if source.is(target) {
            return Some(
                target
                    .get()
                    .with_state(py, |t| match check_detached(id, home, source, t) {
                        Ok(()) => inside_itself().into(),
                        Err(fail) => fail,
                    }),
            );
        }
        if let Some(j) = sources.iter().take(i).position(|other| other.is(&**source)) {
            let (other_id, other_home) = *nodes.get(j)?;
            let other = sources.get(j)?;
            return Some(source.get().with_state(py, |s| {
                check_detached(other_id, other_home, other, s)
                    .and_then(|()| check_detached(id, home, source, s))
                    .err()
                    .unwrap_or(Fail::Changed)
            }));
        }
    }
    None
}

/// Puts detached `nodes` into the owner's block in place of the nodes `at` picks, by list rules, all in one step: if
/// any of them can't go in, nothing changes. A picked node that gets a new one in its place hands it its
/// whitespace, as `children[i] = node` does; the rest are detached. `open` gives a node without a children block one
/// first.
pub(crate) fn splice(
    py: Python<'_>,
    owner: &Owner,
    at: &At<'_>,
    nodes: &[Bound<'_, Node>],
    open: bool,
) -> PyResult<()> {
    let moving: Vec<(NodeId, &Home)> = nodes.iter().map(|n| (n.get().id, &*n.get().home)).collect();
    for (i, (id, _)) in moving.iter().enumerate() {
        if moving.iter().take(i).any(|(other, _)| other == id) {
            return Err(PyValueError::new_err("can't insert the same node twice"));
        }
    }
    let garbage = owner.on_doc(py, |target, parent| {
        let snapshot = target.get().with_state(py, |s| -> Result<_, Fail> {
            Ok(block_ids(&s.ids, parent)?.map(<[NodeId]>::to_vec))
        })?;
        let len = match (&snapshot, open) {
            (Some(ids), _) => ids.len(),
            (None, true) => 0,
            (None, false) => return Err(no_block().into()),
        };
        let splice = at.splice(len, nodes.len())?;
        // Only a node's home, never an entry's, empties.
        let sources = moving
            .iter()
            .map(|(_, home)| home.doc(py).ok_or_else(stale))
            .collect::<PyResult<Vec<_>>>()?;
        if sources.iter().any(|source| !source.get().home) {
            return Err(attached().into());
        }
        if let Some(fail) = shared_home(py, target, &moving, &sources) {
            return Err(fail);
        }
        let homes = splice
            .picked
            .iter()
            .map(|_| NewHome::new(py, target))
            .collect::<PyResult<Vec<_>>>()?;
        let here = Arc::new(target.clone_ref(py));
        let docs: Vec<&Document> = std::iter::once(target.get())
            .chain(sources.iter().map(|s| s.get()))
            .collect();
        lock_all(py, &docs, |states| -> Result<Garbage, Fail> {
            let (t, from) = states.split_first_mut().expect("the target is locked first");
            for ((id, home), (source, s)) in moving.iter().zip(sources.iter().zip(from.iter())) {
                check_detached(*id, home, source, s)?;
            }
            if block_ids(&t.ids, parent)?.map(<[NodeId]>::to_vec) != snapshot {
                return Err(Fail::Changed);
            }
            let old = snapshot.clone().unwrap_or_default();
            if let (None, Some(p)) = (&snapshot, parent) {
                let path = t.ids.path(p).expect("checked by block_ids");
                layout::open_block(&mut t.kdl, &path, t.eol);
                t.ids.open_block(p);
            }
            let style = style(t, parent);
            let mut garbage = Garbage::default();
            let mut new = moving.iter().zip(from.iter_mut()).map(|(&(id, _), s)| {
                let node = s.kdl.nodes_mut().remove(0);
                (node, s.ids.take_subtree(id))
            });
            // Replacements come first and leave every position where it was; only then are any left over detached.
            for (&index, to) in splice.picked.iter().zip(&homes) {
                let replaced = *old.get(index).expect("picked from this block");
                let Some((node, subtree)) = new.next() else {
                    detach(t, parent, replaced, &style, to, &mut garbage)?;
                    continue;
                };
                // The new node takes the old one's place in one step, so the block never empties on the way.
                let keys = subtree.keys();
                let replaced_node = layout::replace(block_mut(&mut t.kdl, &t.ids, parent), index, node, &style);
                rehome(t, replaced, replaced_node, to, &mut garbage);
                t.ids.put_subtree(parent, index, subtree);
                handles::repoint(keys, &here, &mut garbage);
            }
            for (i, (node, subtree)) in new.enumerate() {
                let keys = subtree.keys();
                let index = splice.insert_at + i;
                layout::place(block_mut(&mut t.kdl, &t.ids, parent), index, node, &style);
                t.ids.put_subtree(parent, index, subtree);
                handles::repoint(keys, &here, &mut garbage);
            }
            Ok(garbage)
        })
    })?;
    handles::edited();
    garbage.release(py);
    Ok(())
}

/// The nodes in an iterable, for a splice. Anything else raises `TypeError`, naming `what` was being set.
#[expect(
    clippy::map_err_ignore,
    reason = "this message says what was being set; PyO3's doesn't"
)]
pub(crate) fn nodes_in<'py>(iterable: &Bound<'py, PyAny>, what: &str) -> PyResult<Vec<Bound<'py, Node>>> {
    let not_nodes = || PyTypeError::new_err(format!("{what} takes an iterable of Nodes"));
    let Ok(items) = iterable.try_iter() else {
        return Err(not_nodes());
    };
    items
        .map(|item| item?.cast_into::<Node>().map_err(|_| not_nodes()))
        .collect()
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
        handles::edited();
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

/// `node.children = None`: removes the block, detaching any children it had.
pub(crate) fn close_block(py: Python<'_>, owner: &Owner) -> PyResult<()> {
    take_out(py, owner, take_all, true)?;
    Ok(())
}

/// `node.detach()`: takes the node out of whatever block it's in. A detached node stays where it is.
pub(crate) fn detach_node(py: Python<'_>, node: &Node) -> PyResult<()> {
    let id = node.id;
    let garbage = handles::on_home(py, &node.home, |doc| {
        let parent = doc.get().with_state(py, |s| -> Result<_, Fail> {
            if !s.ids.contains(id) {
                return Err(Fail::Missing(stale()));
            }
            Ok(s.ids.parent(id))
        })?;
        // A private home's one top-level node is the detached node.
        if parent.is_none() && doc.get().home {
            return Ok(None);
        }
        let to = NewHome::new(py, doc)?;
        doc.get().with_state(py, |s| {
            if !s.ids.contains(id) || s.ids.parent(id) != parent {
                return Err(Fail::Changed);
            }
            let style = style(s, parent);
            let mut garbage = Garbage::default();
            detach(s, parent, id, &style, &to, &mut garbage)?;
            Ok(Some(garbage))
        })
    })?;
    if let Some(garbage) = garbage {
        handles::edited();
        garbage.release(py);
    }
    Ok(())
}

/// `node.parent`: None at the top level, and for a detached node.
pub(crate) fn parent<'py>(py: Python<'py>, node: &Node) -> PyResult<Option<Bound<'py, Node>>> {
    handles::retry(|| {
        let (doc, parent) = handles::on_home(py, &node.home, |doc| {
            let parent = doc.get().with_state(py, |s| -> Result<_, Fail> {
                if !s.ids.contains(node.id) {
                    return Err(Fail::Missing(stale()));
                }
                Ok(s.ids.parent(node.id))
            })?;
            Ok((doc.clone_ref(py), parent))
        })?;
        match parent {
            None => Ok(Some(None)),
            Some(p) => Ok(Document::node_handle(doc.bind(py), p)?.map(Some)),
        }
    })
}

/// `for n in doc` and `for n in node`: the children, read once up front. A node without a block has none.
pub(crate) fn iter_children<'py>(py: Python<'py>, owner: &Owner) -> PyResult<Bound<'py, PyAny>> {
    let found = handles::retry(|| {
        let (doc, ids) = owner.read(py, |s, parent| {
            Ok(block_ids(&s.ids, parent)?.map(<[NodeId]>::to_vec).unwrap_or_default())
        })?;
        handles::publish_all(doc.bind(py), &ids, Document::node_handle)
    })?;
    Ok(PyList::new(py, found)?.try_iter()?.into_any())
}

/// `doc.iter(key)` and `node.iter(key)`: every node below the owner that `key` matches (every node for None), depth
/// first in file order. It walks a snapshot, so editing while iterating is safe, and a work list, so deep documents
/// don't run out of stack.
pub(crate) fn descendants<'py>(owner: &Owner, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let py = key.py();
    let key = if key.is_none() { None } else { Some(Key::extract(key)?) };
    let found = handles::retry(|| {
        let (doc, found) = owner.read(py, |s, parent| {
            let Some(top) = block_ids(&s.ids, parent)? else {
                return Ok(Vec::new());
            };
            let mut found = Vec::new();
            let mut pending: Vec<(NodeId, &KdlNode)> = top
                .iter()
                .copied()
                .zip(block(&s.kdl, &s.ids, parent).nodes())
                .rev()
                .collect();
            while let Some((id, node)) = pending.pop() {
                if key.as_ref().is_none_or(|k| k.matches(node)) {
                    found.push(id);
                }
                if let (Some(ids), Some(block)) = (s.ids.children(id), node.children()) {
                    pending.extend(ids.iter().copied().zip(block.nodes()).rev());
                }
            }
            Ok(found)
        })?;
        handles::publish_all(doc.bind(py), &found, Document::node_handle)
    })?;
    Ok(PyList::new(py, found)?.try_iter()?.into_any())
}

/// `key in doc` and `key in node`: whether a child matches `key`, as `doc[key]` would find it.
pub(crate) fn contains(owner: &Owner, key: &Bound<'_, PyAny>) -> PyResult<bool> {
    if key.is_instance_of::<Node>() {
        return Err(PyTypeError::new_err(
            "`in` on a document or node tests a name or a (type, name) key; use `node in parent.children` for a node",
        ));
    }
    Ok(!lookup(owner, key, true)?.is_empty())
}

/// Reverses the block in place.
fn reverse(py: Python<'_>, owner: &Owner) -> PyResult<()> {
    owner.on_doc(py, |doc, parent| {
        doc.get().with_state(py, |s| -> Result<(), Fail> {
            let ids = block_ids(&s.ids, parent)?.ok_or_else(no_block)?.to_vec();
            let style = style(s, parent);
            layout::reverse(block_mut(&mut s.kdl, &s.ids, parent), &style);
            let subtrees: Vec<_> = ids.iter().map(|&id| s.ids.take_subtree(id)).collect();
            for (index, subtree) in subtrees.into_iter().rev().enumerate() {
                s.ids.put_subtree(parent, index, subtree);
            }
            Ok(())
        })
    })?;
    handles::edited();
    Ok(())
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
    handles::retry(|| {
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
        handles::publish_all(doc.bind(py), &found, Document::node_handle)
    })
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
        let Index(index) = index.extract()?;
        let mut found = self.owner.chosen(py, |ids| pick_id(ids, index))?;
        Ok(found.pop().expect("chose one node").into_any())
    }

    fn __setitem__(&self, index: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = value.py();
        if let Ok(slice) = index.cast::<PySlice>() {
            let nodes = nodes_in(value, "slice assignment to children")?;
            return splice(py, &self.owner, &At::Slice(slice.clone()), &nodes, false);
        }
        let Index(index) = index.extract()?;
        let node = value.cast::<Node>()?;
        splice(py, &self.owner, &At::Replace(index), std::slice::from_ref(node), false)
    }

    fn __delitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = index.py();
        match index.cast::<PySlice>() {
            Ok(slice) => take_out(py, &self.owner, |ids| slice_ids(ids, slice), false)?,
            Err(_) => {
                let Index(index) = index.extract()?;
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

    fn __repr__(&self, py: Python<'_>) -> String {
        let Ok(found) = self.owner.chosen(py, take_all) else {
            return "Children(<no children block>)".to_owned();
        };
        let found: Vec<_> = found.iter().map(|node| node.get().repr(py)).collect();
        format!("Children([{}])", found.join(", "))
    }

    /// Equal to a `Children` or a list holding the same nodes in the same order. Like a list, it isn't equal to a
    /// tuple.
    fn __eq__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        let theirs: Vec<Option<NodeId>> = if let Ok(other) = other.cast::<Children>() {
            other.get().owner.ids(py)?.1.into_iter().map(Some).collect()
        } else if let Ok(other) = other.cast::<PyList>() {
            other
                .iter()
                .map(|n| n.cast::<Node>().ok().map(|n| n.get().id))
                .collect()
        } else {
            return Ok(py.NotImplemented().into_bound(py));
        };
        let ours = self.owner.ids(py)?.1;
        let same = ours.len() == theirs.len() && ours.iter().zip(&theirs).all(|(a, b)| Some(*a) == *b);
        Ok(same.into_pyobject(py)?.to_owned().into_any())
    }

    fn __iadd__(&self, nodes: &Bound<'_, PyAny>) -> PyResult<()> {
        self.extend(nodes)
    }

    #[pyo3(signature = (node, start=Index(0), stop=Index(isize::MAX)))]
    fn index(&self, node: &Bound<'_, PyAny>, start: Index, stop: Index) -> PyResult<usize> {
        let ids = self.owner.ids(node.py())?.1;
        let range = search_range(ids.len(), start, stop);
        let found = node.cast::<Node>().ok().and_then(|node| {
            let id = node.get().id;
            ids.get(range.clone())?.iter().position(|&n| n == id)
        });
        found.map(|i| range.start + i).ok_or_else(|| not_in(node, "children"))
    }

    fn count(&self, node: &Bound<'_, PyAny>) -> PyResult<usize> {
        Ok(usize::from(self.__contains__(node)?))
    }

    fn insert(&self, index: Index, node: &Bound<'_, Node>) -> PyResult<()> {
        splice(
            node.py(),
            &self.owner,
            &At::Insert(index.0),
            std::slice::from_ref(node),
            false,
        )
    }

    fn append(&self, node: &Bound<'_, Node>) -> PyResult<()> {
        self.insert(Index(isize::MAX), node)
    }

    /// All or nothing: if one of the nodes can't go in, none do.
    fn extend(&self, nodes: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = nodes.py();
        // Collected first, so `children.extend(children)` ends (by raising: those nodes are already attached).
        let nodes = nodes_in(nodes, "children.extend")?;
        splice(py, &self.owner, &At::Insert(isize::MAX), &nodes, false)
    }

    #[pyo3(signature = (index=Index(-1)))]
    fn pop<'py>(&self, py: Python<'py>, index: Index) -> PyResult<Bound<'py, Node>> {
        let mut taken = take_out(py, &self.owner, |ids| pick_id(ids, index.0), false)?;
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
