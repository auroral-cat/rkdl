use pyo3::PyTraverseError;
use pyo3::exceptions::PyTypeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::edit::{Item, Snapshot, chosen, edit, read};
use crate::entry::{Entry, contents, new_entry, put_contents, store};
use crate::index::{insert_at, not_in, pick_one, pick_slice};
use crate::node::Node;
use crate::tree::EntryId;
use crate::values;

/// The node's arguments: (index in the entry list, item).
fn args(items: &[Item]) -> Vec<(usize, &Item)> {
    items.iter().enumerate().filter(|(_, i)| i.name.is_none()).collect()
}

fn arg(items: &[Item], index: isize) -> PyResult<&Item> {
    Ok(pick_one(&args(items), index, "args")?.1)
}

fn arg_values(py: Python<'_>, snapshot: &Snapshot) -> PyResult<Vec<Py<PyAny>>> {
    args(&snapshot.items)
        .into_iter()
        .map(|(_, item)| item.to_python(py, snapshot.types()))
        .collect()
}

fn arg_ids(items: &[Item]) -> Vec<EntryId> {
    args(items).into_iter().map(|(_, i)| i.id).collect()
}

/// Removes the arguments `pick` chooses from the argument ids.
fn remove_args(py: Python<'_>, node: &Node, pick: impl Fn(&[EntryId]) -> PyResult<Vec<EntryId>>) -> PyResult<()> {
    edit(
        py,
        node,
        |snapshot| pick(&arg_ids(&snapshot.items)),
        |mut target, ids| ids.into_iter().for_each(|id| target.remove(id)),
    )
}

/// The first argument equal to `value`, by Python's `==`.
fn find(snapshot: &Snapshot, value: &Bound<'_, PyAny>) -> PyResult<Option<Item>> {
    for (_, item) in args(&snapshot.items) {
        if item
            .to_python(value.py(), snapshot.types())?
            .bind(value.py())
            .eq(value)?
        {
            return Ok(Some(item.clone()));
        }
    }
    Ok(None)
}

/// Inserts `new` as arguments where the current `index`-th one is. Every value is converted before any is written,
/// so a value that can't be leaves the node unchanged.
fn insert_values(node: &Node, index: isize, new: &[Bound<'_, PyAny>]) -> PyResult<()> {
    let Some(first) = new.first() else {
        return Ok(());
    };
    edit(
        first.py(),
        node,
        |snapshot| {
            let args = args(&snapshot.items);
            let at = match (args.get(insert_at(args.len(), index)), args.last()) {
                (Some(&(at, _)), _) => at,
                (None, Some(&(last, _))) => last + 1,
                (None, None) => 0,
            };
            let converted = new
                .iter()
                .map(|v| values::from_python(v, None, snapshot.types()))
                .collect::<PyResult<Vec<_>>>()?;
            Ok((at, converted))
        },
        |mut target, (at, converted)| {
            for (i, (value, ty)) in converted.into_iter().enumerate() {
                target.insert(at + i, new_entry(None, value, ty));
            }
        },
    )
}

/// A live list of a node's arguments, as plain Python values.
#[pyclass(module = "rkdl", frozen)]
pub struct Args {
    node: Py<Node>,
}

impl Args {
    pub(crate) fn new(node: Py<Node>) -> Self {
        Args { node }
    }
}

#[pymethods]
impl Args {
    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        Ok(args(&read(py, self.node.get())?.items).len())
    }

    fn __getitem__<'py>(&self, index: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = index.py();
        let snapshot = read(py, self.node.get())?;
        if let Ok(slice) = index.cast::<PySlice>() {
            let found = arg_values(py, &snapshot)?;
            return Ok(PyList::new(py, pick_slice(&found, slice)?)?.into_any());
        }
        let item = arg(&snapshot.items, index.extract()?)?;
        Ok(item.to_python(py, snapshot.types())?.into_bound(py))
    }

    fn __setitem__(&self, index: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if index.is_instance_of::<PySlice>() {
            return Err(PyTypeError::new_err("node.args doesn't support slice assignment"));
        }
        let index: isize = index.extract()?;
        edit(
            value.py(),
            self.node.get(),
            |snapshot| {
                let item = arg(&snapshot.items, index)?;
                let (value, ty) = values::from_python(value, item.ty.as_deref(), snapshot.types())?;
                Ok((item.id, value, ty))
            },
            |mut target, (id, value, ty)| store(target.entry_mut(id), value, ty),
        )
    }

    fn __delitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = index.py();
        if let Ok(slice) = index.cast::<PySlice>() {
            return remove_args(py, self.node.get(), |ids| {
                Ok(pick_slice(ids, slice)?.into_iter().copied().collect())
            });
        }
        let index: isize = index.extract()?;
        remove_args(py, self.node.get(), |ids| Ok(vec![*pick_one(ids, index, "args")?]))
    }

    /// Puts the new argument where the current `index`-th one is, so properties between arguments stay put.
    fn insert(&self, index: isize, value: &Bound<'_, PyAny>) -> PyResult<()> {
        insert_values(self.node.get(), index, std::slice::from_ref(value))
    }

    fn append(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.insert(isize::MAX, value)
    }

    fn extend(&self, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        // Collected first, so `args.extend(args)` ends.
        let new: Vec<_> = iterable.try_iter()?.collect::<PyResult<_>>()?;
        insert_values(self.node.get(), isize::MAX, &new)
    }

    #[pyo3(signature = (index=-1))]
    fn pop(&self, py: Python<'_>, index: isize) -> PyResult<Py<PyAny>> {
        edit(
            py,
            self.node.get(),
            |snapshot| {
                let item = arg(&snapshot.items, index)?;
                Ok((item.id, item.to_python(py, snapshot.types())?))
            },
            |mut target, (id, value)| {
                target.remove(id);
                value
            },
        )
    }

    fn remove(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        edit(
            value.py(),
            self.node.get(),
            |snapshot| {
                find(snapshot, value)?
                    .map(|item| item.id)
                    .ok_or_else(|| not_in(value, "args"))
            },
            |mut target, id| target.remove(id),
        )
    }

    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        remove_args(py, self.node.get(), |ids| Ok(ids.to_vec()))
    }

    /// Reverses the arguments' values in place. Each argument's slot keeps its whitespace and its place among the
    /// properties; the values move with their annotations and spellings.
    fn reverse(&self, py: Python<'_>) -> PyResult<()> {
        edit(
            py,
            self.node.get(),
            |snapshot| Ok(arg_ids(&snapshot.items)),
            |mut target, ids| {
                let moved: Vec<_> = ids.iter().map(|&id| contents(target.entry_mut(id))).collect();
                for (&id, c) in ids.iter().zip(moved.into_iter().rev()) {
                    put_contents(target.entry_mut(id), c);
                }
            },
        )
    }

    fn __iadd__(&self, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        self.extend(iterable)
    }

    fn index(&self, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        let snapshot = read(value.py(), self.node.get())?;
        let found = find(&snapshot, value)?.ok_or_else(|| not_in(value, "args"))?;
        Ok(args(&snapshot.items)
            .iter()
            .position(|(_, i)| i.id == found.id)
            .expect("found among the arguments"))
    }

    fn count(&self, value: &Bound<'_, PyAny>) -> PyResult<usize> {
        let py = value.py();
        let snapshot = read(py, self.node.get())?;
        let mut count = 0;
        for v in arg_values(py, &snapshot)? {
            count += usize::from(v.bind(py).eq(value)?);
        }
        Ok(count)
    }

    fn __contains__(&self, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(find(&read(value.py(), self.node.get())?, value)?.is_some())
    }

    // Converts every value first, so editing the node while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let found = arg_values(py, &read(py, self.node.get())?)?;
        Ok(PyList::new(py, found)?.try_iter()?.into_any())
    }

    fn __reversed__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut found = arg_values(py, &read(py, self.node.get())?)?;
        found.reverse();
        Ok(PyList::new(py, found)?.try_iter()?.into_any())
    }

    /// The handle for the `index`-th argument.
    fn entry<'py>(&self, py: Python<'py>, index: isize) -> PyResult<Bound<'py, Entry>> {
        let mut found = chosen(py, self.node.get(), |items| Ok(vec![arg(items, index)?.id]))?;
        Ok(found.pop().expect("chose one entry"))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let found = arg_values(py, &read(py, self.node.get())?)?;
        Ok(format!("Args({})", PyList::new(py, found)?.repr()?))
    }

    // No `__clear__`: a cycle through the node passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.node)
    }
}
