//! The process-wide handle cache, and how handles find their document.
//!
//! Ids are global, so one cache covers every document and a move never has to move cache entries. What a move does
//! change is each handle's document pointer. That pointer lives in a `Home` shared between the handle and its cache
//! slot, so a move can repoint handles while the documents are still locked, without calling Python. A reader that
//! raced a move sees its pointer change and retries.
//!
//! Lock order: document locks, then the cache, then a `Home`. Nothing takes a document lock while holding the cache.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::type_object::PyTypeCheck;
use pyo3::types::PyWeakrefReference;

use crate::document::Document;

/// A document pointer. Every move installs a fresh Arc, so comparing Arcs tells a reader whether its handle moved,
/// even if it came back to the same document.
pub(crate) type DocRef = Arc<Py<Document>>;

/// The document a handle's node or entry currently lives in.
pub(crate) struct Home(Mutex<DocRef>);

impl Home {
    pub(crate) fn new(doc: DocRef) -> Arc<Home> {
        Arc::new(Home(Mutex::new(doc)))
    }

    pub(crate) fn doc(&self) -> DocRef {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Python references and pointers released under a lock. Dropping them can run finalizers, which can re-enter the
/// cache or a document, so they're dropped only after every lock is released.
#[derive(Default)]
pub(crate) struct Garbage {
    pub(crate) weakrefs: Vec<Py<PyWeakrefReference>>,
    homes: Vec<Arc<Home>>,
    docs: Vec<DocRef>,
}

struct Slot {
    object: Py<PyWeakrefReference>,
    // Dead once the handle is gone, which is how the sweep tells without calling Python.
    home: Weak<Home>,
}

struct Cache {
    slots: HashMap<u64, Slot>,
    swept_at: usize,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        slots: HashMap::new(),
        swept_at: 0,
    })
});

/// The live handle for `key`, if there is one.
pub(crate) fn get<'py, T: PyTypeCheck>(py: Python<'py>, key: u64) -> Option<Bound<'py, T>> {
    let found = {
        let cache = CACHE.lock_py_attached(py).unwrap_or_else(PoisonError::into_inner);
        cache.slots.get(&key).and_then(|slot| slot.object.bind(py).upgrade())
    };
    found?.cast_into::<T>().ok()
}

/// Records `object` as the handle for `key` unless a live one is already there, in which case `object` goes to the
/// garbage and this returns false. Called with the document that holds `key` locked, so no move can slip between
/// checking where the node is and publishing a handle that says so.
pub(crate) fn intern_locked(key: u64, object: Py<PyWeakrefReference>, home: &Arc<Home>, garbage: &mut Garbage) -> bool {
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.slots.get(&key).is_some_and(|slot| slot.home.strong_count() > 0) {
        garbage.weakrefs.push(object);
        return false;
    }
    let slot = Slot {
        object,
        home: Arc::downgrade(home),
    };
    if let Some(dead) = cache.slots.insert(key, slot) {
        garbage.weakrefs.push(dead.object);
    }
    // Sweeping only when the map has doubled keeps the cost per insert constant.
    if cache.slots.len() > (2 * cache.swept_at).max(64) {
        let dead = cache.slots.extract_if(|_, slot| slot.home.strong_count() == 0);
        garbage.weakrefs.extend(dead.map(|(_, slot)| slot.object));
        cache.swept_at = cache.slots.len();
    }
    true
}

/// Points the live handles among `keys` at `doc`. Called with the documents locked, alongside the move itself.
pub(crate) fn repoint(keys: impl IntoIterator<Item = u64>, doc: &DocRef, garbage: &mut Garbage) {
    let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    for key in keys {
        let Some(home) = cache.slots.get(&key).and_then(|slot| slot.home.upgrade()) else {
            continue;
        };
        let old = std::mem::replace(&mut *home.0.lock().unwrap_or_else(PoisonError::into_inner), doc.clone());
        garbage.docs.push(old);
        // This may be the last reference if the handle is being freed right now.
        garbage.homes.push(home);
    }
}

pub(crate) fn stale() -> PyErr {
    PyRuntimeError::new_err("handle is out of step with its document (a bug in rkdl)")
}

/// Why an operation on a document didn't go through.
pub(crate) enum Fail {
    /// The node wasn't in the document. Fine if a move changed the pointer meanwhile, a bug otherwise.
    Gone,
    /// The document changed between reading it and editing it. Try again.
    Changed,
    Err(PyErr),
}

impl From<PyErr> for Fail {
    fn from(err: PyErr) -> Self {
        Fail::Err(err)
    }
}

/// Runs `op` against the document `home` points at, again if the handle moved while `op` waited for the lock.
pub(crate) fn on_home<R>(home: &Home, mut op: impl FnMut(&DocRef) -> Result<R, Fail>) -> PyResult<R> {
    loop {
        let doc = home.doc();
        match op(&doc) {
            Ok(result) => return Ok(result),
            Err(Fail::Err(err)) => return Err(err),
            Err(Fail::Changed) => {}
            Err(Fail::Gone) if Arc::ptr_eq(&doc, &home.doc()) => return Err(stale()),
            Err(Fail::Gone) => {}
        }
    }
}
