// Stubs until task 03 fills them in; task 03 removes this allow.
#![allow(dead_code, unused_variables)]

use kdl::KdlValue;
use pyo3::prelude::*;
use pyo3::types::PyType;

/// One type annotation that converts values. Exposed to Python as rkdl.Type(name, cls, decode, encode).
#[pyclass(module = "rkdl", frozen)]
pub struct Type {
    name: String,
    cls: Py<PyType>,
    decode: Py<PyAny>,
    encode: Py<PyAny>,
}

#[pymethods]
impl Type {
    #[new]
    #[pyo3(signature = (name, cls, decode, encode))]
    fn new(name: String, cls: Py<PyType>, decode: Py<PyAny>, encode: Py<PyAny>) -> PyResult<Self> {
        todo!()
    }
}

/// Built-in reserved types plus any user Types. Immutable once built, shared by Arc.
pub struct TypeRegistry {}

impl TypeRegistry {
    pub fn builtin() -> Self {
        // TODO(03): register the reserved types. Empty for now so parsing works.
        TypeRegistry {}
    }

    pub fn with_user_types(py: Python<'_>, types: Vec<Py<Type>>) -> PyResult<Self> {
        todo!()
    }
}

/// KDL value plus its entry's type annotation -> Python object. Raises if a known type's contents don't fit.
pub fn to_python(py: Python<'_>, value: &KdlValue, ty: Option<&str>, types: &TypeRegistry) -> PyResult<Py<PyAny>> {
    todo!()
}

/// Python object -> KDL value plus the annotation to write, applying the "keep the current annotation if the value
/// fits it" rule from plan.md.
pub fn from_python(
    value: &Bound<'_, PyAny>,
    current_ty: Option<&str>,
    types: &TypeRegistry,
) -> PyResult<(KdlValue, Option<String>)> {
    todo!()
}

/// Canonical KDL text for a value, written into KdlEntryFormat::value_repr after every value write.
pub fn canonical_repr(value: &KdlValue) -> String {
    todo!()
}
