//! The process-wide handle cache, and how handles find their document.
//!
//! Ids are global, so one cache covers every document and a move never has to move cache entries. What a move does
//! change is each handle's document pointer. That pointer lives in a `Home` shared between the handle and its cache
//! slot, so a move can repoint handles while the documents are still locked, without calling Python. A reader that
//! raced a move sees its home's generation change and retries.
//!
//! The cache drops a slot when its handle dies, from a callback on the slot's weakref. The cache keeps that weakref
//! alive, so Python runs the callback even when the handle dies in a garbage cycle, and runs it before the cycle's
//! finalizers. A finalizer that looks the node up again gets a fresh handle. The old one, which only finalizers can
//! still see, is never repointed again and goes stale.
//!
//! Lock order: document locks, then the cache, then a `Home`. Nothing takes a document lock while holding the cache.

use std::cell::Cell;
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
    /// The handle's entry was removed. Holding the document would keep all of it alive for a handle that can only
    /// raise.
    Gone,
}

impl Ptr {
    fn doc(&self) -> Option<&Py<Document>> {
        match self {
            Ptr::Own(doc) => Some(doc),
            Ptr::Shared(doc) => Some(doc),
            Ptr::Gone => None,
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

    /// None once the handle's entry was removed.
    pub(crate) fn doc(&self, py: Python<'_>) -> Option<DocRef> {
        let place = self.place();
        Some(DocRef {
            doc: place.ptr.doc()?.clone_ref(py),
            generation: place.generation,
        })
    }

    /// Whether a move has repointed this home since `doc` was read from it. A handle that moved away and back still
    /// counts as moved.
    pub(crate) fn moved_from(&self, doc: &DocRef) -> bool {
        self.place().generation != doc.generation
    }

    /// Tells the cycle collector about the document, for the handle that owns this home.
    pub(crate) fn traverse(&self, visit: &PyVisit<'_>) -> Result<(), PyTraverseError> {
        // A move on another thread may be repointing this home right now, and traversal mustn't wait for it. Not
        // reporting the document then is safe: the mover holds the old document on its stack for the whole move, so
        // the collector can't free it as garbage meanwhile.
        let place = match self.0.try_lock() {
            Ok(place) => place,
            // unreachable: a home's lock is poisoned only by a panic while it's held, and nothing under it can panic
            // (its holders only swap a Place, compare a generation or clone_ref)
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            // unreachable: a race no test can make happen on demand: the collector traverses a handle on one thread
            // while a move on another holds that handle's home lock in repoint, which takes nanoseconds and never
            // runs Python. A probe hits it: loops/coverage/runs/round3/handles/probe-traverse-race.py in the wiki
            // (two threads calling gc.collect() while a block node with 500 handled children moves between two
            // documents, a few hits in 5 s; moving one child at a time hits it far less). Skipping and waiting give
            // the same result there (the holders run detached and need nothing to finish), so a test couldn't fail
            Err(TryLockError::WouldBlock) => return Ok(()),
        };
        match &place.ptr {
            Ptr::Own(doc) => visit.call(doc),
            Ptr::Shared(_) | Ptr::Gone => Ok(()),
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
    /// released.
    pub(crate) fn release(mut self, py: Python<'_>) {
        self.settle(py);
    }

    fn settle(&mut self, py: Python<'_>) {
        for (home, generation, to) in std::mem::take(&mut self.moved) {
            self.old.extend(home.settle(py, generation, &to));
        }
    }
}

impl Drop for Garbage {
    /// Only does anything when a panic skipped `release`. Homes left sharing a reference would hide their document
    /// from the cycle collector for good, so this settles them anyway. The panic poisoned the documents it had
    /// locked, but a node it moved to a new private home may still be usable.
    fn drop(&mut self) {
        if !self.moved.is_empty() {
            Python::try_attach(|py| self.settle(py));
        }
    }
}

struct Slot {
    object: Py<PyWeakrefReference>,
    // How `repoint` reaches the handle's home without calling Python. Dead once the handle is gone.
    home: Weak<Home>,
}

static CACHE: LazyLock<Mutex<HashMap<u64, Slot>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The callback on a slot's weakref, which drops the slot once its handle is gone.
#[pyclass(module = "rkdl", frozen)]
struct Forget(u64);

#[pymethods]
impl Forget {
    fn __call__(&self, weakref: &Bound<'_, PyAny>) {
        let dead = {
            let mut slots = CACHE
                .lock_py_attached(weakref.py())
                .unwrap_or_else(PoisonError::into_inner);
            // A handle's home is freed before its weakref is cleared, so a new handle may have the slot already.
            match slots.get(&self.0) {
                Some(slot) if slot.object.as_ptr() == weakref.as_ptr() => slots.remove(&self.0),
                _ => None,
            }
        };
        drop(dead);
    }
}

/// The live handle for `key`, if there is one.
pub(crate) fn get<T: PyTypeCheck>(py: Python<'_>, key: u64) -> Option<Bound<'_, T>> {
    let found = {
        let slots = CACHE.lock_py_attached(py).unwrap_or_else(PoisonError::into_inner);
        slots.get(&key).and_then(|slot| slot.object.bind(py).upgrade())
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
        let weak = PyWeakrefReference::new_with(object.as_any(), Forget(key))?.unbind();
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
            // Another thread published a handle first, or one is being freed right now and its slot is about to go.
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
    let mut slots = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if slots.get(&key).is_some_and(|slot| slot.home.strong_count() > 0) {
        garbage.weakrefs.push(object);
        return false;
    }
    let slot = Slot {
        object,
        home: Arc::downgrade(home),
    };
    // A dead slot is still here between freeing a handle's home and running its weakref's callback.
    if let Some(dead) = slots.insert(key, slot) {
        garbage.weakrefs.push(dead.object);
    }
    true
}

/// Points the live handles among `keys` at `doc`. Called with the documents locked, alongside the move itself. The
/// caller hands the garbage to `Garbage::release` once it's unlocked.
pub(crate) fn repoint(keys: impl IntoIterator<Item = u64>, doc: &Arc<Py<Document>>, garbage: &mut Garbage) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let slots = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    for key in keys {
        let Some(home) = slots.get(&key).and_then(|slot| slot.home.upgrade()) else {
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

/// Empties the home of the removed entry `key`'s handle, if it has one. Called with the entry's document locked,
/// alongside the removal. The new generation stops a move that's about to settle the home from refilling it.
pub(crate) fn remove(key: u64, garbage: &mut Garbage) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let slots = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(home) = slots.get(&key).and_then(|slot| slot.home.upgrade()) else {
        return;
    };
    let old = std::mem::replace(
        &mut *home.place(),
        Place {
            ptr: Ptr::Gone,
            generation,
        },
    );
    garbage.old.push(old.ptr);
}

pub(crate) fn removed() -> PyErr {
    PyRuntimeError::new_err("entry was removed")
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

thread_local! {
    /// Edits made on this thread. Python callbacks run on the thread that called them, so an operation that sees this
    /// change while it runs knows its own callbacks edited something.
    static EDITS: Cell<u64> = const { Cell::new(0) };
}

/// Records an edit on this thread. Every edit that can change what an operation planned from calls it.
pub(crate) fn edited() {
    EDITS.set(EDITS.get().wrapping_add(1));
}

/// How many times an operation starts over after its own Python callbacks (a type's decode or encode, `__eq__`,
/// `__index__`) edited something. A callback that changes the document every time would otherwise loop forever. A
/// change from another thread doesn't count: under contention an operation keeps retrying, as long as it takes.
const RETRIES: usize = 64;

/// Runs `attempt` until it returns something, giving up once its own callbacks have made it start over `RETRIES`
/// times.
pub(crate) fn retry<R>(mut attempt: impl FnMut() -> PyResult<Option<R>>) -> PyResult<R> {
    let mut own = 0;
    loop {
        let before = EDITS.get();
        if let Some(result) = attempt()? {
            return Ok(result);
        }
        if EDITS.get() != before {
            own += 1;
            if own == RETRIES {
                return Err(PyRuntimeError::new_err("the document kept changing during this edit"));
            }
        }
    }
}

/// Runs `op` against the document `home` points at, again if the handle moved or the document changed while `op`
/// ran.
pub(crate) fn on_home<R>(
    py: Python<'_>,
    home: &Home,
    mut op: impl FnMut(&Py<Document>) -> Result<R, Fail>,
) -> PyResult<R> {
    retry(|| {
        // Only an entry's home empties, when the entry is removed.
        let doc = home.doc(py).ok_or_else(removed)?;
        match op(&doc) {
            Ok(result) => Ok(Some(result)),
            Err(Fail::Err(err)) => Err(err),
            Err(Fail::Missing(err)) if !home.moved_from(&doc) => Err(err),
            Err(Fail::Changed | Fail::Missing(_)) => Ok(None),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{empty_doc, with_python};

    // `Fail::Missing` from a home that didn't move is the answer (its doc): the error comes out after one try. Retrying
    // it instead would loop forever, since nothing changes.
    #[test]
    fn missing_from_an_unmoved_home_is_the_answer() {
        with_python(|py| {
            let home = Home::new(empty_doc(py));
            let mut tries = 0;
            let result: PyResult<()> = on_home(py, &home, |_| {
                tries += 1;
                Err(Fail::Missing(stale()))
            });
            let err = result.expect_err("the error is the answer");
            assert!(err.is_instance_of::<PyRuntimeError>(py));
            assert!(err.to_string().contains("out of step with its document"), "{err}");
            assert_eq!(tries, 1);
        });
    }

    // A `Garbage` dropped without `release` (a panic skipped it) still gives each home `repoint` moved a reference of
    // its own, so the document doesn't stay hidden from the collector.
    #[test]
    fn garbage_dropped_without_release_settles_its_homes() {
        with_python(|py| {
            let home = Home::new(empty_doc(py));
            let to = Arc::new(empty_doc(py));
            let mut garbage = Garbage::default();
            repoint_one(&home, &to, &mut garbage);
            assert!(matches!(&home.place().ptr, Ptr::Shared(_)));
            drop(garbage);
            assert!(matches!(&home.place().ptr, Ptr::Own(doc) if doc.is(&*to)));
        });
    }

    /// What `repoint` does to one live handle's home.
    fn repoint_one(home: &Arc<Home>, to: &Arc<Py<Document>>, garbage: &mut Garbage) {
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
        let old = std::mem::replace(
            &mut *home.place(),
            Place {
                ptr: Ptr::Shared(Arc::clone(to)),
                generation,
            },
        );
        garbage.old.push(old.ptr);
        garbage.moved.push((Arc::clone(home), generation, Arc::clone(to)));
    }
}
