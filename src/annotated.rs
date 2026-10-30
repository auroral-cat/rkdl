//! `rkdl.Value`: a value to write together with its type annotation.

use pyo3::PyTraverseError;
use pyo3::exceptions::PyTypeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyString, PyTuple};

// No `__clear__`: it's frozen, and a cycle through it has to pass the object it holds, which is mutable and clears.
/// A value to write with the type annotation given here, in place of whatever annotation the entry had.
///
/// Write one anywhere a value goes: `node.args[0] = Value(8080, type="u16")`, `node.props["when"] =
/// Value("2029-01-01", type="date")`, `entry.value = ...`, `append`, `update`, `Node("server", Value(8080, type="u16"))`.
/// The value must fit an annotation rkdl knows, or the write raises `ValueError` and nothing changes. `type=None`
/// writes the value with no annotation at all, so `Value(date(2029, 1, 1))` is the plain string `"2029-01-01"`.
///
/// Reading the entry back gives the plain Python value, not a Value.
///
/// Args:
///     value: The value to write.
///     type: The annotation, without parentheses: `"u16"` for `(u16)8080`. None for no annotation.
///
/// Raises:
///     TypeError: `type` isn't a str or None, or `value` is itself a Value.
#[pyclass(module = "rkdl", frozen)]
pub(crate) struct Value {
    value: Py<PyAny>,
    ty: Option<String>,
}

impl Value {
    /// The value and the annotation to write, if `value` is a Value.
    pub(crate) fn unwrap<'py>(value: &Bound<'py, PyAny>) -> Option<(Bound<'py, PyAny>, Option<String>)> {
        let inner = value.cast::<Value>().ok()?.get();
        Some((inner.value.bind(value.py()).clone(), inner.ty.clone()))
    }
}

#[pymethods]
impl Value {
    #[classattr]
    #[pyo3(name = "__match_args__")]
    fn match_args() -> (&'static str, &'static str) {
        ("value", "type")
    }

    #[new]
    #[pyo3(signature = (value, r#type=None))]
    fn new(value: Bound<'_, PyAny>, r#type: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        if value.is_instance_of::<Value>() {
            return Err(PyTypeError::new_err(
                "a Value can't hold another Value; give its .value and the annotation to use",
            ));
        }
        let ty = match r#type.filter(|t| !t.is_none()) {
            None => None,
            Some(t) if t.is_instance_of::<PyString>() => Some(t.extract::<String>()?),
            Some(t) => {
                return Err(PyTypeError::new_err(format!(
                    "type must be a str or None, not {}",
                    t.get_type().qualname()?
                )));
            }
        };
        Ok(Value {
            value: value.unbind(),
            ty,
        })
    }

    /// The value to write.
    #[getter]
    fn value(&self, py: Python<'_>) -> Py<PyAny> {
        self.value.clone_ref(py)
    }

    /// The annotation to write, or None for no annotation.
    #[getter]
    fn r#type(&self) -> Option<String> {
        self.ty.clone()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let value = self.value.bind(py).repr()?;
        Ok(match &self.ty {
            Some(ty) => format!("Value({value}, type={})", PyString::new(py, ty).repr()?),
            None => format!("Value({value})"),
        })
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = other.py();
        let Ok(other) = other.cast::<Value>() else {
            return Ok(py.NotImplemented());
        };
        let other = other.get();
        if self.ty != other.ty {
            return Ok(false.into_pyobject(py)?.to_owned().into_any().unbind());
        }
        let same = self.value.bind(py).eq(other.value.bind(py))?;
        Ok(same.into_pyobject(py)?.to_owned().into_any().unbind())
    }

    fn __hash__(&self, py: Python<'_>) -> PyResult<isize> {
        PyTuple::new(py, [self.value.bind(py).clone(), self.r#type().into_pyobject(py)?])?.hash()
    }

    // Pickling and copying make a new Value from these.
    fn __getnewargs__(&self, py: Python<'_>) -> (Py<PyAny>, Option<String>) {
        (self.value.clone_ref(py), self.ty.clone())
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.value)
    }
}
