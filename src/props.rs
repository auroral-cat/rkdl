use pyo3::PyTraverseError;
use pyo3::exceptions::{PyKeyError, PyTypeError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::edit::{Item, Snapshot, chosen, edit, read};
use crate::entry::{Entry, new_entry, store};
use crate::node::Node;
use crate::values;

/// The entry `props[key]` reads, writes and deletes: the last one with that key, which is `KdlNode::entry`'s rule.
/// Deleting doesn't use `KdlNode::remove`, which takes the first match and compares key spellings, so it never finds a
/// parsed key (the parser records every key's spelling).
fn last<'a>(items: &'a [Item], key: &str) -> Option<&'a Item> {
    items.iter().rev().find(|i| i.key() == Some(key))
}

/// Each key once, in order of first appearance.
fn keys(items: &[Item]) -> Vec<&str> {
    let mut keys: Vec<&str> = Vec::new();
    for key in items.iter().filter_map(Item::key) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

fn get(py: Python<'_>, snapshot: &Snapshot, key: &str) -> PyResult<Option<Py<PyAny>>> {
    last(&snapshot.items, key)
        .map(|item| item.to_python(py, snapshot.types()))
        .transpose()
}

fn key_error(key: &str) -> PyErr {
    PyKeyError::new_err(key.to_owned())
}

/// Keys are str. Anything else is simply not there.
fn str_key(key: &Bound<'_, PyAny>) -> Option<String> {
    key.extract().ok()
}

/// A live dict of a node's properties, as plain Python values.
#[pyclass(module = "rkdl", frozen)]
pub struct Props {
    node: Py<Node>,
}

impl Props {
    pub(crate) fn new(node: Py<Node>) -> Self {
        Props { node }
    }

    fn set(&self, key: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        edit(
            value.py(),
            self.node.get(),
            |snapshot| {
                let found = last(&snapshot.items, key);
                let (value, ty) = values::from_python(value, found.and_then(|i| i.ty.as_deref()), snapshot.types())?;
                Ok((found.map(|i| i.id), value, ty))
            },
            |mut target, (found, value, ty)| {
                if let Some(id) = found {
                    store(target.entry_mut(id), value, ty);
                } else {
                    let end = target.len();
                    target.insert(end, new_entry(Some(key.to_owned()), value, ty));
                }
            },
        )
    }

    /// A plain dict of the properties.
    fn dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let snapshot = read(py, self.node.get())?;
        let dict = PyDict::new(py);
        for key in keys(&snapshot.items) {
            dict.set_item(key, get(py, &snapshot, key)?)?;
        }
        Ok(dict)
    }

    /// Removes the entry `del` picks, and returns the value `[]` reads, as `MutableMapping.pop` would.
    fn take(&self, py: Python<'_>, key: &str) -> PyResult<Option<Py<PyAny>>> {
        edit(
            py,
            self.node.get(),
            |snapshot| {
                last(&snapshot.items, key)
                    .map(|item| Ok((item.id, item.to_python(py, snapshot.types())?)))
                    .transpose()
            },
            |mut target, planned| {
                planned.map(|(id, value)| {
                    target.remove(id);
                    value
                })
            },
        )
    }
}

#[pymethods]
impl Props {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(keys(&read(py, self.node.get())?.items).len())
    }

    fn __getitem__(&self, key: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = key.py();
        let found = match str_key(key) {
            Some(k) => get(py, &read(py, self.node.get())?, &k)?,
            None => None,
        };
        found.ok_or_else(|| PyKeyError::new_err(key.clone().unbind()))
    }

    /// Writes the entry `[]` reads, or adds a property at the end.
    fn __setitem__(&self, key: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let key = str_key(key).ok_or_else(|| PyTypeError::new_err("property keys must be str"))?;
        self.set(&key, value)
    }

    fn __delitem__(&self, key: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = key.py();
        let Some(k) = str_key(key) else {
            return Err(PyKeyError::new_err(key.clone().unbind()));
        };
        edit(
            py,
            self.node.get(),
            |snapshot| last(&snapshot.items, &k).map(|i| i.id).ok_or_else(|| key_error(&k)),
            |mut target, id| target.remove(id),
        )
    }

    fn __contains__(&self, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        let Some(k) = str_key(key) else {
            return Ok(false);
        };
        Ok(last(&read(key.py(), self.node.get())?.items, &k).is_some())
    }

    // Snapshots the keys first, so editing the node while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let snapshot = read(py, self.node.get())?;
        Ok(PyList::new(py, keys(&snapshot.items))?.try_iter()?.into_any())
    }

    #[pyo3(signature = (key, default=None))]
    fn get(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        let found = match str_key(key) {
            Some(k) => get(py, &read(py, self.node.get())?, &k)?,
            None => None,
        };
        Ok(found.or(default).unwrap_or_else(|| py.None()))
    }

    #[pyo3(signature = (key, *default))]
    fn pop(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: Vec<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        if default.len() > 1 {
            return Err(PyTypeError::new_err("pop expected at most 2 arguments"));
        }
        let found = match str_key(key) {
            Some(k) => self.take(py, &k)?,
            None => None,
        };
        found
            .or_else(|| default.into_iter().next())
            .ok_or_else(|| PyKeyError::new_err(key.clone().unbind()))
    }

    #[pyo3(signature = (key, default=None))]
    fn setdefault(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        if let Ok(found) = self.__getitem__(key) {
            return Ok(found);
        }
        let default = default.unwrap_or_else(|| py.None());
        self.__setitem__(key, default.bind(py))?;
        Ok(default)
    }

    #[pyo3(signature = (other=None, **kwargs))]
    fn update(&self, other: Option<&Bound<'_, PyAny>>, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<()> {
        if let Some(other) = other {
            if other.hasattr("keys")? {
                for key in other.call_method0("keys")?.try_iter()? {
                    let key = key?;
                    self.__setitem__(&key, &other.get_item(&key)?)?;
                }
            } else {
                for pair in other.try_iter()? {
                    let (key, value): (Bound<'_, PyAny>, Bound<'_, PyAny>) = pair?.extract()?;
                    self.__setitem__(&key, &value)?;
                }
            }
        }
        for (key, value) in kwargs.into_iter().flatten() {
            self.__setitem__(&key, &value)?;
        }
        Ok(())
    }

    /// Removes every property. Not through `KdlNode::remove`, which can't find parsed keys.
    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        edit(
            py,
            self.node.get(),
            |snapshot| {
                Ok(snapshot
                    .items
                    .iter()
                    .filter(|i| i.name.is_some())
                    .map(|i| i.id)
                    .collect::<Vec<_>>())
            },
            |mut target, ids| ids.into_iter().for_each(|id| target.remove(id)),
        )
    }

    /// Removes and returns the last property, as `dict.popitem` does. With duplicate keys that's the entry `[]`
    /// reads, and an earlier one with the same key stays.
    fn popitem(&self, py: Python<'_>) -> PyResult<(String, Py<PyAny>)> {
        edit(
            py,
            self.node.get(),
            |snapshot| {
                let item = snapshot
                    .items
                    .iter()
                    .rev()
                    .find(|i| i.name.is_some())
                    .ok_or_else(|| PyKeyError::new_err("popitem(): props is empty"))?;
                let key = item.key().expect("a property").to_owned();
                Ok((item.id, key, item.to_python(py, snapshot.types())?))
            },
            |mut target, (id, key, value)| {
                target.remove(id);
                (key, value)
            },
        )
    }

    /// `Mapping`'s equality: the same keys with equal values, compared as dicts.
    fn __eq__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        let mapping = py.import("collections.abc")?.getattr("Mapping")?;
        if !other.is_instance(&mapping)? {
            return Ok(py.NotImplemented().into_bound(py));
        }
        let theirs = PyDict::new(py);
        for pair in other.call_method0("items")?.try_iter()? {
            let (key, value): (Bound<'_, PyAny>, Bound<'_, PyAny>) = pair?.extract()?;
            theirs.set_item(key, value)?;
        }
        let same = self.dict(py)?.eq(theirs)?;
        Ok(same.into_pyobject(py)?.to_owned().into_any())
    }

    fn keys<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        abc_view(slf, "KeysView")
    }

    fn values<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        abc_view(slf, "ValuesView")
    }

    fn items<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        abc_view(slf, "ItemsView")
    }

    /// The handle for the entry `[key]` reads.
    fn entry<'py>(&self, py: Python<'py>, key: &str) -> PyResult<Bound<'py, Entry>> {
        let mut found = chosen(py, self.node.get(), |items| {
            Ok(vec![last(items, key).ok_or_else(|| key_error(key))?.id])
        })?;
        Ok(found.pop().expect("chose one entry"))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("Props({})", self.dict(py)?.repr()?))
    }

    // No `__clear__`: a cycle through the node passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.node)
    }
}

// The standard views are live and only need the mapping methods above.
fn abc_view<'py>(props: &Bound<'py, Props>, view: &str) -> PyResult<Bound<'py, PyAny>> {
    let py = props.py();
    py.import("collections.abc")?.getattr(view)?.call1((props,))
}
