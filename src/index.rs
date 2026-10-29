//! Python's index rules, shared by every list-like view. `view` names the view in errors: `children`, `args` or
//! `entries`.

use pyo3::exceptions::{PyIndexError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PySlice;

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
