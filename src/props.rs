use pyo3::PyTraverseError;
use pyo3::exceptions::{PyKeyError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};

use crate::annotated::Value;
use crate::edit::{Item, Reads, Snapshot, chosen, edit, read};
use crate::entry::{Entry, new_entry, store};
use crate::node::Node;
use crate::tree::EntryId;
use crate::values::{self, Encoded, Place, Slot};

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

/// The pairs `dict.update(other, **kwargs)` would write, each key once, in the order a dict would hold them.
fn pairs<'py>(
    other: Option<&Bound<'py, PyAny>>,
    kwargs: Option<&Bound<'py, PyDict>>,
) -> PyResult<Vec<(String, Bound<'py, PyAny>)>> {
    let mut pairs: Vec<(String, Bound<'py, PyAny>)> = Vec::new();
    let mut add = |key: &Bound<'py, PyAny>, value: Bound<'py, PyAny>| -> PyResult<()> {
        let key = str_key(key).ok_or_else(|| PyTypeError::new_err("property keys must be str"))?;
        match pairs.iter_mut().find(|(k, _)| *k == key) {
            Some(pair) => pair.1 = value,
            None => pairs.push((key, value)),
        }
        Ok(())
    };
    if let Some(other) = other {
        if other.hasattr("keys")? {
            for key in other.call_method0("keys")?.try_iter()? {
                let key = key?;
                add(&key, other.get_item(&key)?)?;
            }
        } else {
            for (index, pair) in other.try_iter()?.enumerate() {
                let [key, value] = two_items(&pair?, index)?;
                add(&key, value)?;
            }
        }
    }
    for (key, value) in kwargs.into_iter().flatten() {
        add(&key, value)?;
    }
    Ok(pairs)
}

/// A pair is any iterable of two items, as for a dict: pairs loaded from JSON are lists. `index` is the pair's place
/// in what the caller passed, for the error.
fn two_items<'py>(pair: &Bound<'py, PyAny>, index: usize) -> PyResult<[Bound<'py, PyAny>; 2]> {
    let items = match pair.try_iter() {
        Ok(items) => items,
        Err(e) if e.is_instance_of::<PyTypeError>(pair.py()) => {
            return Err(PyTypeError::new_err(format!(
                "item {index} is not a (key, value) pair: it isn't iterable"
            )));
        }
        Err(e) => return Err(e),
    };
    // All of it, like a dict, so the error can say how long it was.
    let items = items.collect::<PyResult<Vec<_>>>()?;
    <[_; 2]>::try_from(items).map_err(|items| {
        let len = items.len();
        PyValueError::new_err(format!(
            "item {index} is not a (key, value) pair: it has length {len}, not 2"
        ))
    })
}

/// Writes every pair in one edit: a key that's there gets its value written as `props[key] = value` would, and a new
/// one goes at the end. With `replace`, every other property goes, as do the earlier duplicates of a key written,
/// so the node holds exactly these properties.
fn write(node: &Node, py: Python<'_>, pairs: &[(String, Bound<'_, PyAny>)], replace: bool) -> PyResult<()> {
    if pairs.is_empty() && !replace {
        return Ok(());
    }
    edit(
        py,
        node,
        Reads::Shape,
        |snapshot| {
            let written = pairs
                .iter()
                .map(|(key, value)| {
                    let found = last(&snapshot.items, key);
                    let encoded = match found {
                        Some(item) => item.encode_over(value, snapshot.types())?,
                        None => values::encode(value, None, snapshot.types())?,
                    };
                    Ok((found.map(|i| i.id), key.clone(), encoded))
                })
                .collect::<PyResult<Vec<_>>>()?;
            let kept: Vec<EntryId> = written.iter().filter_map(|w| w.0).collect();
            let removed: Vec<EntryId> = if replace {
                let props = snapshot.items.iter().filter(|i| i.name.is_some());
                props.map(|i| i.id).filter(|id| !kept.contains(id)).collect()
            } else {
                Vec::new()
            };
            Ok((written, removed))
        },
        |mut target, (written, removed)| {
            for (found, key, encoded) in written {
                if let Some(id) = found {
                    store(target.entry_mut(id), encoded);
                } else {
                    let end = target.len();
                    target.insert(end, new_entry(Some(key), encoded));
                }
            }
            for id in removed {
                target.remove(id);
            }
        },
    )
}

/// `node.props = mapping`: the node ends up with exactly these properties. Keys it had already keep their place and
/// spelling.
pub(crate) fn assign(node: &Node, value: &Bound<'_, PyAny>) -> PyResult<()> {
    write(node, value.py(), &pairs(Some(value), None)?, true)
}

/// A node's properties, as a live dict of values.
///
/// It works like a dict, and every edit changes the node. A new key goes after the last property. If a key appears
/// more than once in the text, the last one counts, as in KDL. A value written over a property keeps its annotation
/// and has to fit it, as with `Entry.value`. A `Value` sets the annotation too.
#[pyclass(module = "rkdl", frozen)]
pub struct Props {
    node: Py<Node>,
}

impl Props {
    pub(crate) fn new(node: Py<Node>) -> Self {
        Props { node }
    }

    /// Whether `value` is the props view of `node`.
    pub(crate) fn is_view_of(value: &Bound<'_, PyAny>, node: &Bound<'_, Node>) -> bool {
        value.cast::<Props>().is_ok_and(|view| view.get().node.is(node))
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
            Reads::Values,
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

    // Writes the entry `[]` reads, or adds a property at the end.
    fn __setitem__(&self, key: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let key = str_key(key).ok_or_else(|| PyTypeError::new_err("property keys must be str"))?;
        write(self.node.get(), value.py(), &[(key, value.clone())], false)
    }

    fn __delitem__(&self, key: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = key.py();
        let Some(k) = str_key(key) else {
            return Err(PyKeyError::new_err(key.clone().unbind()));
        };
        edit(
            py,
            self.node.get(),
            Reads::Shape,
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

    /// Returns the value of property `key`, or `default`.
    #[pyo3(signature = (key, default=None))]
    fn get(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        let found = match str_key(key) {
            Some(k) => get(py, &read(py, self.node.get())?, &k)?,
            None => None,
        };
        Ok(found.or(default).unwrap_or_else(|| py.None()))
    }

    /// Removes property `key` and returns its value, or `default` if there's no such property.
    ///
    /// Raises:
    ///     KeyError: There's no property `key`, and no default.
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

    /// Returns the value of property `key`, first adding it with `default` if there's no such property. A `Value`
    /// default returns the value as the property reads, without the wrapper.
    #[pyo3(signature = (key, default=None))]
    fn setdefault(&self, py: Python<'_>, key: &Bound<'_, PyAny>, default: Option<Py<PyAny>>) -> PyResult<Py<PyAny>> {
        let key = str_key(key).ok_or_else(|| PyTypeError::new_err("property keys must be str"))?;
        let default = default.unwrap_or_else(|| py.None());
        edit(
            py,
            self.node.get(),
            Reads::Values,
            |snapshot| {
                if let Some(item) = last(&snapshot.items, &key) {
                    return Ok((item.to_python(py, snapshot.types())?, None));
                }
                let encoded = values::encode(default.bind(py), None, snapshot.types())?;
                // A Value isn't what reading the property gives, so it returns the value as it now reads.
                let found = if default.bind(py).is_instance_of::<Value>() {
                    let Encoded { value, ty, repr } = &encoded;
                    let written = new_entry(Some(key.clone()), encoded.clone()).to_string();
                    let place = Place {
                        node: &snapshot.node,
                        slot: Slot::Prop(&key),
                        pos: None,
                        path: None,
                        text: written.trim(),
                    };
                    values::to_python_spelled(
                        py,
                        value,
                        repr.as_deref(),
                        ty.as_deref(),
                        snapshot.types(),
                        Some(&place),
                    )?
                } else {
                    default.clone_ref(py)
                };
                Ok((found, Some(encoded)))
            },
            |mut target, (found, new)| {
                if let Some(encoded) = new {
                    let end = target.len();
                    target.insert(end, new_entry(Some(key.clone()), encoded));
                }
                found
            },
        )
    }

    /// Sets properties from a mapping or an iterable of pairs, and from keyword arguments. A pair is any iterable of two
    /// items, such as a tuple or a list.
    ///
    /// If any value can't be converted, nothing changes.
    #[pyo3(signature = (other=None, /, **kwargs))]
    fn update(
        &self,
        py: Python<'_>,
        other: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        write(self.node.get(), py, &pairs(other, kwargs)?, false)
    }

    /// Returns the properties as a plain dict.
    fn copy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        self.dict(py)
    }

    // `props | mapping` is a plain dict, as `dict | dict` is.
    fn __or__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        if !is_mapping(other)? {
            return Ok(py.NotImplemented().into_bound(py));
        }
        let merged = self.dict(py)?;
        merge(&merged, other)?;
        Ok(merged.into_any())
    }

    fn __ror__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        if !is_mapping(other)? {
            return Ok(py.NotImplemented().into_bound(py));
        }
        let merged = PyDict::new(py);
        merge(&merged, other)?;
        merge(&merged, self.dict(py)?.as_any())?;
        Ok(merged.into_any())
    }

    // `props |= other` is `props.update(other)`, which, like a dict's, also takes an iterable of pairs.
    fn __ior__(&self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        self.update(other.py(), Some(other), None)
    }

    // Not through `KdlNode::remove`, which can't find parsed keys.
    /// Removes every property.
    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        edit(
            py,
            self.node.get(),
            Reads::Shape,
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

    // With duplicate keys, the last property is the entry `[]` reads, and an earlier one with the same key stays.
    /// Removes the last property and returns its key and value.
    ///
    /// Raises:
    ///     KeyError: There are no properties.
    fn popitem(&self, py: Python<'_>) -> PyResult<(String, Py<PyAny>)> {
        edit(
            py,
            self.node.get(),
            Reads::Values,
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

    // `Mapping`'s equality: the same keys with equal values, compared as dicts.
    fn __eq__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        if !is_mapping(other)? {
            return Ok(py.NotImplemented().into_bound(py));
        }
        let theirs = PyDict::new(py);
        merge(&theirs, other)?;
        let same = self.dict(py)?.eq(theirs)?;
        Ok(same.into_pyobject(py)?.to_owned().into_any())
    }

    /// Returns a live view of the keys.
    fn keys<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        abc_view(slf, "KeysView")
    }

    /// Returns a live view of the values.
    fn values<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        abc_view(slf, "ValuesView")
    }

    /// Returns a live view of the `(key, value)` pairs.
    fn items<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        abc_view(slf, "ItemsView")
    }

    /// Returns the `Entry` for a property.
    ///
    /// Raises:
    ///     KeyError: There's no property `key`.
    fn entry<'py>(&self, py: Python<'py>, key: &str) -> PyResult<Bound<'py, Entry>> {
        let mut found = chosen(py, self.node.get(), |items| {
            Ok(vec![last(items, key).ok_or_else(|| key_error(key))?.id])
        })?;
        Ok(found.pop().expect("chose one entry"))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let Ok(snapshot) = read(py, self.node.get()) else {
            return Ok("Props(<unreadable>)".to_owned());
        };
        let found = keys(&snapshot.items)
            .into_iter()
            .filter_map(|key| last(&snapshot.items, key).map(|item| (key, item)))
            .map(|(key, item)| {
                let key = PyString::new(py, key).repr().map(|k| k.to_string());
                Ok(format!(
                    "{}: {}",
                    key.unwrap_or_default(),
                    item.repr(py, snapshot.types(), false)?
                ))
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok(format!("Props({{{}}})", found.join(", ")))
    }

    // No `__clear__`: a cycle through the node passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.node)
    }
}

fn is_mapping(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value.is_instance(&value.py().import("collections.abc")?.getattr("Mapping")?)
}

/// Copies a mapping's items into `dict`.
fn merge(dict: &Bound<'_, PyDict>, mapping: &Bound<'_, PyAny>) -> PyResult<()> {
    for pair in mapping.call_method0("items")?.try_iter()? {
        let (key, value): (Bound<'_, PyAny>, Bound<'_, PyAny>) = pair?.extract()?;
        dict.set_item(key, value)?;
    }
    Ok(())
}

// The standard views are live and only need the mapping methods above.
fn abc_view<'py>(props: &Bound<'py, Props>, view: &str) -> PyResult<Bound<'py, PyAny>> {
    let py = props.py();
    py.import("collections.abc")?.getattr(view)?.call1((props,))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{repr, unreadable_node, with_python};

    // Reprs never raise: the props view of a node that can't be read (an rkdl bug state) says so.
    #[test]
    fn repr_of_a_node_that_cant_be_read() {
        with_python(|py| {
            let props = Bound::new(py, Props::new(unreadable_node(py))).expect("allocates");
            assert_eq!(repr(props.as_any()), "Props(<unreadable>)");
        });
    }
}
