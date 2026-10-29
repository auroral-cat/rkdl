use kdl::{KdlDocument, KdlDocumentFormat, KdlNode, KdlNodeFormat};
use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::document::{Document, lock_both};
use crate::node::{Node, stale};
use crate::tree::{IdTree, NodeId, Subtree, node_at, node_at_mut};

/// Whose children a view or lookup is over. A node's view follows the node when it moves between documents.
pub(crate) enum Owner {
    Doc(Py<Document>),
    Node(Py<Node>),
}

impl Owner {
    /// The document and the parent node, or None for the top level.
    fn locate(&self, py: Python<'_>) -> (Py<Document>, Option<NodeId>) {
        match self {
            Owner::Doc(doc) => (doc.clone_ref(py), None),
            Owner::Node(node) => (node.get().doc(py), Some(node.get().id)),
        }
    }
}

fn no_block() -> PyErr {
    PyValueError::new_err("node has no children block")
}

fn block_ids(ids: &IdTree, parent: Option<NodeId>) -> PyResult<&[NodeId]> {
    match parent {
        None => Ok(ids.top()),
        Some(p) if !ids.contains(p) => Err(stale()),
        Some(p) => ids.children(p).ok_or_else(no_block),
    }
}

// Only called after block_ids has checked the parent.
fn block_mut<'a>(kdl: &'a mut KdlDocument, ids: &IdTree, parent: Option<NodeId>) -> &'a mut KdlDocument {
    match parent {
        None => kdl,
        Some(p) => node_at_mut(kdl, &ids.path(p).expect("checked by block_ids"))
            .children_mut()
            .as_mut()
            .expect("checked by block_ids"),
    }
}

fn subtree_ids(ids: &IdTree, root: NodeId) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        found.push(id);
        stack.extend(ids.children(id).into_iter().flatten());
    }
    found
}

fn handles<'py>(py: Python<'py>, doc: &Py<Document>, ids: &[NodeId]) -> PyResult<Vec<Bound<'py, Node>>> {
    let doc = doc.bind(py);
    ids.iter().map(|&id| Document::node_handle(doc, id)).collect()
}

// Whitespace and indentation. See notes/kdl-rs.md, "Editing".

fn leading(node: &KdlNode) -> &str {
    node.format().map_or("", |f| &f.leading)
}

/// The indentation in a leading: the whitespace that starts its last line.
fn indent_of(leading: &str) -> &str {
    let line = leading.rsplit('\n').next().unwrap_or("");
    let end = line.find(|c: char| !c.is_whitespace()).unwrap_or(line.len());
    &line[..end]
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

fn fmt_mut(node: &mut KdlNode) -> &mut KdlNodeFormat {
    if node.format().is_none() {
        node.set_format(KdlNodeFormat {
            before_children: " ".into(),
            terminator: "\n".into(),
            ..Default::default()
        });
    }
    node.format_mut().expect("just set")
}

fn block_fmt_mut(block: &mut KdlDocument) -> &mut KdlDocumentFormat {
    if block.format().is_none() {
        block.set_format(KdlDocumentFormat::default());
    }
    block.format_mut().expect("just set")
}

/// One level of indentation, from the first indented child in the document.
fn indent_unit(kdl: &KdlDocument) -> String {
    kdl.nodes()
        .iter()
        .filter_map(|n| n.children()?.nodes().first())
        .map(|child| indent_of(leading(child)))
        .find(|indent| !indent.is_empty())
        .unwrap_or("    ")
        .to_owned()
}

/// Swaps an `old` indent for `new` at the start of each line. `line_start` says whether the text itself starts a
/// line. Blank lines stay blank.
fn reindent(text: &str, old: &str, new: &str, line_start: bool) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let blank = line.is_empty() && i + 1 < lines.len();
        match line.strip_prefix(old) {
            Some(rest) if (i > 0 || line_start) && !blank => {
                out.push_str(new);
                out.push_str(rest);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// Moves everything under `node` from indent `old` to indent `new`, so a moved subtree lines up with its new place.
fn reindent_children(node: &mut KdlNode, old: &str, new: &str) {
    let Some(block) = node.children_mut() else {
        return;
    };
    let mut line_start = false;
    if let Some(f) = block.format_mut() {
        f.leading = reindent(&f.leading, old, new, false);
        line_start = f.leading.ends_with('\n');
    }
    for child in block.nodes_mut() {
        if let Some(f) = child.format_mut() {
            f.leading = reindent(&f.leading, old, new, line_start);
            line_start = format!("{}{}", f.terminator, f.trailing).ends_with('\n');
        }
        reindent_children(child, old, new);
    }
    if let Some(f) = block.format_mut() {
        f.trailing = reindent(&f.trailing, old, new, line_start);
    }
}

/// Puts `node` into `block` at `index` and fixes up whitespace so it lines up with its siblings. `nested` is the
/// parent's indent and one indent level, or None at the top level of a document.
fn place(block: &mut KdlDocument, index: usize, mut node: KdlNode, nested: Option<(&str, &str)>) {
    let len = block.nodes().len();
    let (indent, newline) = match nested {
        None => (String::new(), false),
        Some((parent_indent, unit)) if len == 0 => {
            let f = block_fmt_mut(block);
            // The new node brings its own newline after `{`, and the closing `}` lines up with the parent.
            if is_blank(&f.leading) && is_blank(&f.trailing) {
                f.leading.clear();
                f.trailing = parent_indent.to_owned();
            }
            (format!("{parent_indent}{unit}"), !f.leading.ends_with('\n'))
        }
        Some(_) => {
            let nodes = block.nodes_mut();
            let sibling = index.saturating_sub(1);
            let indent = indent_of(leading(&nodes[sibling])).to_owned();
            // The newline after `{` is in the old first child's leading. It belongs to whichever node comes first.
            let mut newline = false;
            if index == 0 {
                let f = fmt_mut(&mut nodes[0]);
                if let Some(rest) = f.leading.strip_prefix('\n') {
                    f.leading = rest.to_owned();
                    newline = true;
                }
            }
            (indent, newline)
        }
    };

    // The last node of a block or file may have no terminator at all.
    if index > 0 {
        let f = fmt_mut(&mut block.nodes_mut()[index - 1]);
        if f.terminator.is_empty() {
            f.terminator = "\n".into();
        }
    }

    let f = fmt_mut(&mut node);
    let old = indent_of(&f.leading).to_owned();
    // Keep the node's own comments, but not the whitespace it had where it came from.
    let own = reindent(f.leading.trim_start(), &old, &indent, false);
    f.leading = format!("{}{indent}{own}", if newline { "\n" } else { "" });
    if !f.terminator.ends_with('\n') {
        f.terminator = "\n".into();
    }
    if is_blank(&f.trailing) {
        f.trailing.clear();
    }
    if old != indent {
        reindent_children(&mut node, &old, &indent);
    }
    block.nodes_mut().insert(index, node);
}

/// Takes the node at `index` out of `block`, the inverse of `place`.
fn unplace(block: &mut KdlDocument, index: usize, nested: bool) -> KdlNode {
    let node = block.nodes_mut().remove(index);
    if !nested {
        return node;
    }
    if index == 0
        && leading(&node).starts_with('\n')
        && let Some(next) = block.nodes_mut().first_mut()
    {
        let f = fmt_mut(next);
        if !f.leading.starts_with('\n') {
            f.leading.insert(0, '\n');
        }
    }
    // An emptied block prints as `{}`.
    if block.nodes().is_empty()
        && let Some(f) = block.format_mut()
        && is_blank(&f.leading)
        && is_blank(&f.trailing)
    {
        f.leading.clear();
        f.trailing.clear();
    }
    node
}

// Structural edits.

fn check_insertable(target: &Py<Document>, node: &Bound<'_, Node>) -> PyResult<Py<Document>> {
    let source = node.get().doc(node.py());
    if !source.get().home {
        return Err(PyValueError::new_err(
            "node is already in a document; detach it first (pop, remove, del) or insert node.copy()",
        ));
    }
    // A detached node's home holds only that node's subtree, so the target block is inside the node.
    if source.as_ptr() == target.as_ptr() {
        return Err(PyValueError::new_err("can't insert a node inside itself"));
    }
    Ok(source)
}

fn insert(py: Python<'_>, owner: &Owner, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
    let (target, parent) = owner.locate(py);
    let source = check_insertable(&target, node)?;
    let id = node.get().id;
    let moved = lock_both(py, target.get(), source.get(), |t, s| -> PyResult<Vec<NodeId>> {
        let s = s.expect("checked that the documents differ");
        let len = block_ids(&t.ids, parent)?.len();
        let index = if index < 0 {
            index.saturating_add_unsigned(len).max(0) as usize
        } else {
            (index as usize).min(len)
        };
        if s.ids.top() != [id] {
            return Err(stale());
        }
        let nested = match parent {
            None => None,
            Some(p) => {
                let parent_node = node_at(&t.kdl, &t.ids.path(p).expect("checked by block_ids"));
                Some((indent_of(leading(parent_node)).to_owned(), indent_unit(&t.kdl)))
            }
        };
        let moved = subtree_ids(&s.ids, id);
        let kdl_node = s.kdl.nodes_mut().remove(0);
        let subtree = s.ids.take_subtree(id);
        let nested = nested.as_ref().map(|(indent, unit)| (indent.as_str(), unit.as_str()));
        place(block_mut(&mut t.kdl, &t.ids, parent), index, kdl_node, nested);
        t.ids.put_subtree(parent, index, subtree);
        Ok(moved)
    })?;
    Document::move_handles(py, source.bind(py), target.bind(py), &moved)
}

struct Taken {
    node: KdlNode,
    subtree: Subtree,
    moved: Vec<NodeId>,
}

/// Takes the nodes `pick` chooses out of the block, each into a private document of its own, and returns their
/// handles. `close` also removes the block itself.
fn take_out<'py>(
    py: Python<'py>,
    owner: &Owner,
    pick: impl FnOnce(&[NodeId]) -> PyResult<Vec<NodeId>> + Send,
    close: bool,
) -> PyResult<Vec<Bound<'py, Node>>> {
    let (doc, parent) = owner.locate(py);
    let taken = doc.get().with_state(py, |s| -> PyResult<Vec<Taken>> {
        let picked = pick(block_ids(&s.ids, parent)?)?;
        let mut taken = Vec::with_capacity(picked.len());
        for id in picked {
            let index = block_ids(&s.ids, parent)?
                .iter()
                .position(|&n| n == id)
                .ok_or_else(stale)?;
            let moved = subtree_ids(&s.ids, id);
            let node = unplace(block_mut(&mut s.kdl, &s.ids, parent), index, parent.is_some());
            let subtree = s.ids.take_subtree(id);
            taken.push(Taken { node, subtree, moved });
        }
        if let (true, Some(p)) = (close, parent) {
            node_at_mut(&mut s.kdl, &s.ids.path(p).expect("checked by block_ids")).clear_children();
            s.ids.close_block(p);
        }
        Ok(taken)
    })?;

    let doc = doc.bind(py);
    taken
        .into_iter()
        .map(|t| {
            let root = t.subtree.root();
            let mut kdl = KdlDocument::new();
            kdl.nodes_mut().push(t.node);
            let ids = IdTree::from_subtree(t.subtree);
            let home = Bound::new(py, Document::with_ids(ids, kdl, doc.get().types.clone(), true))?;
            Document::move_handles(py, doc, &home, &t.moved)?;
            Document::node_handle(&home, root)
        })
        .collect()
}

fn take_all(ids: &[NodeId]) -> PyResult<Vec<NodeId>> {
    Ok(ids.to_vec())
}

/// `node.children = []`: an empty block, detaching any children it had.
pub(crate) fn open_block(owner: &Owner, py: Python<'_>) -> PyResult<()> {
    let (doc, parent) = owner.locate(py);
    let id = parent.expect("only nodes open blocks");
    let had_block = doc.get().with_state(py, |s| -> PyResult<bool> {
        let path = s.ids.path(id).ok_or_else(stale)?;
        let node = node_at_mut(&mut s.kdl, &path);
        if node.children().is_some() {
            return Ok(true);
        }
        let f = fmt_mut(node);
        if f.before_children.is_empty() {
            f.before_children = " ".into();
        }
        let mut block = KdlDocument::new();
        block.set_format(KdlDocumentFormat::default());
        node.set_children(block);
        s.ids.open_block(id);
        Ok(false)
    })?;
    if had_block {
        take_out(py, owner, take_all, false)?;
    }
    Ok(())
}

/// `node.children = None`: removes the block, detaching any children it had.
pub(crate) fn close_block(owner: &Owner, py: Python<'_>) -> PyResult<()> {
    let Owner::Node(node) = owner else {
        unreachable!("only nodes close blocks")
    };
    if node.get().with_node(py, |n| n.children().is_none())? {
        return Ok(());
    }
    take_out(py, owner, take_all, true)?;
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
    let (doc, parent) = owner.locate(py);
    let found = doc.get().with_state(py, |s| -> PyResult<Vec<NodeId>> {
        let ids = match block_ids(&s.ids, parent) {
            Ok(ids) => ids,
            Err(_) if parent.is_some_and(|p| s.ids.contains(p)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let block = match parent {
            None => &s.kdl,
            Some(p) => node_at(&s.kdl, &s.ids.path(p).expect("checked by block_ids"))
                .children()
                .expect("checked by block_ids"),
        };
        let matching = ids.iter().zip(block.nodes()).filter(|(_, n)| key.matches(n));
        Ok(matching
            .take(if first { 1 } else { usize::MAX })
            .map(|(&id, _)| id)
            .collect())
    })?;
    handles(py, &doc, &found)
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

    fn snapshot(&self, py: Python<'_>) -> PyResult<(Py<Document>, Vec<NodeId>)> {
        let (doc, parent) = self.owner.locate(py);
        let ids = doc
            .get()
            .with_state(py, |s| block_ids(&s.ids, parent).map(<[NodeId]>::to_vec))?;
        Ok((doc, ids))
    }
}

fn position(ids: &[NodeId], index: isize) -> PyResult<usize> {
    let i = if index < 0 {
        index.checked_add_unsigned(ids.len())
    } else {
        Some(index)
    };
    match i {
        Some(i) if i >= 0 && (i as usize) < ids.len() => Ok(i as usize),
        _ => Err(PyIndexError::new_err("children index out of range")),
    }
}

fn sliced(ids: &[NodeId], slice: &Bound<'_, PySlice>) -> PyResult<Vec<NodeId>> {
    let s = slice.indices(ids.len() as isize)?;
    Ok((0..s.slicelength)
        .map(|k| ids[(s.start + k as isize * s.step) as usize])
        .collect())
}

#[pymethods]
impl Children {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(self.snapshot(py)?.1.len())
    }

    fn __getitem__<'py>(&self, index: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = index.py();
        let (doc, ids) = self.snapshot(py)?;
        if let Ok(slice) = index.cast::<PySlice>() {
            return Ok(PyList::new(py, handles(py, &doc, &sliced(&ids, slice)?)?)?.into_any());
        }
        let id = ids[position(&ids, index.extract()?)?];
        Ok(Document::node_handle(doc.bind(py), id)?.into_any())
    }

    fn __setitem__(&self, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
        let py = node.py();
        check_insertable(&self.owner.locate(py).0, node)?;
        let (_, ids) = self.snapshot(py)?;
        let i = position(&ids, index)?;
        let id = ids[i];
        take_out(py, &self.owner, move |_| Ok(vec![id]), false)?;
        insert(py, &self.owner, i as isize, node)
    }

    fn __delitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = index.py();
        let (_, ids) = self.snapshot(py)?;
        let picked = match index.cast::<PySlice>() {
            Ok(slice) => sliced(&ids, slice)?,
            Err(_) => vec![ids[position(&ids, index.extract()?)?]],
        };
        take_out(py, &self.owner, move |_| Ok(picked), false)?;
        Ok(())
    }

    // Snapshots the handles first, so editing the list while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (doc, ids) = self.snapshot(py)?;
        Ok(PyList::new(py, handles(py, &doc, &ids)?)?.try_iter()?.into_any())
    }

    fn __contains__(&self, node: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Ok(node) = node.cast::<Node>() else {
            return Ok(false);
        };
        Ok(self.snapshot(node.py())?.1.contains(&node.get().id))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let (doc, ids) = self.snapshot(py)?;
        Ok(format!(
            "Children({})",
            PyList::new(py, handles(py, &doc, &ids)?)?.repr()?
        ))
    }

    fn index(&self, node: &Bound<'_, Node>) -> PyResult<usize> {
        let (_, ids) = self.snapshot(node.py())?;
        let id = find_id(&ids, node.get().id)?;
        Ok(ids.iter().position(|&n| n == id).expect("just found"))
    }

    fn insert(&self, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
        insert(node.py(), &self.owner, index, node)
    }

    fn append(&self, node: &Bound<'_, Node>) -> PyResult<()> {
        insert(node.py(), &self.owner, isize::MAX, node)
    }

    fn extend(&self, nodes: &Bound<'_, PyAny>) -> PyResult<()> {
        for node in nodes.try_iter()? {
            self.append(node?.cast::<Node>()?)?;
        }
        Ok(())
    }

    #[pyo3(signature = (index=-1))]
    fn pop<'py>(&self, py: Python<'py>, index: isize) -> PyResult<Bound<'py, Node>> {
        let mut taken = take_out(py, &self.owner, move |ids| Ok(vec![ids[position(ids, index)?]]), false)?;
        Ok(taken.pop().expect("took one node"))
    }

    fn remove(&self, node: &Bound<'_, Node>) -> PyResult<()> {
        let id = node.get().id;
        take_out(
            node.py(),
            &self.owner,
            move |ids| find_id(ids, id).map(|id| vec![id]),
            false,
        )?;
        Ok(())
    }

    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        take_out(py, &self.owner, take_all, false)?;
        Ok(())
    }
}

fn find_id(ids: &[NodeId], id: NodeId) -> PyResult<NodeId> {
    match ids.contains(&id) {
        true => Ok(id),
        false => Err(PyValueError::new_err("node is not in this list")),
    }
}
