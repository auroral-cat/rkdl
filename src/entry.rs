use std::sync::Arc;

use kdl::{KdlEntry, KdlIdentifier, KdlValue};
use pyo3::PyTraverseError;
use pyo3::exceptions::PyValueError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice, PyTuple};

use crate::document::{DocState, Document};
use crate::edit::{Item, chosen, read};
use crate::handles::{self, Fail, Home, removed};
use crate::index::{Index, not_in, pick_one, pick_slice, search_range};
use crate::node::Node;
use crate::spelling;
use crate::tree::{EntryId, node_at_mut};
use crate::values::{self, Encoded};

// Reading and writing a KdlEntry.

/// Writes a value and the annotation `values::encode` chose for it. Writing what's already there changes nothing.
/// Otherwise only `value_repr` is rewritten: to the text the value came with, or in the style of the old text where
/// it can be. The rest of the format holds the entry's whitespace and any `/-` comment in front of it.
pub(crate) fn store(entry: &mut KdlEntry, new: Encoded) {
    let Encoded { value, ty, repr } = new;
    let old = entry.format().map(|f| f.value_repr.as_str());
    let same_text = repr.is_none() || repr.as_deref() == old;
    if same_text && values::same_value(entry.value(), &value) && entry.ty().map(KdlIdentifier::value) == ty.as_deref() {
        return;
    }
    let repr = repr
        .or_else(|| old.and_then(|old| spelling::value(old, &value)))
        .unwrap_or_else(|| values::canonical_repr(&value));
    entry.set_value(value);
    set_ty(entry, ty);
    values::set_value_repr(entry, repr);
}

fn set_ty(entry: &mut KdlEntry, ty: Option<String>) {
    match ty {
        // The same annotation keeps its spelling.
        Some(ty) if entry.ty().is_some_and(|old| old.value() == ty) => {}
        Some(ty) => entry.set_ty(spelling::identifier(entry.ty(), ty)),
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
    store(&mut entry, Encoded { value, ty, repr: None });
    entry
}

/// Finds the entry under its document's lock. If it isn't there and the handle didn't move meanwhile, it was
/// removed.
fn entry_mut(s: &mut DocState, id: EntryId) -> Result<&mut KdlEntry, Fail> {
    let (entries, index) = siblings_mut(s, id)?;
    Ok(&mut entries[index])
}

/// The entry's node's entries, and where the entry is among them, as `entry_mut` finds it.
fn siblings_mut(s: &mut DocState, id: EntryId) -> Result<(&mut Vec<KdlEntry>, usize), Fail> {
    let Some((node, index)) = s.ids.entry_position(id) else {
        return Err(Fail::Missing(removed()));
    };
    let path = s.ids.path(node).expect("an entry's node is in the tree");
    Ok((node_at_mut(&mut s.kdl, &path).entries_mut(), index))
}

// The handle.

/// One argument or property of a node, as a live handle.
///
/// `name, value = entry` unpacks it, and `str(entry)` is its KDL text. Get one from `node.entries`,
/// `node.args.entry(i)` or `node.props.entry(key)`. The handle keeps working while its node moves, and stops
/// working once the entry is removed.
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
    /// The property's key, or None for an argument.
    ///
    /// A property can be renamed, but an argument can't be given a name, nor a property lose its own.
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_entry(py, |e| e.name().map(|n| n.value().to_owned()))
    }

    // Renames a property, keeping its place and its value's text. An argument can't gain a name, nor a property
    // lose one, and a property can't take the key of another one: that would hide one of them from `node.props`.
    #[setter]
    fn set_name(&self, py: Python<'_>, name: Option<String>) -> PyResult<()> {
        let name = name.ok_or_else(|| {
            PyValueError::new_err("a property can't become an argument; remove it and add its value to node.args")
        })?;
        handles::on_home(py, &self.home, |doc| {
            doc.get().with_state(py, |s| {
                let (entries, index) = siblings_mut(s, self.id)?;
                let taken = entries.iter().any(|e| e.name().is_some_and(|k| k.value() == name));
                let entry = entries.get_mut(index).ok_or_else(|| Fail::Missing(removed()))?;
                let Some(old) = entry.name() else {
                    return Err(Fail::Err(PyValueError::new_err(
                        "an argument can't be given a name; remove it and add it to node.props",
                    )));
                };
                if old.value() == name {
                    return Ok(());
                }
                if taken {
                    return Err(Fail::Err(PyValueError::new_err(format!(
                        "the node already has a property {name:?}; remove it first, or set its value instead"
                    ))));
                }
                let new = spelling::identifier(Some(old), name.clone());
                entry.set_name(Some(new));
                Ok(())
            })
        })?;
        handles::edited();
        Ok(())
    }

    /// The value, converted by its type annotation.
    ///
    /// Writing a value keeps the annotation if the value fits it, and otherwise picks one from the value's class.
    ///
    /// Raises:
    ///     ValueError: The annotation is a reserved type the value can't be read as, or the new value is a number
    ///         that doesn't fit it, like 300 or 3.5 written over `(u8)`.
    ///     TypeError: The new value can't be converted to KDL.
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
                .with_state(py, |s| entry_mut(s, self.id).map(|e| Item::new(self.id, e)))?;
            // Converting may call Python, so it happens between two locks.
            let encoded = before.encode_over(value, doc.get().types.get())?;
            doc.get().with_state(py, |s| {
                let entry = entry_mut(s, self.id)?;
                if entry.ty().map(KdlIdentifier::value) != before.ty.as_deref() {
                    return Err(Fail::Changed);
                }
                store(entry, encoded);
                Ok(())
            })
        })?;
        handles::edited();
        Ok(())
    }

    /// The type annotation, or None. Setting None removes it.
    ///
    /// Setting it doesn't convert the value, so a type the value can't be read as raises `ValueError`.
    #[getter]
    fn r#type(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_entry(py, |e| e.ty().map(|t| t.value().to_owned()))
    }

    // Setting it doesn't convert the value, so it refuses a type the value doesn't read as.
    #[setter]
    fn set_type(&self, py: Python<'_>, ty: Option<&str>) -> PyResult<()> {
        handles::on_home(py, &self.home, |doc| {
            let value = doc
                .get()
                .with_state(py, |s| entry_mut(s, self.id).map(|e| e.value().clone()))?;
            // Checking may call a Type's decode, so it happens between two locks.
            if let Some(ty) = ty {
                values::check_fits(py, &value, ty, doc.get().types.get())?;
            }
            doc.get().with_state(py, |s| {
                let entry = entry_mut(s, self.id)?;
                if !values::same_value(entry.value(), &value) {
                    return Err(Fail::Changed);
                }
                set_ty(entry, ty.map(str::to_owned));
                Ok(())
            })
        })?;
        handles::edited();
        Ok(())
    }

    // `name, value = entry`.
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
        let Ok((item, doc)) = self.item(py) else {
            return Ok("<Entry (removed)>".to_owned());
        };
        let value = item.repr(py, doc.get().types.get(), true)?;
        Ok(match item.key() {
            Some(key) => format!("<Entry {}={value}>", values::identifier(key)),
            None => format!("<Entry {value}>"),
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

/// A node's arguments and properties as `Entry` objects, in the order of the text.
///
/// A live, read-only list.
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
        let Index(index) = index.extract()?;
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

    /// Returns the position of an entry.
    ///
    /// Raises:
    ///     ValueError: The entry isn't one of these.
    #[pyo3(
        signature = (entry, start=Index(0), stop=Index(isize::MAX)),
        text_signature = "($self, entry, start=0, stop=sys.maxsize)"
    )]
    fn index(&self, entry: &Bound<'_, PyAny>, start: Index, stop: Index) -> PyResult<usize> {
        let items = read(entry.py(), self.node.get())?.items;
        let range = search_range(items.len(), start, stop);
        let found = entry.cast::<Entry>().ok().and_then(|entry| {
            let id = entry.get().id;
            items.get(range.clone())?.iter().position(|i| i.id == id)
        });
        found.map(|i| range.start + i).ok_or_else(|| not_in(entry, "entries"))
    }

    /// Returns 1 if the entry is one of these, else 0.
    fn count(&self, entry: &Bound<'_, PyAny>) -> PyResult<usize> {
        Ok(usize::from(self.__contains__(entry)?))
    }

    // Equal to an `Entries` or a list holding the same entries in the same order. Like a list, it isn't equal to
    // a tuple.
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
