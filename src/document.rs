use std::sync::{Arc, Mutex, MutexGuard};

use kdl::KdlDocument;
use pyo3::prelude::*;

use crate::format::Formatter;
use crate::tree::IdTree;
use crate::values::TypeRegistry;

pub(crate) struct DocState {
    pub(crate) kdl: KdlDocument,
    pub(crate) ids: IdTree,
}

impl DocState {
    fn check(&self) {
        if cfg!(debug_assertions) {
            self.ids.check_shape(&self.kdl);
        }
    }
}

#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Document {
    state: Mutex<DocState>,
    // Read by the value views in round 3, which removes this allow.
    #[allow(dead_code)]
    types: Arc<TypeRegistry>,
}

impl Document {
    pub(crate) fn new(kdl: KdlDocument, types: Arc<TypeRegistry>) -> Self {
        Document {
            state: Mutex::new(DocState {
                ids: IdTree::from_document(&kdl),
                kdl,
            }),
            types,
        }
    }

    fn lock(&self) -> MutexGuard<'_, DocState> {
        // A panic while locked means the two trees may disagree, so a poisoned document stays unusable.
        self.state.lock().expect("document state poisoned by an earlier panic")
    }

    /// The only way to touch DocState. The closure can't hold a Python token, so it can't call into Python while
    /// the lock is held.
    pub(crate) fn with_state<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut DocState) -> R + Send) -> R {
        py.detach(|| {
            let mut state = self.lock();
            let result = f(&mut state);
            state.check();
            result
        })
    }
}

/// Locks two documents for an edit that spans both, such as moving a node. They're locked in address order so two
/// threads moving nodes in opposite directions can't deadlock. If `a` and `b` are the same document it's locked once
/// and `f` gets `None` for `b`.
// Used by node moves in round 3, which removes this allow.
#[allow(dead_code)]
pub(crate) fn lock_both<R: Send>(
    py: Python<'_>,
    a: &Document,
    b: &Document,
    f: impl FnOnce(&mut DocState, Option<&mut DocState>) -> R + Send,
) -> R {
    py.detach(|| {
        if std::ptr::eq(a, b) {
            let mut state = a.lock();
            let result = f(&mut state, None);
            state.check();
            return result;
        }
        let (mut sa, mut sb) = if (a as *const Document) < (b as *const Document) {
            let sa = a.lock();
            (sa, b.lock())
        } else {
            let sb = b.lock();
            (a.lock(), sb)
        };
        let result = f(&mut sa, Some(&mut sb));
        sa.check();
        sb.check();
        result
    })
}

#[pymethods]
impl Document {
    fn __str__(&self, py: Python<'_>) -> String {
        self.with_state(py, |s| s.kdl.to_string())
    }

    fn __len__(&self, py: Python<'_>) -> usize {
        self.with_state(py, |s| s.kdl.nodes().len())
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        format!("<Document with {} nodes>", self.__len__(py))
    }

    // autoformat only rewrites format strings, never structure, so the IdTree stays valid.
    #[pyo3(signature = (formatter=None))]
    fn fmt(&self, py: Python<'_>, formatter: Option<&Formatter>) {
        self.with_state(py, |s| match formatter {
            Some(f) => f.apply(&mut s.kdl),
            None => Formatter::default().apply(&mut s.kdl),
        });
    }
}
