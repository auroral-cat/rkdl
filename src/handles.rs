//! The process-wide handle cache, and how handles find their document.
//!
//! Ids are global, so one cache covers every document and a move never has to move cache entries. What a move does
//! change is each handle's document pointer. That pointer lives in a `Home` shared between the handle and its cache
//! slot, so a move can repoint handles while the documents are still locked, without calling Python. A reader that
//! raced a move sees its home's generation change and retries.
//!
//! Lock order: document locks, then the cache, then a `Home`. Nothing takes a document lock while holding the cache.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, TryLockError, Weak};

use pyo3::exceptions::PyRuntimeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::sync::MutexExt;
use pyo3::type_object::PyTypeCheck;
use pyo3::types::PyWeakrefReference;
use pyo3::{PyClass, PyTraverseError};

use crate::document::{DocState, Document};

/// Stamped on a home by every move, so a reader can tell its handle moved, even if it came back to the same document.
/// Homes that never moved have generation 0.
static GENERATION: AtomicU64 = AtomicU64::new(1);

/// How a home points at its document.
enum Ptr {
    /// A reference of the home's own, which its handle reports to the cycle collector.
    Own(Py<Document>),
    /// One reference shared by every home a move repointed. The move runs detached, where it can't make references,
    /// so it installs this and `Garbage::release` swaps in owned ones once it's done. Nobody reports it to the
    /// collector: reporting it from each holder would count more references than exist.
    Shared(Arc<Py<Document>>),
}

impl Ptr {
    fn doc(&self) -> &Py<Document> {
        match self {
            Ptr::Own(doc) => doc,
            Ptr::Shared(doc) => doc,
        }
    }
}

struct Place {
    ptr: Ptr,
    generation: u64,
}

/// The document a handle's node or entry currently lives in.
pub(crate) struct Home(Mutex<Place>);

/// A home's document as read at one moment, to check later whether the handle moved since.
pub(crate) struct DocRef {
    doc: Py<Document>,
    generation: u64,
}

impl Deref for DocRef {
    type Target = Py<Document>;

    fn deref(&self) -> &Py<Document> {
        &self.doc
    }
}

impl Home {
    pub(crate) fn new(doc: Py<Document>) -> Arc<Home> {
        Arc::new(Home(Mutex::new(Place {
            ptr: Ptr::Own(doc),
            generation: 0,
        })))
    }

    fn place(&self) -> MutexGuard<'_, Place> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn doc(&self, py: Python<'_>) -> DocRef {
        let place = self.place();
        DocRef {
            doc: place.ptr.doc().clone_ref(py),
            generation: place.generation,
        }
    }

    /// Whether a move has repointed this home since `doc` was read from it. A handle that moved away and back still
    /// counts as moved.
    pub(crate) fn moved_from(&self, doc: &DocRef) -> bool {
        self.place().generation != doc.generation
    }

    /// Tells the cycle collector about the document, for the handle that owns this home.
    pub(crate) fn traverse(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        // A move on another thread may be repointing this home right now, and traversal mustn't wait for it.
        let place = match self.0.try_lock() {
            Ok(place) => place,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Ok(()),
        };
        match &place.ptr {
            Ptr::Own(doc) => visit.call(doc),
            Ptr::Shared(_) => Ok(()),
        }
    }

    /// Gives a home that `repoint` moved a reference of its own, unless another move has repointed it since. Returns
    /// the pointer it replaced, to drop once the lock is released.
    fn settle(&self, py: Python<'_>, generation: u64, to: &Py<Document>) -> Option<Ptr> {
        let mut place = self.place();
        if place.generation != generation {
            return None;
        }
        Some(std::mem::replace(&mut place.ptr, Ptr::Own(to.clone_ref(py))))
    }
}

/// Python references and pointers released under a lock. Dropping them can run finalizers, which can re-enter the
/// cache or a document, so they're dropped only after every lock is released.
#[derive(Default)]
pub(crate) struct Garbage {
    pub(crate) weakrefs: Vec<Py<PyWeakrefReference>>,
    old: Vec<Ptr>,
    moved: Vec<(Arc<Home>, u64, Arc<Py<Document>>)>,
}

impl Garbage {
    /// Drops the garbage, after giving every home `repoint` moved a reference of its own. Call it once every lock is
    /// released. Garbage that's only dropped leaves those homes sharing a reference, which works but hides it from
    /// the cycle collector.
    pub(crate) fn release(mut self, py: Python<'_>) {
        for (home, generation, to) in std::mem::take(&mut self.moved) {
            self.old.extend(home.settle(py, generation, &to));
        }
    }
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
pub(crate) fn get<T: PyTypeCheck>(py: Python<'_>, key: u64) -> Option<Bound<'_, T>> {
    let found = {
        let cache = CACHE.lock_py_attached(py).unwrap_or_else(PoisonError::into_inner);
        cache.slots.get(&key).and_then(|slot| slot.object.bind(py).upgrade())
    };
    found?.cast_into::<T>().ok()
}

/// The one handle for `key`, so `doc["a"] is doc["a"]`. `make` builds a handle object for a home in `doc`, and
/// `present` says, under `doc`'s lock, whether `key` is still there. None means it isn't: the caller's view of the
/// document is out of date and it should look again.
pub(crate) fn publish<'py, T: PyClass, I: Into<PyClassInitializer<T>>>(
    doc: &Bound<'py, Document>,
    key: u64,
    make: impl Fn(Arc<Home>) -> I,
    present: impl Fn(&DocState) -> bool + Sync,
) -> PyResult<Option<Bound<'py, T>>> {
    let py = doc.py();
    loop {
        if let Some(object) = get::<T>(py, key) {
            return Ok(Some(object));
        }
        // Python objects are made before locking; the lock only decides whether to publish this one.
        let home = Home::new(doc.clone().unbind());
        let object = Bound::new(py, make(Arc::clone(&home)))?;
        let weak = PyWeakrefReference::new(object.as_any())?.unbind();
        let (found, published, garbage) = doc.get().with_state(py, |s| {
            let mut garbage = Garbage::default();
            if !present(s) {
                garbage.weakrefs.push(weak);
                return (false, false, garbage);
            }
            let published = intern_locked(key, weak, &home, &mut garbage);
            (true, published, garbage)
        });
        garbage.release(py);
        match (found, published) {
            (false, _) => return Ok(None),
            (true, true) => return Ok(Some(object)),
            // Another thread published a handle first, or one is being freed right now.
            (true, false) => py.detach(std::thread::yield_now),
        }
    }
}

/// Handles for every id in `ids`, made by `handle` (a `publish` caller). None if one of them has left `doc` since
/// `ids` was read from it, and the caller should read again.
pub(crate) fn publish_all<'py, I: Copy, T>(
    doc: &Bound<'py, Document>,
    ids: &[I],
    handle: impl Fn(&Bound<'py, Document>, I) -> PyResult<Option<Bound<'py, T>>>,
) -> PyResult<Option<Vec<Bound<'py, T>>>> {
    ids.iter().map(|&id| handle(doc, id)).collect()
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

/// Points the live handles among `keys` at `doc`. Called with the documents locked, alongside the move itself. The
/// caller hands the garbage to `Garbage::release` once it's unlocked.
pub(crate) fn repoint(keys: impl IntoIterator<Item = u64>, doc: &Arc<Py<Document>>, garbage: &mut Garbage) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    for key in keys {
        let Some(home) = cache.slots.get(&key).and_then(|slot| slot.home.upgrade()) else {
            continue;
        };
        let old = std::mem::replace(
            &mut *home.place(),
            Place {
                ptr: Ptr::Shared(Arc::clone(doc)),
                generation,
            },
        );
        garbage.old.push(old.ptr);
        // This may be the last reference if the handle is being freed right now.
        garbage.moved.push((home, generation, Arc::clone(doc)));
    }
}

pub(crate) fn stale() -> PyErr {
    PyRuntimeError::new_err("handle is out of step with its document (a bug in rkdl)")
}

/// Why an operation on a document didn't go through.
pub(crate) enum Fail {
    /// The node or entry wasn't in the document. If the handle's home still points there, the error is the answer;
    /// if a move repointed it meanwhile, try again.
    Missing(PyErr),
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
pub(crate) fn on_home<R>(
    py: Python<'_>,
    home: &Home,
    mut op: impl FnMut(&Py<Document>) -> Result<R, Fail>,
) -> PyResult<R> {
    loop {
        let doc = home.doc(py);
        match op(&doc) {
            Ok(result) => return Ok(result),
            Err(Fail::Err(err)) => return Err(err),
            Err(Fail::Missing(err)) if !home.moved_from(&doc) => return Err(err),
            Err(Fail::Changed | Fail::Missing(_)) => {}
        }
    }
}
