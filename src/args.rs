use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::entry::{Entry, Item, Snapshot, chosen, edit, new_entry, read, store};
use crate::index::{insert_at, not_in, pick_one, pick_slice};
use crate::node::Node;
use crate::values;

/// The node's arguments: (index in the entry list, item).
fn args(items: &[Item]) -> Vec<(usize, &Item)> {
    items.iter().enumerate().filter(|(_, i)| i.name.is_none()).collect()
}

fn arg(items: &[Item], index: isize) -> PyResult<&Item> {
    Ok(pick_one(&args(items), index, "args")?.1)
}

fn values<'py>(py: Python<'py>, snapshot: &Snapshot) -> PyResult<Vec<Py<PyAny>>> {
    args(&snapshot.items)
        .into_iter()
        .map(|(_, item)| item.to_python(py, snapshot.types()))
        .collect()
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
            let values = values(py, &snapshot)?;
            return Ok(PyList::new(py, pick_slice(&values, slice)?)?.into_any());
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
        let slice = index.cast::<PySlice>().ok();
        let index: Option<isize> = if slice.is_some() { None } else { Some(index.extract()?) };
        edit(
            py,
            self.node.get(),
            |snapshot| {
                let ids: Vec<_> = args(&snapshot.items).into_iter().map(|(_, i)| i.id).collect();
                match (&slice, index) {
                    (Some(slice), _) => Ok(pick_slice(&ids, slice)?.into_iter().copied().collect()),
                    (None, Some(index)) => Ok(vec![*pick_one(&ids, index, "args")?]),
                    (None, None) => unreachable!("an index or a slice"),
                }
            },
            |mut target, ids| ids.into_iter().for_each(|id| target.remove(id)),
        )
    }

    /// Puts the new argument where the current `index`-th one is, so properties between arguments stay put.
    fn insert(&self, index: isize, value: &Bound<'_, PyAny>) -> PyResult<()> {
        edit(
            value.py(),
            self.node.get(),
            |snapshot| {
                let args = args(&snapshot.items);
                let at = match (args.get(insert_at(args.len(), index)), args.last()) {
                    (Some(&(at, _)), _) => at,
                    (None, Some(&(last, _))) => last + 1,
                    (None, None) => 0,
                };
                let (value, ty) = values::from_python(value, None, snapshot.types())?;
                Ok((at, value, ty))
            },
            |mut target, (at, value, ty)| target.insert(at, new_entry(None, value, ty)),
        )
    }

    fn append(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.insert(isize::MAX, value)
    }

    fn extend(&self, values: &Bound<'_, PyAny>) -> PyResult<()> {
        // Collected first, so `args.extend(args)` ends.
        let values: Vec<_> = values.try_iter()?.collect::<PyResult<_>>()?;
        values.iter().try_for_each(|v| self.append(v))
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
        edit(
            py,
            self.node.get(),
            |snapshot| Ok(args(&snapshot.items).into_iter().map(|(_, i)| i.id).collect::<Vec<_>>()),
            |mut target, ids| ids.into_iter().for_each(|id| target.remove(id)),
        )
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
        for v in values(py, &snapshot)? {
            count += usize::from(v.bind(py).eq(value)?);
        }
        Ok(count)
    }

    fn __contains__(&self, value: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(find(&read(value.py(), self.node.get())?, value)?.is_some())
    }

    // Converts every value first, so editing the node while iterating is well defined.
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let values = values(py, &read(py, self.node.get())?)?;
        Ok(PyList::new(py, values)?.try_iter()?.into_any())
    }

    fn __reversed__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let mut values = values(py, &read(py, self.node.get())?)?;
        values.reverse();
        Ok(PyList::new(py, values)?.try_iter()?.into_any())
    }

    /// The handle for the `index`-th argument.
    fn entry<'py>(&self, py: Python<'py>, index: isize) -> PyResult<Bound<'py, Entry>> {
        let mut found = chosen(py, self.node.get(), |items| Ok(vec![arg(items, index)?.id]))?;
        Ok(found.pop().expect("chose one entry"))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let values = values(py, &read(py, self.node.get())?)?;
        Ok(format!("Args({})", PyList::new(py, values)?.repr()?))
    }
}
