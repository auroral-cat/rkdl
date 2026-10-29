use std::sync::Arc;

use kdl::{KdlEntry, KdlIdentifier, KdlNode, KdlValue};
use pyo3::exceptions::{PyIndexError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice, PyTuple, PyWeakrefReference};

use crate::document::{DocState, Document};
use crate::handles::{self, DocRef, Fail, Garbage, Home};
use crate::node::Node;
use crate::tree::{EntryId, IdTree, NodeId, node_at, node_at_mut};
use crate::values::{self, TypeRegistry};

fn removed() -> PyErr {
    PyRuntimeError::new_err("entry was removed")
}

// Reading and writing a KdlEntry.

/// Writes a value and the annotation `values::from_python` chose for it. Only `value_repr` is rewritten: the rest
/// of the format holds the entry's whitespace and any `/-` comment in front of it.
pub(crate) fn store(entry: &mut KdlEntry, value: KdlValue, ty: Option<String>) {
    let repr = values::canonical_repr(&value);
    entry.set_value(value);
    set_ty(entry, ty);
    if let Some(f) = entry.format_mut() {
        f.value_repr = repr;
    }
}

fn set_ty(entry: &mut KdlEntry, ty: Option<String>) {
    match ty {
        // The same annotation keeps its spelling.
        Some(ty) if entry.ty().is_some_and(|old| old.value() == ty) => {}
        Some(ty) => entry.set_ty(ty),
        None if entry.ty().is_some() => *entry = without_ty(entry),
        None => {}
    }
}

/// `entry` with its type annotation removed. KdlEntry can set an annotation but not remove one (`ty_mut` is an
/// `Option<&mut>`, unlike KdlNode's), so this rebuilds the entry around the same name, value, format and span. A
/// candidate for an upstream fix.
fn without_ty(entry: &KdlEntry) -> KdlEntry {
    let mut new = match entry.name() {
        Some(name) => KdlEntry::new_prop(name.clone(), entry.value().clone()),
        None => KdlEntry::new(entry.value().clone()),
    };
    if let Some(f) = entry.format() {
        let mut f = f.clone();
        // Whitespace inside and after `(...)` belongs to the annotation, and `after_ty` prints even without one.
        f.before_ty_name.clear();
        f.after_ty_name.clear();
        f.after_ty.clear();
        new.set_format(f);
    }
    new.set_span(entry.span());
    new
}

pub(crate) fn new_entry(key: Option<String>, value: KdlValue, ty: Option<String>) -> KdlEntry {
    let mut entry = match key {
        Some(key) => KdlEntry::new_prop(key, value),
        None => KdlEntry::new(value),
    };
    if let Some(ty) = ty {
        entry.set_ty(ty);
    }
    entry
}

/// Finds the entry under its document's lock. Moves repoint handles under that lock too, so if the handle still
/// points at `doc` and the entry isn't there, it was removed.
fn entry_mut<'a>(s: &'a mut DocState, id: EntryId, doc: &DocRef, home: &Home) -> Result<&'a mut KdlEntry, Fail> {
    let Some((node, index)) = s.ids.entry_position(id) else {
        return Err(match Arc::ptr_eq(doc, &home.doc()) {
            true => Fail::Err(removed()),
            false => Fail::Changed,
        });
    };
    let path = s.ids.path(node).expect("an entry's node is in the tree");
    Ok(&mut node_at_mut(&mut s.kdl, &path).entries_mut()[index])
}

// Snapshots and edits, shared by the views.

/// One entry as read under the lock, to work on once it's released.
#[derive(Clone)]
pub(crate) struct Item {
    pub(crate) id: EntryId,
    pub(crate) name: Option<KdlIdentifier>,
    pub(crate) value: KdlValue,
    pub(crate) ty: Option<String>,
}

impl Item {
    fn new(id: EntryId, entry: &KdlEntry) -> Self {
        Item {
            id,
            name: entry.name().cloned(),
            value: entry.value().clone(),
            ty: entry.ty().map(|t| t.value().to_owned()),
        }
    }

    pub(crate) fn key(&self) -> Option<&str> {
        self.name.as_ref().map(|n| n.value())
    }

    pub(crate) fn to_python(&self, py: Python<'_>, types: &TypeRegistry) -> PyResult<Py<PyAny>> {
        values::to_python(py, &self.value, self.ty.as_deref(), types)
    }

    // Values may change between planning an edit and applying it. Which entries there are, their keys and their
    // annotations may not, since the plan depends on them.
    fn same_shape(&self, other: &Item) -> bool {
        self.id == other.id && self.name == other.name && self.ty == other.ty
    }
}

fn items(s: &DocState, node: NodeId) -> Result<Vec<Item>, Fail> {
    let path = s.ids.path(node).ok_or(Fail::Gone)?;
    let entries = node_at(&s.kdl, &path).entries();
    Ok(s.ids
        .entries(node)
        .iter()
        .zip(entries)
        .map(|(&id, e)| Item::new(id, e))
        .collect())
}

/// A node's entries in file order, and the document they were read from.
pub(crate) struct Snapshot {
    pub(crate) doc: DocRef,
    pub(crate) items: Vec<Item>,
}

impl Snapshot {
    pub(crate) fn types(&self) -> &TypeRegistry {
        &self.doc.get().types
    }
}

pub(crate) fn read(py: Python<'_>, node: &Node) -> PyResult<Snapshot> {
    handles::on_home(&node.home, |doc| {
        let items = doc.get().with_state(py, |s| items(s, node.id))?;
        Ok(Snapshot {
            doc: doc.clone(),
            items,
        })
    })
}

/// A node's entries, open for editing. Edits go to the KdlNode and the IdTree together.
pub(crate) struct Target<'a> {
    kdl: &'a mut KdlNode,
    ids: &'a mut IdTree,
    node: NodeId,
}

impl Target<'_> {
    fn index(&self, id: EntryId) -> usize {
        self.ids.entry_position(id).expect("planned from this node").1
    }

    pub(crate) fn len(&self) -> usize {
        self.kdl.entries().len()
    }

    pub(crate) fn entry_mut(&mut self, id: EntryId) -> &mut KdlEntry {
        let index = self.index(id);
        &mut self.kdl.entries_mut()[index]
    }

    // Plans compute indexes from a snapshot with the same shape, so they're in bounds and KdlNode's panicking
    // insert/remove aren't needed.
    pub(crate) fn insert(&mut self, index: usize, entry: KdlEntry) {
        self.kdl.entries_mut().insert(index, entry);
        self.ids.insert_entry(self.node, index);
    }

    pub(crate) fn remove(&mut self, id: EntryId) {
        let index = self.index(id);
        self.kdl.entries_mut().remove(index);
        self.ids.remove_entry(id);
    }
}

/// Plans an edit from a snapshot of the node's entries, with Python available, then applies the plan under the
/// lock. If the entries changed shape in between, it plans again.
pub(crate) fn edit<P: Send, R: Send>(
    py: Python<'_>,
    node: &Node,
    mut plan: impl FnMut(&Snapshot) -> PyResult<P>,
    apply: impl Fn(Target<'_>, P) -> R + Sync,
) -> PyResult<R> {
    handles::on_home(&node.home, |doc| {
        let snapshot = Snapshot {
            doc: doc.clone(),
            items: doc.get().with_state(py, |s| items(s, node.id))?,
        };
        let planned = plan(&snapshot)?;
        // A plan that can't be applied goes back out unapplied: it may hold Python objects, which are dropped only
        // once the lock is released.
        let applied = doc.get().with_state(py, |s| {
            let now = match items(s, node.id) {
                Ok(now) => now,
                Err(fail) => return Err((fail, planned)),
            };
            let same =
                now.len() == snapshot.items.len() && now.iter().zip(&snapshot.items).all(|(a, b)| a.same_shape(b));
            if !same {
                return Err((Fail::Changed, planned));
            }
            let path = s.ids.path(node.id).expect("found by items");
            let target = Target {
                kdl: node_at_mut(&mut s.kdl, &path),
                ids: &mut s.ids,
                node: node.id,
            };
            Ok(apply(target, planned))
        });
        applied.map_err(|(fail, _)| fail)
    })
}

/// Handles for the entries `choose` picks. If one of them leaves the document before its handle exists, the node
/// is read again.
pub(crate) fn chosen<'py>(
    py: Python<'py>,
    node: &Node,
    choose: impl Fn(&[Item]) -> PyResult<Vec<EntryId>>,
) -> PyResult<Vec<Bound<'py, Entry>>> {
    'retry: loop {
        let snapshot = read(py, node)?;
        let doc = snapshot.doc.bind(py);
        let mut found = Vec::new();
        for id in choose(&snapshot.items)? {
            match Entry::handle(doc, id)? {
                Some(entry) => found.push(entry),
                None => continue 'retry,
            }
        }
        return Ok(found);
    }
}

/// Python's index rules: negative counts from the end. `what` names the sequence in the error.
pub(crate) fn position(len: usize, index: isize, what: &str) -> PyResult<usize> {
    let i = if index < 0 {
        index.checked_add_unsigned(len)
    } else {
        Some(index)
    };
    match i {
        Some(i) if i >= 0 && (i as usize) < len => Ok(i as usize),
        _ => Err(PyIndexError::new_err(format!("{what} index out of range"))),
    }
}

/// The indexes a slice picks from a sequence of `len` items.
pub(crate) fn sliced(len: usize, slice: &Bound<'_, PySlice>) -> PyResult<Vec<usize>> {
    let s = slice.indices(len as isize)?;
    Ok((0..s.slicelength)
        .map(|k| (s.start + k as isize * s.step) as usize)
        .collect())
}

// The handle.

/// One argument or property. The handle stays valid while its node moves, and dies when the entry is removed.
#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Entry {
    // Ids are global, so moving the node to another document only repoints `home`.
    id: EntryId,
    home: Arc<Home>,
}

impl Entry {
    /// The one handle for `id`, published the same way as `Document::node_handle`. None means the entry isn't in
    /// `doc` any more, and the caller should look again.
    pub(crate) fn handle<'py>(doc: &Bound<'py, Document>, id: EntryId) -> PyResult<Option<Bound<'py, Entry>>> {
        let py = doc.py();
        loop {
            if let Some(entry) = handles::get::<Entry>(py, id.key()) {
                return Ok(Some(entry));
            }
            let home = Home::new(Arc::new(doc.clone().unbind()));
            let entry = Bound::new(py, Entry { id, home: home.clone() })?;
            let weak = PyWeakrefReference::new(&entry)?.unbind();
            let (found, published, garbage) = doc.get().with_state(py, |s| {
                let mut garbage = Garbage::default();
                if s.ids.entry_position(id).is_none() {
                    garbage.weakrefs.push(weak);
                    return (false, false, garbage);
                }
                let published = handles::intern_locked(id.key(), weak, &home, &mut garbage);
                (true, published, garbage)
            });
            drop(garbage);
            match (found, published) {
                (false, _) => return Ok(None),
                (true, true) => return Ok(Some(entry)),
                (true, false) => py.detach(std::thread::yield_now),
            }
        }
    }

    /// Runs `f` on this entry under its document's lock.
    fn with_entry<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut KdlEntry) -> R + Send) -> PyResult<R> {
        let mut f = Some(f);
        handles::on_home(&self.home, |doc| {
            doc.get().with_state(py, |s| {
                let entry = entry_mut(s, self.id, doc, &self.home)?;
                let f = f.take().expect("runs once it finds the entry");
                Ok(f(entry))
            })
        })
    }

    fn item(&self, py: Python<'_>) -> PyResult<(Item, DocRef)> {
        handles::on_home(&self.home, |doc| {
            let item = doc.get().with_state(py, |s| {
                entry_mut(s, self.id, doc, &self.home).map(|e| Item::new(self.id, e))
            })?;
            Ok((item, doc.clone()))
        })
    }
}

#[pymethods]
impl Entry {
    /// None for an argument.
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_entry(py, |e| e.name().map(|n| n.value().to_owned()))
    }

    #[getter]
    fn value(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let (item, doc) = self.item(py)?;
        item.to_python(py, &doc.get().types)
    }

    #[setter]
    fn set_value(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = value.py();
        handles::on_home(&self.home, |doc| {
            let before = doc.get().with_state(py, |s| {
                entry_mut(s, self.id, doc, &self.home).map(|e| Item::new(self.id, e).ty)
            })?;
            // Converting may call Python, so it happens between two locks.
            let (value, ty) = values::from_python(value, before.as_deref(), &doc.get().types)?;
            doc.get().with_state(py, |s| {
                let entry = entry_mut(s, self.id, doc, &self.home)?;
                if entry.ty().map(|t| t.value()) != before.as_deref() {
                    return Err(Fail::Changed);
                }
                store(entry, value, ty);
                Ok(())
            })
        })
    }

    /// Setting it doesn't convert the value, so reading `value` afterwards raises if the contents don't fit.
    #[getter]
    fn r#type(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_entry(py, |e| e.ty().map(|t| t.value().to_owned()))
    }

    #[setter]
    fn set_type(&self, py: Python<'_>, ty: Option<String>) -> PyResult<()> {
        self.with_entry(py, |e| set_ty(e, ty))
    }

    /// `name, value = entry`.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let (item, doc) = self.item(py)?;
        let value = item.to_python(py, &doc.get().types)?;
        Ok(
            PyTuple::new(py, [item.key().into_pyobject(py)?.into_any().unbind(), value])?
                .try_iter()?
                .into_any(),
        )
    }

    fn __str__(&self, py: Python<'_>) -> PyResult<String> {
        self.with_entry(py, |e| e.to_string().trim().to_owned())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let (item, doc) = self.item(py)?;
        // A value whose contents don't fit its type still gets a repr, in KDL.
        let value = match item.to_python(py, &doc.get().types) {
            Ok(value) => value.bind(py).repr()?.to_string(),
            Err(_) => values::canonical_repr(&item.value),
        };
        let ty = item.ty.as_deref().map(|t| format!("({t})")).unwrap_or_default();
        Ok(match item.key() {
            Some(key) => format!("<Entry {key}={ty}{value}>"),
            None => format!("<Entry {ty}{value}>"),
        })
    }

    fn __eq__(&self, other: PyRef<'_, Entry>) -> bool {
        self.id == other.id
    }

    fn __hash__(&self) -> u64 {
        self.id.key()
    }
}

// node.entries

/// A live, read-only list of a node's entries in file order.
#[pyclass(module = "rkdl", frozen)]
pub struct Entries {
    node: Py<Node>,
}

impl Entries {
    pub(crate) fn new(node: Py<Node>) -> Self {
        Entries { node }
    }
}

fn ids(items: &[Item]) -> Vec<EntryId> {
    items.iter().map(|i| i.id).collect()
}

#[pymethods]
impl Entries {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(read(py, self.node.get())?.items.len())
    }

    fn __getitem__<'py>(&self, index: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = index.py();
        let node = self.node.get();
        if let Ok(slice) = index.cast::<PySlice>() {
            let found = chosen(py, node, |items| {
                Ok(sliced(items.len(), slice)?.into_iter().map(|i| items[i].id).collect())
            })?;
            return Ok(PyList::new(py, found)?.into_any());
        }
        let index: isize = index.extract()?;
        let mut found = chosen(py, node, |items| {
            Ok(vec![items[position(items.len(), index, "entries")?].id])
        })?;
        Ok(found.pop().expect("chose one entry").into_any())
    }

    // Snapshots the handles first, so editing the node while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let found = chosen(py, self.node.get(), |items| Ok(ids(items)))?;
        Ok(PyList::new(py, found)?.try_iter()?.into_any())
    }

    fn __reversed__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut found = chosen(py, self.node.get(), |items| Ok(ids(items)))?;
        found.reverse();
        Ok(PyList::new(py, found)?.try_iter()?.into_any())
    }

    fn __contains__(&self, entry: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Ok(entry) = entry.cast::<Entry>() else {
            return Ok(false);
        };
        let items = read(entry.py(), self.node.get())?.items;
        Ok(items.iter().any(|i| i.id == entry.get().id))
    }

    fn index(&self, entry: &Bound<'_, Entry>) -> PyResult<usize> {
        let items = read(entry.py(), self.node.get())?.items;
        items
            .iter()
            .position(|i| i.id == entry.get().id)
            .ok_or_else(|| PyValueError::new_err("entry is not in this list"))
    }

    fn count(&self, entry: &Bound<'_, PyAny>) -> PyResult<usize> {
        Ok(usize::from(self.__contains__(entry)?))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let found = chosen(py, self.node.get(), |items| Ok(ids(items)))?;
        Ok(format!("Entries({})", PyList::new(py, found)?.repr()?))
    }
}
