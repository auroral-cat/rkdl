use pyo3::PyTraverseError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyList, PySlice};

use crate::edit::{Item, Reads, Snapshot, chosen, edit, read};
use crate::entry::{Entry, contents, new_entry, put_contents, store};
use crate::index::{Index, Splice, insert_at, not_in, pick_one, pick_slice, search_range};
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

/// Where in the entry list a new argument goes to become argument `index`: where the current `index`-th one is, so
/// properties between arguments stay put.
fn entry_index(args: &[(usize, &Item)], index: usize) -> usize {
    match (args.get(index), args.last()) {
        (Some(&(at, _)), _) => at,
        (None, Some(&(last, _))) => last + 1,
        (None, None) => 0,
    }
}

/// Removes the arguments `pick` chooses from the argument ids.
fn remove_args(py: Python<'_>, node: &Node, pick: impl Fn(&[EntryId]) -> PyResult<Vec<EntryId>>) -> PyResult<()> {
    edit(
        py,
        node,
        Reads::Shape,
        |snapshot| pick(&arg_ids(&snapshot.items)),
        |mut target, ids| ids.into_iter().for_each(|id| target.remove(id)),
    )
}

/// The first argument equal to `value` by Python's `==` among arguments `start..stop`, with its position.
fn find(snapshot: &Snapshot, value: &Bound<'_, PyAny>, start: Index, stop: Index) -> PyResult<Option<(usize, Item)>> {
    let args = args(&snapshot.items);
    for position in search_range(args.len(), start, stop) {
        let Some(&(_, item)) = args.get(position) else {
            break;
        };
        if item
            .to_python(value.py(), snapshot.types())?
            .bind(value.py())
            .eq(value)?
        {
            return Ok(Some((position, item.clone())));
        }
    }
    Ok(None)
}

fn find_any(snapshot: &Snapshot, value: &Bound<'_, PyAny>) -> PyResult<Option<Item>> {
    Ok(find(snapshot, value, Index(0), Index(isize::MAX))?.map(|(_, item)| item))
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
        Reads::Shape,
        |snapshot| {
            let args = args(&snapshot.items);
            let at = entry_index(&args, insert_at(args.len(), index));
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

/// `args[slice] = new` by list rules, or `node.args = new` with no slice. An argument that gets a new value keeps
/// its place, whitespace and annotation, as with `args[i] = value`. All or nothing.
pub(crate) fn assign(node: &Node, slice: Option<&Bound<'_, PySlice>>, new: &Bound<'_, PyAny>) -> PyResult<()> {
    let py = new.py();
    // Collected first, so `args[:] = args` ends.
    let new: Vec<_> = new.try_iter()?.collect::<PyResult<_>>()?;
    edit(
        py,
        node,
        Reads::Shape,
        |snapshot| {
            let args = args(&snapshot.items);
            let change = match slice {
                Some(slice) => Splice::of(args.len(), slice, new.len())?,
                None => Splice::all(args.len()),
            };
            let picked: Vec<&Item> = change.picked.iter().filter_map(|&p| args.get(p)).map(|a| a.1).collect();
            let written = picked
                .iter()
                .zip(&new)
                .map(|(item, value)| Ok((item.id, item.encode_over(value, snapshot.types())?)))
                .collect::<PyResult<Vec<_>>>()?;
            let removed: Vec<EntryId> = picked.iter().skip(new.len()).map(|item| item.id).collect();
            let added = new
                .iter()
                .skip(picked.len())
                .map(|v| values::from_python(v, None, snapshot.types()))
                .collect::<PyResult<Vec<_>>>()?;
            // A splice either removes arguments or adds them, never both, so this index needs no adjusting.
            let at = entry_index(&args, change.insert_at);
            Ok((written, removed, at, added))
        },
        |mut target, (written, removed, at, added)| {
            for (id, encoded) in written {
                store(target.entry_mut(id), encoded);
            }
            for id in removed {
                target.remove(id);
            }
            for (i, (value, ty)) in added.into_iter().enumerate() {
                target.insert(at + i, new_entry(None, value, ty));
            }
        },
    )
}

/// A node's arguments, as a live list of values.
///
/// It works like a list, and every edit changes the node. Arguments keep their place among the properties. A value
/// written over an argument keeps its annotation and has to fit it, as with `Entry.value`.
#[pyclass(module = "rkdl", frozen)]
pub struct Args {
    node: Py<Node>,
}

impl Args {
    pub(crate) fn new(node: Py<Node>) -> Self {
        Args { node }
    }

    /// Whether `value` is the args view of `node`.
    pub(crate) fn is_view_of(value: &Bound<'_, PyAny>, node: &Bound<'_, Node>) -> bool {
        value.cast::<Args>().is_ok_and(|view| view.get().node.is(node))
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
        let item = arg(&snapshot.items, index.extract::<Index>()?.0)?;
        Ok(item.to_python(py, snapshot.types())?.into_bound(py))
    }

    fn __setitem__(&self, index: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if let Ok(slice) = index.cast::<PySlice>() {
            return assign(self.node.get(), Some(slice), value);
        }
        let Index(index) = index.extract()?;
        edit(
            value.py(),
            self.node.get(),
            Reads::Shape,
            |snapshot| {
                let item = arg(&snapshot.items, index)?;
                Ok((item.id, item.encode_over(value, snapshot.types())?))
            },
            |mut target, (id, encoded)| store(target.entry_mut(id), encoded),
        )
    }

    fn __delitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = index.py();
        if let Ok(slice) = index.cast::<PySlice>() {
            return remove_args(py, self.node.get(), |ids| {
                Ok(pick_slice(ids, slice)?.into_iter().copied().collect())
            });
        }
        let Index(index) = index.extract()?;
        remove_args(py, self.node.get(), |ids| Ok(vec![*pick_one(ids, index, "args")?]))
    }

    // Puts the new argument where the current `index`-th one is, so properties between arguments stay put.
    /// Inserts an argument before the one at `index`.
    fn insert(&self, index: Index, value: &Bound<'_, PyAny>) -> PyResult<()> {
        insert_values(self.node.get(), index.0, std::slice::from_ref(value))
    }

    /// Adds an argument after the last one.
    fn append(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.insert(Index(isize::MAX), value)
    }

    /// Adds arguments after the last one. If any value can't be converted, none are added.
    fn extend(&self, iterable: &Bound<'_, PyAny>) -> PyResult<()> {
        // Collected first, so `args.extend(args)` ends.
        let new: Vec<_> = iterable.try_iter()?.collect::<PyResult<_>>()?;
        insert_values(self.node.get(), isize::MAX, &new)
    }

    /// Removes an argument and returns its value.
    #[pyo3(signature = (index=Index(-1)), text_signature = "($self, index=-1)")]
    fn pop(&self, py: Python<'_>, index: Index) -> PyResult<Py<PyAny>> {
        edit(
            py,
            self.node.get(),
            Reads::Values,
            |snapshot| {
                let item = arg(&snapshot.items, index.0)?;
                Ok((item.id, item.to_python(py, snapshot.types())?))
            },
            |mut target, (id, value)| {
                target.remove(id);
                value
            },
        )
    }

    /// Removes the first argument equal to `value`.
    ///
    /// Raises:
    ///     ValueError: No argument is equal to `value`.
    fn remove(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        edit(
            value.py(),
            self.node.get(),
            Reads::Values,
            |snapshot| {
                find_any(snapshot, value)?
                    .map(|item| item.id)
                    .ok_or_else(|| not_in(value, "args"))
            },
            |mut target, id| target.remove(id),
        )
    }

    /// Removes every argument.
    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        remove_args(py, self.node.get(), |ids| Ok(ids.to_vec()))
    }

    // Reverses the arguments' values in place. Each argument's slot keeps its whitespace and its place among the
    // properties; the values move with their annotations and spellings.
    /// Reverses the arguments in place. Each slot in the text keeps its whitespace; the values move.
    fn reverse(&self, py: Python<'_>) -> PyResult<()> {
        edit(
            py,
            self.node.get(),
            Reads::Shape,
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

    /// Returns the position of the first argument equal to `value`.
    ///
    /// Raises:
    ///     ValueError: No argument is equal to `value`.
    #[pyo3(
        signature = (value, start=Index(0), stop=Index(isize::MAX)),
        text_signature = "($self, value, start=0, stop=sys.maxsize)"
    )]
    fn index(&self, value: &Bound<'_, PyAny>, start: Index, stop: Index) -> PyResult<usize> {
        let snapshot = read(value.py(), self.node.get())?;
        let (position, _) = find(&snapshot, value, start, stop)?.ok_or_else(|| not_in(value, "args"))?;
        Ok(position)
    }

    /// Returns how many arguments are equal to `value`.
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
        Ok(find_any(&read(value.py(), self.node.get())?, value)?.is_some())
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

    // Equal to an `Args` or a list with equal values in the same order. Like a list, it isn't equal to a tuple.
    fn __eq__<'py>(&self, other: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        let py = other.py();
        let theirs = if let Ok(other) = other.cast::<Args>() {
            PyList::new(py, arg_values(py, &read(py, other.get().node.get())?)?)?
        } else if let Ok(other) = other.cast::<PyList>() {
            other.clone()
        } else {
            return Ok(py.NotImplemented().into_bound(py));
        };
        let ours = PyList::new(py, arg_values(py, &read(py, self.node.get())?)?)?;
        Ok(ours.eq(theirs)?.into_pyobject(py)?.to_owned().into_any())
    }

    /// Returns the `Entry` for an argument.
    ///
    /// Raises:
    ///     IndexError: There's no argument at `index`.
    fn entry<'py>(&self, py: Python<'py>, index: Index) -> PyResult<Bound<'py, Entry>> {
        let mut found = chosen(py, self.node.get(), |items| Ok(vec![arg(items, index.0)?.id]))?;
        Ok(found.pop().expect("chose one entry"))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let Ok(snapshot) = read(py, self.node.get()) else {
            return Ok("Args(<unreadable>)".to_owned());
        };
        let found = args(&snapshot.items)
            .into_iter()
            .map(|(_, item)| item.repr(py, snapshot.types(), false))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(format!("Args([{}])", found.join(", ")))
    }

    // No `__clear__`: a cycle through the node passes a Type, which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.node)
    }
}
