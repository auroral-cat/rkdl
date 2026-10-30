use std::sync::Arc;

use kdl::{KdlEntry, KdlIdentifier, KdlValue};
use pyo3::PyTraverseError;
use pyo3::exceptions::PyRuntimeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice, PyTuple};

use crate::document::{DocState, Document};
use crate::edit::{Item, chosen, read};
use crate::handles::{self, Fail, Home};
use crate::index::{not_in, pick_one, pick_slice};
use crate::node::Node;
use crate::tree::{EntryId, node_at_mut};
use crate::values;

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
    values::set_value_repr(entry, repr);
}

fn set_ty(entry: &mut KdlEntry, ty: Option<String>) {
    match ty {
        // The same annotation keeps its spelling.
        Some(ty) if entry.ty().is_some_and(|old| old.value() == ty) => {}
        Some(ty) => entry.set_ty(values::identifier(ty)),
        None if entry.ty().is_some() => *entry = without_ty(entry),
        None => {}
    }
}

/// `entry` with its type annotation removed. `KdlEntry` can set an annotation but not remove one (`ty_mut` is an
/// `Option<&mut>`, unlike `KdlNode`'s), so this rebuilds the entry around the same name, value, format and span. A
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

/// What an entry holds apart from its key and the whitespace around it: the value, its annotation, and both as
/// spelled in the file.
pub(crate) struct Contents {
    value: KdlValue,
    ty: Option<KdlIdentifier>,
    repr: String,
}

pub(crate) fn contents(entry: &KdlEntry) -> Contents {
    let repr = match entry.format() {
        Some(f) => f.value_repr.clone(),
        None => values::canonical_repr(entry.value()),
    };
    Contents {
        value: entry.value().clone(),
        ty: entry.ty().cloned(),
        repr,
    }
}

/// Moves `contents` into `entry`, spellings included.
pub(crate) fn put_contents(entry: &mut KdlEntry, contents: Contents) {
    entry.set_value(contents.value);
    match contents.ty {
        Some(ty) => entry.set_ty(ty),
        None if entry.ty().is_some() => *entry = without_ty(entry),
        None => {}
    }
    values::set_value_repr(entry, contents.repr);
}

pub(crate) fn new_entry(key: Option<String>, value: KdlValue, ty: Option<String>) -> KdlEntry {
    let mut entry = match key {
        Some(key) => KdlEntry::new_prop(values::identifier(key), KdlValue::Null),
        None => KdlEntry::new(KdlValue::Null),
    };
    store(&mut entry, value, ty);
    entry
}

/// Finds the entry under its document's lock. If it isn't there and the handle didn't move meanwhile, it was
/// removed.
fn entry_mut(s: &mut DocState, id: EntryId) -> Result<&mut KdlEntry, Fail> {
    let Some((node, index)) = s.ids.entry_position(id) else {
        return Err(Fail::Missing(removed()));
    };
    let path = s.ids.path(node).expect("an entry's node is in the tree");
    Ok(&mut node_at_mut(&mut s.kdl, &path).entries_mut()[index])
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
    /// The one handle for `id`. See `handles::publish`.
    pub(crate) fn handle<'py>(doc: &Bound<'py, Document>, id: EntryId) -> PyResult<Option<Bound<'py, Entry>>> {
        handles::publish(
            doc,
            id.key(),
            |home| Entry { id, home },
            |s| s.ids.entry_position(id).is_some(),
        )
    }

    /// Runs `f` on this entry under its document's lock.
    fn with_entry<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut KdlEntry) -> R + Send) -> PyResult<R> {
        let mut f = Some(f);
        handles::on_home(py, &self.home, |doc| {
            doc.get().with_state(py, |s| {
                let entry = entry_mut(s, self.id)?;
                let f = f.take().expect("runs once it finds the entry");
                Ok(f(entry))
            })
        })
    }

    fn item(&self, py: Python<'_>) -> PyResult<(Item, Py<Document>)> {
        handles::on_home(py, &self.home, |doc| {
            let item = doc
                .get()
                .with_state(py, |s| entry_mut(s, self.id).map(|e| Item::new(self.id, e)))?;
            Ok((item, doc.clone_ref(py)))
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
        item.to_python(py, doc.get().types.get())
    }

    #[setter]
    fn set_value(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = value.py();
        handles::on_home(py, &self.home, |doc| {
            let before = doc
                .get()
                .with_state(py, |s| entry_mut(s, self.id).map(|e| Item::new(self.id, e).ty))?;
            // Converting may call Python, so it happens between two locks.
            let (value, ty) = values::from_python(value, before.as_deref(), doc.get().types.get())?;
            doc.get().with_state(py, |s| {
                let entry = entry_mut(s, self.id)?;
                if entry.ty().map(KdlIdentifier::value) != before.as_deref() {
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
        let value = item.to_python(py, doc.get().types.get())?;
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
        let value = match item.to_python(py, doc.get().types.get()) {
            Ok(value) => value.bind(py).repr()?.to_string(),
            Err(_) => values::canonical_repr(&item.value),
        };
        let ty = item
            .ty
            .as_deref()
            .map(|t| format!("({})", values::identifier(t)))
            .unwrap_or_default();
        Ok(match item.key() {
            Some(key) => format!("<Entry {}={ty}{value}>", values::identifier(key)),
            None => format!("<Entry {ty}{value}>"),
        })
    }

    #[expect(clippy::needless_pass_by_value, reason = "PyO3 extracts arguments as PyRef by value")]
    fn __eq__(&self, other: PyRef<'_, Entry>) -> bool {
        self.id == other.id
    }

    fn __hash__(&self) -> u64 {
        self.id.key()
    }

    // No `__clear__`, as for Node: any cycle through the home passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.home.traverse(&visit)
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
                Ok(pick_slice(items, slice)?.into_iter().map(|i| i.id).collect())
            })?;
            return Ok(PyList::new(py, found)?.into_any());
        }
        let index: isize = index.extract()?;
        let mut found = chosen(py, node, |items| Ok(vec![pick_one(items, index, "entries")?.id]))?;
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
            .ok_or_else(|| not_in(entry.as_any(), "entries"))
    }

    fn count(&self, entry: &Bound<'_, PyAny>) -> PyResult<usize> {
        Ok(usize::from(self.__contains__(entry)?))
    }

    /// Equal to an `Entries` or a list holding the same entries in the same order. Like a list, it isn't equal to
    /// a tuple.
    fn __eq__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        let theirs: Vec<Option<EntryId>> = if let Ok(other) = other.cast::<Entries>() {
            read(py, other.get().node.get())?
                .items
                .iter()
                .map(|i| Some(i.id))
                .collect()
        } else if let Ok(other) = other.cast::<PyList>() {
            other
                .iter()
                .map(|e| e.cast::<Entry>().ok().map(|e| e.get().id))
                .collect()
        } else {
            return Ok(py.NotImplemented().into_bound(py));
        };
        let ours = read(py, self.node.get())?.items;
        let same = ours.len() == theirs.len() && ours.iter().zip(&theirs).all(|(a, b)| Some(a.id) == *b);
        Ok(same.into_pyobject(py)?.to_owned().into_any())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let found = chosen(py, self.node.get(), |items| Ok(ids(items)))?;
        Ok(format!("Entries({})", PyList::new(py, found)?.repr()?))
    }

    // No `__clear__`: a cycle through the node passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.node)
    }
}
