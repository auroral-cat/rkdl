//! Reading and editing a node's entries, shared by the `args`, `props` and `entries` views.

use kdl::{KdlEntry, KdlIdentifier, KdlNode, KdlValue};
use pyo3::prelude::*;

use crate::document::{DocState, Document};
use crate::entry::Entry;
use crate::handles::{self, Fail, Garbage, stale};
use crate::node::Node;
use crate::tree::{EntryId, IdTree, NodeId, node_at, node_at_mut};
use crate::values::{self, TypeRegistry};

/// One entry as read under the lock, to work on once it's released.
#[derive(Clone)]
pub(crate) struct Item {
    pub(crate) id: EntryId,
    pub(crate) name: Option<KdlIdentifier>,
    pub(crate) value: KdlValue,
    pub(crate) ty: Option<String>,
}

impl Item {
    pub(crate) fn new(id: EntryId, entry: &KdlEntry) -> Self {
        Item {
            id,
            name: entry.name().cloned(),
            value: entry.value().clone(),
            ty: entry.ty().map(|t| t.value().to_owned()),
        }
    }

    pub(crate) fn key(&self) -> Option<&str> {
        self.name.as_ref().map(KdlIdentifier::value)
    }

    pub(crate) fn to_python(&self, py: Python<'_>, types: &TypeRegistry) -> PyResult<Py<PyAny>> {
        values::to_python(py, &self.value, self.ty.as_deref(), types)
    }

    // Values may change between planning an edit and applying it, unless the plan read them. Which entries there are,
    // their keys and their annotations may not, since every plan depends on them.
    fn same_shape(&self, other: &Item) -> bool {
        self.id == other.id && self.name == other.name && self.ty == other.ty
    }

    /// Compares floats by their bits, so a NaN is the same as itself and doesn't retry forever.
    fn same_value(&self, other: &Item) -> bool {
        match (&self.value, &other.value) {
            (KdlValue::Float(a), KdlValue::Float(b)) => a.to_bits() == b.to_bits(),
            (a, b) => a == b,
        }
    }
}

fn items(s: &DocState, node: NodeId) -> Result<Vec<Item>, Fail> {
    let path = s.ids.path(node).ok_or_else(|| Fail::Missing(stale()))?;
    let entries = node_at(&s.kdl, &path).entries();
    Ok(s.ids
        .entries(node)
        .iter()
        .zip(entries)
        .map(|(&id, e)| Item::new(id, e))
        .collect())
}

/// A node's entries in file order, and the document they were read from.
pub(crate) struct Snapshot {
    pub(crate) doc: Py<Document>,
    pub(crate) items: Vec<Item>,
}

impl Snapshot {
    pub(crate) fn types(&self) -> &TypeRegistry {
        self.doc.get().types.get()
    }
}

/// Reads the node's entries from `doc`, once. The caller runs it under `on_home`, which retries.
fn snapshot(py: Python<'_>, doc: &Py<Document>, node: NodeId) -> Result<Snapshot, Fail> {
    let items = doc.get().with_state(py, |s| items(s, node))?;
    Ok(Snapshot {
        doc: doc.clone_ref(py),
        items,
    })
}

pub(crate) fn read(py: Python<'_>, node: &Node) -> PyResult<Snapshot> {
    handles::on_home(py, &node.home, |doc| snapshot(py, doc, node.id))
}

/// A node's entries, open for editing. Edits go to the `KdlNode` and the `IdTree` together.
pub(crate) struct Target<'a> {
    kdl: &'a mut KdlNode,
    ids: &'a mut IdTree,
    node: NodeId,
    garbage: &'a mut Garbage,
}

impl Target<'_> {
    fn index(&self, id: EntryId) -> usize {
        self.ids.entry_position(id).expect("planned from this node").1
    }

    pub(crate) fn len(&self) -> usize {
        self.kdl.entries().len()
    }

    pub(crate) fn entry_mut(&mut self, id: EntryId) -> &mut KdlEntry {
        let index = self.index(id);
        &mut self.kdl.entries_mut()[index]
    }

    // Plans compute indexes from a snapshot with the same shape, so they're in bounds and KdlNode's panicking
    // insert/remove aren't needed.
    pub(crate) fn insert(&mut self, index: usize, entry: KdlEntry) {
        self.kdl.entries_mut().insert(index, entry);
        self.ids.insert_entry(self.node, index);
    }

    /// Removes the entry, and lets go of the document for its handle, which can only raise now.
    pub(crate) fn remove(&mut self, id: EntryId) {
        let index = self.index(id);
        self.kdl.entries_mut().remove(index);
        self.ids.remove_entry(id);
        handles::remove(id.key(), self.garbage);
    }
}

/// What an edit's plan depends on, which must still hold when it's applied.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reads {
    /// Which entries there are, with their keys and annotations.
    Shape,
    /// Their values too, for a plan that returns or matches one.
    Values,
}

/// Plans an edit from a snapshot of the node's entries, with Python available, then applies the plan under the
/// lock. If what the plan `reads` changed in between, it plans again.
pub(crate) fn edit<P: Send, R: Send>(
    py: Python<'_>,
    node: &Node,
    reads: Reads,
    mut plan: impl FnMut(&Snapshot) -> PyResult<P>,
    apply: impl Fn(Target<'_>, P) -> R + Sync,
) -> PyResult<R> {
    let (result, garbage) = handles::on_home(py, &node.home, |doc| {
        let snapshot = snapshot(py, doc, node.id)?;
        let planned = plan(&snapshot)?;
        // A plan that can't be applied goes back out unapplied: it may hold Python objects, which are dropped only
        // once the lock is released.
        let applied = doc.get().with_state(py, |s| {
            let now = match items(s, node.id) {
                Ok(now) => now,
                Err(fail) => return Err((fail, planned)),
            };
            let same = now.len() == snapshot.items.len()
                && now
                    .iter()
                    .zip(&snapshot.items)
                    .all(|(a, b)| a.same_shape(b) && (reads == Reads::Shape || a.same_value(b)));
            if !same {
                return Err((Fail::Changed, planned));
            }
            let path = s.ids.path(node.id).expect("found by items");
            let mut garbage = Garbage::default();
            let target = Target {
                kdl: node_at_mut(&mut s.kdl, &path),
                ids: &mut s.ids,
                node: node.id,
                garbage: &mut garbage,
            };
            Ok((apply(target, planned), garbage))
        });
        applied.map_err(|(fail, _)| fail)
    })?;
    handles::edited();
    garbage.release(py);
    Ok(result)
}

/// Handles for the entries `choose` picks. If one of them leaves the document before its handle exists, the node
/// is read again.
pub(crate) fn chosen<'py>(
    py: Python<'py>,
    node: &Node,
    choose: impl Fn(&[Item]) -> PyResult<Vec<EntryId>>,
) -> PyResult<Vec<Bound<'py, Entry>>> {
    handles::retry(|| {
        let snapshot = read(py, node)?;
        let ids = choose(&snapshot.items)?;
        handles::publish_all(snapshot.doc.bind(py), &ids, Entry::handle)
    })
}
