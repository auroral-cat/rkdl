//! Python's index rules, shared by every list-like view. `view` names the view in errors: `children`, `args` or
//! `entries`.

use pyo3::exceptions::{PyIndexError, PyOverflowError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PySlice;

/// An index from Python. Any int is accepted: one outside `isize` saturates to `isize::MIN` or `isize::MAX`, which is
/// past either end of any sequence, so it reads as out of range and `insert` clamps it, like a list.
#[derive(Clone, Copy)]
pub(crate) struct Index(pub(crate) isize);

impl<'a, 'py> FromPyObject<'a, 'py> for Index {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        match obj.extract::<isize>() {
            Ok(i) => Ok(Index(i)),
            Err(err) if err.is_instance_of::<PyOverflowError>(obj.py()) => {
                let int = obj.py().import("operator")?.getattr("index")?.call1((obj,))?;
                Ok(Index(if int.lt(0)? { isize::MIN } else { isize::MAX }))
            }
            Err(err) => Err(err),
        }
    }
}

fn out_of_range(view: &str) -> PyErr {
    PyIndexError::new_err(format!("{view} index out of range"))
}

/// Where `index` points in a sequence of `len` items. Negative counts from the end.
pub(crate) fn position(len: usize, index: isize, view: &str) -> PyResult<usize> {
    let i = match usize::try_from(index) {
        Ok(i) => Some(i),
        Err(_) => len.checked_sub(index.unsigned_abs()),
    };
    i.filter(|&i| i < len).ok_or_else(|| out_of_range(view))
}

/// The item `index` points at.
pub(crate) fn pick_one<'a, T>(items: &'a [T], index: isize, view: &str) -> PyResult<&'a T> {
    items
        .get(position(items.len(), index, view)?)
        .ok_or_else(|| out_of_range(view))
}

/// The indexes a slice picks from a sequence of `len` items.
pub(crate) fn sliced(len: usize, slice: &Bound<'_, PySlice>) -> PyResult<Vec<usize>> {
    let s = slice.indices(isize::try_from(len)?)?;
    // `indices` clamps start and step so every index lands in 0..len.
    Ok((0..s.slicelength)
        .map(|k| (s.start + k.cast_signed() * s.step).cast_unsigned())
        .collect())
}

/// What `seq[slice] = new` does to a sequence of `len` items, by list rules. A plain slice replaces the items it
/// picks with however many new ones there are; an extended slice (a step other than 1) replaces each item it picks
/// with one new item.
pub(crate) struct Splice {
    /// The positions picked, in the order the slice picks them.
    pub(crate) picked: Vec<usize>,
    /// For a plain slice, where new items beyond the picked ones go: the slice's start, after the picked items.
    pub(crate) insert_at: usize,
}

impl Splice {
    pub(crate) fn of(len: usize, slice: &Bound<'_, PySlice>, new: usize) -> PyResult<Splice> {
        let s = slice.indices(isize::try_from(len)?)?;
        let picked = sliced(len, slice)?;
        if s.step != 1 && picked.len() != new {
            return Err(PyValueError::new_err(format!(
                "attempt to assign sequence of size {new} to extended slice of size {}",
                picked.len()
            )));
        }
        // A plain slice's start is in 0..=len, and its picks run on from there.
        let insert_at = if s.step == 1 {
            s.start.cast_unsigned() + picked.len()
        } else {
            0
        };
        Ok(Splice { picked, insert_at })
    }

    /// Every item replaced, as `seq[:] = new` does.
    pub(crate) fn all(len: usize) -> Splice {
        Splice {
            picked: (0..len).collect(),
            insert_at: len,
        }
    }
}

/// The positions `seq.index(x, start, stop)` searches, by list rules: negative bounds count from the end, and both
/// clamp to the sequence.
pub(crate) fn search_range(len: usize, start: Index, stop: Index) -> std::ops::Range<usize> {
    let start = insert_at(len, start.0);
    start..insert_at(len, stop.0).max(start)
}

/// The items a slice picks.
pub(crate) fn pick_slice<'a, T>(items: &'a [T], slice: &Bound<'_, PySlice>) -> PyResult<Vec<&'a T>> {
    Ok(sliced(items.len(), slice)?
        .into_iter()
        .filter_map(|i| items.get(i))
        .collect())
}

/// Where `list.insert(index, x)` puts `x` in a sequence of `len` items: out-of-range indexes clamp to either end.
pub(crate) fn insert_at(len: usize, index: isize) -> usize {
    match usize::try_from(index) {
        Ok(i) => i.min(len),
        Err(_) => len.saturating_sub(index.unsigned_abs()),
    }
}

/// The `ValueError` for a `remove` or `index` that found nothing.
pub(crate) fn not_in(value: &Bound<'_, PyAny>, view: &str) -> PyErr {
    match value.repr() {
        Ok(repr) => PyValueError::new_err(format!("{repr} not in {view}")),
        Err(err) => err,
    }
}
