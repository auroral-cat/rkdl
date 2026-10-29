// Stubs until task 02 fills them in; task 02 removes this allow.
#![allow(dead_code, unused_variables)]

use std::num::NonZeroU64;

use kdl::{KdlDocument, KdlNode};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(NonZeroU64);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntryId(NonZeroU64);

// Ids come from one global counter, so they're unique across all documents.

/// A node's ids and everything under it, taken out of one IdTree to be put into another.
pub struct Subtree {}

impl Subtree {
    pub fn root(&self) -> NodeId {
        todo!()
    }
}

/// Mirrors the shape of a KdlDocument: same nodes, entries and children blocks, in the same order.
pub struct IdTree {}

impl IdTree {
    pub fn from_document(doc: &KdlDocument) -> Self {
        todo!()
    }

    /// One top-level node: a detached node's home.
    pub fn from_subtree(subtree: Subtree) -> Self {
        todo!()
    }

    pub fn top(&self) -> &[NodeId] {
        todo!()
    }

    pub fn contains(&self, node: NodeId) -> bool {
        todo!()
    }

    /// None for a top-level node.
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        todo!()
    }

    /// None means the node has no children block.
    pub fn children(&self, node: NodeId) -> Option<&[NodeId]> {
        todo!()
    }

    pub fn entries(&self, node: NodeId) -> &[EntryId] {
        todo!()
    }

    /// Indices from the top level down.
    pub fn path(&self, node: NodeId) -> Option<Vec<usize>> {
        todo!()
    }

    pub fn entry_position(&self, entry: EntryId) -> Option<(NodeId, usize)> {
        todo!()
    }

    /// Assigns fresh ids to `node` and its whole subtree. `parent: None` means top level.
    pub fn insert_node(&mut self, parent: Option<NodeId>, index: usize, node: &KdlNode) -> NodeId {
        todo!()
    }

    pub fn take_subtree(&mut self, node: NodeId) -> Subtree {
        todo!()
    }

    pub fn put_subtree(&mut self, parent: Option<NodeId>, index: usize, subtree: Subtree) {
        todo!()
    }

    /// None -> Some(empty).
    pub fn open_block(&mut self, node: NodeId) {
        todo!()
    }

    /// Some -> None. Returns the removed children.
    pub fn close_block(&mut self, node: NodeId) -> Vec<Subtree> {
        todo!()
    }

    pub fn insert_entry(&mut self, node: NodeId, index: usize) -> EntryId {
        todo!()
    }

    pub fn remove_entry(&mut self, entry: EntryId) {
        todo!()
    }

    /// Panics with a description of the first difference. Called after every edit in debug builds.
    pub fn check_shape(&self, doc: &KdlDocument) {
        todo!()
    }
}

pub fn node_at<'a>(doc: &'a KdlDocument, path: &[usize]) -> &'a KdlNode {
    todo!()
}

pub fn node_at_mut<'a>(doc: &'a mut KdlDocument, path: &[usize]) -> &'a mut KdlNode {
    todo!()
}
