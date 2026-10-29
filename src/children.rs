use std::sync::Arc;

use kdl::{KdlDocument, KdlDocumentFormat, KdlNode, KdlNodeFormat};
use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::document::{DocState, Document, lock_both};
use crate::handles::{self, DocRef, Fail, Garbage, Home, stale};
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
    fn on_doc<R>(&self, py: Python<'_>, mut op: impl FnMut(&DocRef, Option<NodeId>) -> Result<R, Fail>) -> PyResult<R> {
        match self {
            Owner::Doc(doc) => handles::on_home(&Home::new(Arc::new(doc.clone_ref(py))), |doc| op(doc, None)),
            Owner::Node(node) => {
                let node = node.get();
                handles::on_home(&node.home, |doc| op(doc, Some(node.id)))
            }
        }
    }

    /// Reads the owner's document under its lock.
    fn read<R: Send>(
        &self,
        py: Python<'_>,
        f: impl Fn(&DocState, Option<NodeId>) -> Result<R, Fail> + Sync,
    ) -> PyResult<(DocRef, R)> {
        self.on_doc(py, |doc, parent| {
            let result = doc.get().with_state(py, |s| f(s, parent))?;
            Ok((doc.clone(), result))
        })
    }

    fn ids(&self, py: Python<'_>) -> PyResult<(DocRef, Vec<NodeId>)> {
        self.read(py, |s, parent| Ok(block_ids(&s.ids, parent)?.to_vec()))
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
            if let Some(found) = handles_for(py, &doc, &choose(&ids)?)? {
                return Ok(found);
            }
        }
    }
}

fn no_block() -> PyErr {
    PyValueError::new_err("node has no children block")
}

fn block_ids(ids: &IdTree, parent: Option<NodeId>) -> Result<&[NodeId], Fail> {
    match parent {
        None => Ok(ids.top()),
        Some(p) if !ids.contains(p) => Err(Fail::Gone),
        Some(p) => ids.children(p).ok_or_else(|| Fail::Err(no_block())),
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

/// Cache keys for every node and entry under `root`, whose handles move with it.
fn subtree_keys(ids: &IdTree, root: NodeId) -> Vec<u64> {
    let mut keys = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        keys.push(id.key());
        keys.extend(ids.entries(id).iter().map(|e| e.key()));
        stack.extend(ids.children(id).into_iter().flatten());
    }
    keys
}

/// None if one of the nodes has left `doc` since the ids were read.
fn handles_for<'py>(py: Python<'py>, doc: &DocRef, ids: &[NodeId]) -> PyResult<Option<Vec<Bound<'py, Node>>>> {
    let doc = doc.bind(py);
    let mut found = Vec::with_capacity(ids.len());
    for &id in ids {
        match Document::node_handle(doc, id)? {
            Some(node) => found.push(node),
            None => return Ok(None),
        }
    }
    Ok(Some(found))
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

/// Whether `text` ends in a `\` line continuation that still needs its newline.
fn ends_in_open_escline(text: &str) -> bool {
    text.trim_end_matches(|c: char| c.is_whitespace() && c != '\n' && c != '\r')
        .ends_with('\\')
}

/// Whether `text` ends in a line break, give or take spaces after it.
fn ends_line(text: &str) -> bool {
    text.trim_end_matches(|c: char| c.is_whitespace() && c != '\n')
        .ends_with('\n')
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
        // A document's leading text can end in a comment with no line break, and a `// comment` would swallow the
        // new node.
        None if len == 0 => {
            let leading = block.format().map_or("", |f| &f.leading);
            (String::new(), !is_blank(leading) && !ends_line(leading))
        }
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

    let mut newline = newline;
    if index > 0 {
        let prev = &mut block.nodes_mut()[index - 1];
        let ends_in_escline = ends_in_open_escline(&prev.to_string());
        let f = fmt_mut(prev);
        if f.terminator.is_empty() && ends_in_escline {
            // A newline straight after the `\` would continue this node onto the new one's line. Finish the escline
            // and end the node with `;` on its own line, which keeps the old text as it was.
            f.before_terminator.push('\n');
            f.terminator = ";".into();
        } else if f.terminator.is_empty() {
            // The last node of a block or file may have no terminator at all.
            f.terminator = "\n".into();
        }
        // A `;` (or the `;` above) doesn't end the line, so the new node starts one of its own.
        if !format!("{}{}", f.terminator, f.trailing).contains('\n') {
            newline = true;
            // A newline before `}` or the end of the file belonged to the old last node's line. The new node ends
            // that line now and brings its own newline, so drop this one or it leaves a blank line.
            if index == len
                && let Some(bf) = block.format_mut()
                && let Some(rest) = bf.trailing.strip_prefix('\n')
            {
                bf.trailing = rest.to_owned();
            }
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

fn insert(py: Python<'_>, owner: &Owner, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
    let id = node.get().id;
    let home = &node.get().home;
    let garbage = owner.on_doc(py, |target, parent| {
        let source = home.doc();
        check_insertable(target, &source)?;
        let new_home = Arc::new(target.clone_ref(py));
        lock_both(py, target.get(), source.get(), |t, s| -> Result<Garbage, Fail> {
            let s = s.expect("checked that the documents differ");
            let len = block_ids(&t.ids, parent)?.len();
            if s.ids.top() != [id] {
                // Another thread inserted it first; the retry will find it attached.
                return match Arc::ptr_eq(&source, &home.doc()) {
                    true => Err(Fail::Err(stale())),
                    false => Err(Fail::Changed),
                };
            }
            let index = if index < 0 {
                index.saturating_add_unsigned(len).max(0) as usize
            } else {
                (index as usize).min(len)
            };
            let nested = match parent {
                None => None,
                Some(p) => {
                    let parent_node = node_at(&t.kdl, &t.ids.path(p).expect("checked by block_ids"));
                    Some((indent_of(leading(parent_node)).to_owned(), indent_unit(&t.kdl)))
                }
            };
            let keys = subtree_keys(&s.ids, id);
            let kdl_node = s.kdl.nodes_mut().remove(0);
            let subtree = s.ids.take_subtree(id);
            let nested = nested.as_ref().map(|(indent, unit)| (indent.as_str(), unit.as_str()));
            place(block_mut(&mut t.kdl, &t.ids, parent), index, kdl_node, nested);
            t.ids.put_subtree(parent, index, subtree);
            let mut garbage = Garbage::default();
            handles::repoint(keys, &new_home, &mut garbage);
            Ok(garbage)
        })
    })?;
    drop(garbage);
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
        let snapshot = doc.get().with_state(py, |s| match block_ids(&s.ids, parent) {
            Ok(ids) => Ok::<_, Fail>(Some(ids.to_vec())),
            Err(Fail::Err(_)) if close => Ok(None),
            Err(fail) => Err(fail),
        })?;
        let Some(snapshot) = snapshot else {
            return Ok(Vec::new());
        };
        let picked = pick(&snapshot)?;
        // The new homes are made up front, so the edit itself needs no Python.
        let homes = picked
            .iter()
            .map(|_| {
                let home = Py::new(py, Document::empty_home(doc.get().types.clone()))?;
                Ok((home.clone_ref(py), Arc::new(home)))
            })
            .collect::<PyResult<Vec<_>>>()?;

        let garbage = doc.get().with_state(py, |s| -> Result<Garbage, Fail> {
            match block_ids(&s.ids, parent) {
                Ok(ids) if *ids == snapshot[..] => {}
                Ok(_) => return Err(Fail::Changed),
                Err(Fail::Err(_)) if close => return Err(Fail::Changed),
                Err(fail) => return Err(fail),
            }
            let mut garbage = Garbage::default();
            for (&id, (home, home_ref)) in picked.iter().zip(&homes) {
                let index = block_ids(&s.ids, parent)?
                    .iter()
                    .position(|&n| n == id)
                    .expect("picked from this block");
                let keys = subtree_keys(&s.ids, id);
                let node = unplace(block_mut(&mut s.kdl, &s.ids, parent), index, parent.is_some());
                let subtree = s.ids.take_subtree(id);
                let mut kdl = KdlDocument::new();
                kdl.nodes_mut().push(node);
                home.get().fill(kdl, IdTree::from_subtree(subtree));
                handles::repoint(keys, home_ref, &mut garbage);
            }
            if let (true, Some(p)) = (close, parent) {
                node_at_mut(&mut s.kdl, &s.ids.path(p).expect("checked by block_ids")).clear_children();
                s.ids.close_block(p);
            }
            Ok(garbage)
        })?;
        drop(garbage);
        Ok(picked
            .into_iter()
            .zip(homes)
            .map(|(id, (home, _))| (id, home))
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
pub(crate) fn open_block(owner: &Owner, py: Python<'_>) -> PyResult<()> {
    let had_block = owner.on_doc(py, |doc, parent| {
        let id = parent.expect("only nodes open blocks");
        doc.get().with_state(py, |s| -> Result<bool, Fail> {
            let path = s.ids.path(id).ok_or(Fail::Gone)?;
            let node = node_at_mut(&mut s.kdl, &path);
            if node.children().is_some() {
                return Ok(true);
            }
            let f = fmt_mut(node);
            // Without a block, trailing slashdashed entries sit in before_terminator, which prints after the block.
            // They have to stay in front of it.
            if !is_blank(&f.before_terminator) {
                let mut moved = std::mem::take(&mut f.before_terminator);
                // Autoformat ends before_terminator with a newline. That line break goes after the block, not before
                // `{`, where it would end the node early.
                if ends_line(&moved) {
                    moved.truncate(moved.trim_end().len());
                    if f.terminator.is_empty() {
                        f.terminator = "\n".into();
                    }
                }
                if ends_in_open_escline(&moved) {
                    moved.push('\n');
                }
                f.before_children = moved + &f.before_children;
            }
            if !f.before_children.ends_with(char::is_whitespace) {
                f.before_children.push(' ');
            }
            let mut block = KdlDocument::new();
            block.set_format(KdlDocumentFormat::default());
            node.set_children(block);
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
pub(crate) fn close_block(owner: &Owner, py: Python<'_>) -> PyResult<()> {
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
    loop {
        let (doc, found) = owner.read(py, |s, parent| {
            let ids = match block_ids(&s.ids, parent) {
                Ok(ids) => ids,
                // No block means no children to match.
                Err(Fail::Err(_)) => return Ok(Vec::new()),
                Err(fail) => return Err(fail),
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
        if let Some(found) = handles_for(py, &doc, &found)? {
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

fn find_id(ids: &[NodeId], id: NodeId) -> PyResult<usize> {
    ids.iter()
        .position(|&n| n == id)
        .ok_or_else(|| PyValueError::new_err("node is not in this list"))
}

#[pymethods]
impl Children {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(self.owner.read(py, |s, parent| Ok(block_ids(&s.ids, parent)?.len()))?.1)
    }

    fn __getitem__<'py>(&self, index: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = index.py();
        if let Ok(slice) = index.cast::<PySlice>() {
            let found = self.owner.chosen(py, |ids| sliced(ids, slice))?;
            return Ok(PyList::new(py, found)?.into_any());
        }
        let index: isize = index.extract()?;
        let mut found = self.owner.chosen(py, |ids| Ok(vec![ids[position(ids, index)?]]))?;
        Ok(found.pop().expect("chose one node").into_any())
    }

    fn __setitem__(&self, index: isize, node: &Bound<'_, Node>) -> PyResult<()> {
        let py = node.py();
        let (doc, ids) = self.owner.ids(py)?;
        // Checked first so a bad node doesn't cost the old one its place.
        check_insertable(&doc, &node.get().home.doc())?;
        let i = position(&ids, index)?;
        take_out(py, &self.owner, |ids| Ok(vec![ids[position(ids, index)?]]), false)?;
        insert(py, &self.owner, i as isize, node)
    }

    fn __delitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = index.py();
        match index.cast::<PySlice>() {
            Ok(slice) => take_out(py, &self.owner, |ids| sliced(ids, slice), false)?,
            Err(_) => {
                let index: isize = index.extract()?;
                take_out(py, &self.owner, |ids| Ok(vec![ids[position(ids, index)?]]), false)?
            }
        };
        Ok(())
    }

    // Snapshots the handles first, so editing the list while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let found = self.owner.chosen(py, take_all)?;
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

    fn index(&self, node: &Bound<'_, Node>) -> PyResult<usize> {
        find_id(&self.owner.ids(node.py())?.1, node.get().id)
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
        let mut taken = take_out(py, &self.owner, |ids| Ok(vec![ids[position(ids, index)?]]), false)?;
        Ok(taken.pop().expect("took one node"))
    }

    fn remove(&self, node: &Bound<'_, Node>) -> PyResult<()> {
        let id = node.get().id;
        take_out(node.py(), &self.owner, |ids| Ok(vec![ids[find_id(ids, id)?]]), false)?;
        Ok(())
    }

    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        take_out(py, &self.owner, take_all, false)?;
        Ok(())
    }
}
