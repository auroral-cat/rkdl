// Stubs until task 05 fills them in; task 05 removes this allow.
#![allow(dead_code, unused_variables)]

use kdl::KdlDocument;
use pyo3::prelude::*;

/// Autoformat settings, mirroring kdl-rs's FormatConfig.
#[pyclass(module = "rkdl", frozen)]
pub struct Formatter {}

#[pymethods]
impl Formatter {
    #[new]
    fn new() -> Self {
        Formatter {}
    }
}

impl Formatter {
    pub(crate) fn apply(&self, doc: &mut KdlDocument) {
        todo!()
    }
}
