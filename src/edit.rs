//! Reading and editing a node's entries, shared by the `args`, `props` and `entries` views.

use std::sync::Arc;

use kdl::{KdlEntry, KdlIdentifier, KdlNode, KdlValue};
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

use crate::document::{DocState, Document};
use crate::entry::Entry;
use crate::handles::{self, Fail, Garbage, removed, stale};
use crate::node::Node;
use crate::tree::{EntryId, IdTree, NodeId, node_at, node_at_mut};
use crate::values::{self, Encoded, Place, Slot, TypeRegistry};

/// One entry as read under the lock, to work on once it's released.
#[derive(Clone)]
pub(crate) struct Item {
    pub(crate) id: EntryId,
    pub(crate) name: Option<KdlIdentifier>,
    pub(crate) value: KdlValue,
    pub(crate) ty: Option<String>,
    /// The value's text in the file, kept only for annotated values, which are the ones whose text can matter to
    /// reading them (`decimal64`) or to the error saying they can't be read.
    text: Option<String>,
    /// The node's name and, for an argument, its index in `node.args`: where the entry is, for the error when its
    /// value can't be read.
    node: Arc<str>,
    arg: usize,
}

impl Item {
    fn new(id: EntryId, entry: &KdlEntry, node: &Arc<str>, arg: usize) -> Self {
        Item {
            id,
            name: entry.name().cloned(),
            value: entry.value().clone(),
            ty: entry.ty().map(|t| t.value().to_owned()),
            text: entry.ty().and(entry.format()).map(|f| f.value_repr.clone()),
            node: Arc::clone(node),
            arg,
        }
    }

    pub(crate) fn key(&self) -> Option<&str> {
        self.name.as_ref().map(KdlIdentifier::value)
    }

    pub(crate) fn to_python(&self, py: Python<'_>, types: &TypeRegistry) -> PyResult<Py<PyAny>> {
        let place = Place {
            node: &self.node,
            slot: self.key().map_or(Slot::Arg(self.arg), Slot::Prop),
        };
        values::to_python_spelled(
            py,
            &self.value,
            self.text.as_deref(),
            self.ty.as_deref(),
            types,
            Some(&place),
        )
    }

    /// What writing `value` over this entry stores. A value that's the one the entry already reads as is a no-op,
    /// even where converting it would spell it differently (`(date-time)"…Z"` would come back as `+00:00`): if the
    /// annotation stays and the old value, read and written back, gives the same KDL as the new one, the old value
    /// is what's stored, which `store` sees is already there.
    pub(crate) fn encode_over(&self, value: &Bound<'_, PyAny>, types: &TypeRegistry) -> PyResult<Encoded> {
        let new = values::encode(value, self.ty.as_deref(), types)?;
        if new.ty.is_some() && new.ty == self.ty && self.rewrites_as(value.py(), &new, types)? {
            return Ok(Encoded {
                value: self.value.clone(),
                ty: new.ty,
                repr: None,
            });
        }
        Ok(new)
    }

    /// Whether reading this entry and writing the result back would store `new`. A value that can't be read or
    /// written back (an `Exception` from a conversion) isn't `new`; anything else, such as `KeyboardInterrupt`,
    /// propagates.
    fn rewrites_as(&self, py: Python<'_>, new: &Encoded, types: &TypeRegistry) -> PyResult<bool> {
        let again = self
            .to_python(py, types)
            .and_then(|old| values::encode(old.bind(py), self.ty.as_deref(), types));
        match again {
            Ok(again) => {
                Ok(again.ty == new.ty && values::same_value(&again.value, &new.value) && again.repr == new.repr)
            }
            Err(err) if err.is_instance_of::<PyException>(py) => Ok(false),
            Err(err) => Err(err),
        }
    }

    // Values may change between planning an edit and applying it, unless the plan read them. Which entries there are,
    // their keys and their annotations may not, since every plan depends on them.
    fn same_shape(&self, other: &Item) -> bool {
        self.id == other.id && self.name == other.name && self.ty == other.ty
    }

    /// Compares floats by their bits, so a NaN is the same as itself and doesn't retry forever.
    fn same_value(&self, other: &Item) -> bool {
        values::same_value(&self.value, &other.value)
    }

    /// The value's Python repr, for the reprs of entries and views, which don't raise an `Exception`. A value that
    /// doesn't fit its type, or whose Python repr raises one, shows as its KDL text, marked as such: `<kdl: (u8)300>`.
    /// Anything else a decode hook raises, such as `KeyboardInterrupt`, propagates.
    ///
    /// With `ty`, a value that reads shows its annotation in front, as `(u8)7`.
    pub(crate) fn repr(&self, py: Python<'_>, types: &TypeRegistry, ty: bool) -> PyResult<String> {
        let annotation = self.ty.as_deref().map(|t| format!("({})", values::identifier(t)));
        let shown = self
            .to_python(py, types)
            .and_then(|value| Ok(value.bind(py).repr()?.to_string()));
        match shown {
            Ok(shown) if ty => Ok(format!("{}{shown}", annotation.unwrap_or_default())),
            Ok(shown) => Ok(shown),
            Err(err) if err.is_instance_of::<PyException>(py) => {
                let text = self.text.clone().unwrap_or_else(|| values::canonical_repr(&self.value));
                Ok(format!("<kdl: {}{text}>", annotation.unwrap_or_default()))
            }
            Err(err) => Err(err),
        }
    }
}

fn items(s: &DocState, node: NodeId) -> Result<Vec<Item>, Fail> {
    let path = s.ids.path(node).ok_or_else(|| Fail::Missing(stale()))?;
    let kdl = node_at(&s.kdl, &path);
    let name: Arc<str> = kdl.name().value().into();
    let mut args = 0;
    Ok(s.ids
        .entries(node)
        .iter()
        .zip(kdl.entries())
        .map(|(&id, e)| {
            let item = Item::new(id, e, &name, args);
            if e.name().is_none() {
                args += 1;
            }
            item
        })
        .collect())
}

/// One entry, found by its id under its document's lock. If it isn't there and the handle didn't move meanwhile, it
/// was removed.
pub(crate) fn item(s: &DocState, id: EntryId) -> Result<Item, Fail> {
    let (node, index) = s.ids.entry_position(id).ok_or_else(|| Fail::Missing(removed()))?;
    let path = s.ids.path(node).expect("an entry's node is in the tree");
    let kdl = node_at(&s.kdl, &path);
    let entry = kdl.entries().get(index).ok_or_else(|| Fail::Missing(stale()))?;
    let arg = kdl.entries().iter().take(index).filter(|e| e.name().is_none()).count();
    Ok(Item::new(id, entry, &kdl.name().value().into(), arg))
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
    pub(crate) fn insert(&mut self, index: usize, entry: KdlEntry) -> EntryId {
        self.kdl.entries_mut().insert(index, entry);
        self.ids.insert_entry(self.node, index)
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
