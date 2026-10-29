use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use kdl::{KdlDocument, KdlNode};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(NonZeroU64);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntryId(NonZeroU64);

// Ids come from one global counter, so they're unique across all documents.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> NonZeroU64 {
    NonZeroU64::new(NEXT_ID.fetch_add(1, Ordering::Relaxed)).expect("id counter overflowed")
}

impl NodeId {
    fn fresh() -> Self {
        NodeId(next_id())
    }

    /// The id as a plain integer, for keying the handle cache.
    pub fn key(self) -> u64 {
        self.0.get()
    }
}

impl EntryId {
    fn fresh() -> Self {
        EntryId(next_id())
    }

    /// The id as a plain integer. Node and entry ids share one counter, so keys never collide.
    pub fn key(self) -> u64 {
        self.0.get()
    }
}

struct NodeSlot {
    parent: Option<NodeId>,
    children: Option<Vec<NodeId>>,
    entries: Vec<EntryId>,
}

/// A node's ids and everything under it, taken out of one IdTree to be put into another.
pub struct Subtree {
    root: NodeId,
    nodes: HashMap<NodeId, NodeSlot>,
    entries: HashMap<EntryId, NodeId>,
}

impl Subtree {
    /// Handle cache keys for every node and entry in the subtree.
    pub fn keys(&self) -> Vec<u64> {
        let nodes = self.nodes.keys().map(|n| n.key());
        nodes.chain(self.entries.keys().map(|e| e.key())).collect()
    }
}

#[cfg(test)]
impl Subtree {
    pub fn root(&self) -> NodeId {
        self.root
    }
}

/// Mirrors the shape of a KdlDocument: same nodes, entries and children blocks, in the same order.
pub struct IdTree {
    top: Vec<NodeId>,
    nodes: HashMap<NodeId, NodeSlot>,
    entries: HashMap<EntryId, NodeId>,
}

impl IdTree {
    fn empty() -> Self {
        IdTree {
            top: Vec::new(),
            nodes: HashMap::new(),
            entries: HashMap::new(),
        }
    }

    pub fn from_document(doc: &KdlDocument) -> Self {
        let mut tree = IdTree::empty();
        let top = doc.nodes().iter().map(|n| tree.add_node(None, n)).collect();
        tree.top = top;
        tree
    }

    /// One top-level node: a detached node's home.
    pub fn from_subtree(subtree: Subtree) -> Self {
        let mut tree = IdTree::empty();
        tree.put_subtree(None, 0, subtree);
        tree
    }

    fn add_node(&mut self, parent: Option<NodeId>, node: &KdlNode) -> NodeId {
        let id = NodeId::fresh();
        let entries: Vec<EntryId> = node.entries().iter().map(|_| EntryId::fresh()).collect();
        for &e in &entries {
            self.entries.insert(e, id);
        }
        let children = node
            .children()
            .map(|c| c.nodes().iter().map(|n| self.add_node(Some(id), n)).collect());
        self.nodes.insert(
            id,
            NodeSlot {
                parent,
                children,
                entries,
            },
        );
        id
    }

    fn slot(&self, node: NodeId) -> &NodeSlot {
        self.nodes.get(&node).expect("node is not in this tree")
    }

    fn slot_mut(&mut self, node: NodeId) -> &mut NodeSlot {
        self.nodes.get_mut(&node).expect("node is not in this tree")
    }

    fn siblings_mut(&mut self, parent: Option<NodeId>) -> &mut Vec<NodeId> {
        match parent {
            None => &mut self.top,
            Some(p) => self
                .slot_mut(p)
                .children
                .as_mut()
                .expect("parent has no children block"),
        }
    }

    pub fn top(&self) -> &[NodeId] {
        &self.top
    }

    pub fn contains(&self, node: NodeId) -> bool {
        self.nodes.contains_key(&node)
    }

    /// None for a top-level node.
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.slot(node).parent
    }

    /// None means the node has no children block.
    pub fn children(&self, node: NodeId) -> Option<&[NodeId]> {
        self.slot(node).children.as_deref()
    }

    pub fn entries(&self, node: NodeId) -> &[EntryId] {
        &self.slot(node).entries
    }

    /// Indices from the top level down.
    pub fn path(&self, node: NodeId) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        let mut current = node;
        loop {
            let parent = self.nodes.get(&current)?.parent;
            let siblings = match parent {
                None => &self.top[..],
                Some(p) => self.children(p).expect("parent of a node has a children block"),
            };
            path.push(
                siblings
                    .iter()
                    .position(|&n| n == current)
                    .expect("node is missing from its parent"),
            );
            match parent {
                None => break,
                Some(p) => current = p,
            }
        }
        path.reverse();
        Some(path)
    }

    pub fn entry_position(&self, entry: EntryId) -> Option<(NodeId, usize)> {
        let &node = self.entries.get(&entry)?;
        let index = self
            .entries(node)
            .iter()
            .position(|&e| e == entry)
            .expect("entry is missing from its node");
        Some((node, index))
    }

    /// Assigns fresh ids to `node` and its whole subtree. `parent: None` means top level. The handles insert through
    /// `put_subtree` instead, so only tests build trees this way.
    #[cfg(test)]
    pub fn insert_node(&mut self, parent: Option<NodeId>, index: usize, node: &KdlNode) -> NodeId {
        let id = self.add_node(parent, node);
        self.siblings_mut(parent).insert(index, id);
        id
    }

    pub fn take_subtree(&mut self, node: NodeId) -> Subtree {
        let parent = self.parent(node);
        let siblings = self.siblings_mut(parent);
        let index = siblings
            .iter()
            .position(|&n| n == node)
            .expect("node is missing from its parent");
        siblings.remove(index);

        let mut subtree = Subtree {
            root: node,
            nodes: HashMap::new(),
            entries: HashMap::new(),
        };
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            let mut slot = self.nodes.remove(&id).expect("descendant is not in this tree");
            if id == node {
                slot.parent = None;
            }
            for e in &slot.entries {
                self.entries.remove(e);
                subtree.entries.insert(*e, id);
            }
            stack.extend(slot.children.iter().flatten());
            subtree.nodes.insert(id, slot);
        }
        subtree
    }

    pub fn put_subtree(&mut self, parent: Option<NodeId>, index: usize, subtree: Subtree) {
        let root = subtree.root;
        self.siblings_mut(parent).insert(index, root);
        self.nodes.extend(subtree.nodes);
        self.entries.extend(subtree.entries);
        self.slot_mut(root).parent = parent;
    }

    /// None -> Some(empty).
    pub fn open_block(&mut self, node: NodeId) {
        let slot = self.slot_mut(node);
        assert!(slot.children.is_none(), "node already has a children block");
        slot.children = Some(Vec::new());
    }

    /// Some(empty) -> None. The children go first, with `take_subtree`.
    pub fn close_block(&mut self, node: NodeId) {
        let slot = self.slot_mut(node);
        assert!(
            slot.children.as_ref().is_some_and(Vec::is_empty),
            "node has no empty children block"
        );
        slot.children = None;
    }

    pub fn insert_entry(&mut self, node: NodeId, index: usize) -> EntryId {
        let id = EntryId::fresh();
        self.slot_mut(node).entries.insert(index, id);
        self.entries.insert(id, node);
        id
    }

    pub fn remove_entry(&mut self, entry: EntryId) {
        let (node, index) = self.entry_position(entry).expect("entry is not in this tree");
        self.slot_mut(node).entries.remove(index);
        self.entries.remove(&entry);
    }

    /// Panics with a description of the first difference. Called after every edit in debug builds.
    pub fn check_shape(&self, doc: &KdlDocument) {
        self.check_level(&self.top, doc, &mut Vec::new());
    }

    fn check_level(&self, ids: &[NodeId], doc: &KdlDocument, path: &mut Vec<usize>) {
        let nodes = doc.nodes();
        assert!(
            ids.len() == nodes.len(),
            "shape mismatch at {path:?}: tree has {} nodes, document has {}",
            ids.len(),
            nodes.len()
        );
        for (i, (&id, node)) in ids.iter().zip(nodes).enumerate() {
            path.push(i);
            let slot = self.slot(id);
            assert!(
                slot.entries.len() == node.entries().len(),
                "shape mismatch at {path:?}: tree has {} entries, document has {}",
                slot.entries.len(),
                node.entries().len()
            );
            #[expect(
                clippy::panic,
                reason = "a shape mismatch is an rkdl bug, and this only runs in debug builds"
            )]
            match (&slot.children, node.children()) {
                (None, None) => {}
                (Some(ids), Some(doc)) => self.check_level(ids, doc, path),
                (tree, doc) => panic!(
                    "shape mismatch at {path:?}: tree has children block {}, document has {}",
                    tree.is_some(),
                    doc.is_some()
                ),
            }
            path.pop();
        }
    }
}

pub fn node_at<'a>(doc: &'a KdlDocument, path: &[usize]) -> &'a KdlNode {
    let (&first, rest) = path.split_first().expect("empty node path");
    let mut node = &doc.nodes()[first];
    for &i in rest {
        node = &node
            .children()
            .expect("path goes through a node without children")
            .nodes()[i];
    }
    node
}

pub fn node_at_mut<'a>(doc: &'a mut KdlDocument, path: &[usize]) -> &'a mut KdlNode {
    let (&first, rest) = path.split_first().expect("empty node path");
    let mut node = &mut doc.nodes_mut()[first];
    for &i in rest {
        node = &mut node
            .children_mut()
            .as_mut()
            .expect("path goes through a node without children")
            .nodes_mut()[i];
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> KdlDocument {
        KdlDocument::parse(text).unwrap()
    }

    fn node(text: &str) -> KdlNode {
        parse(text).nodes()[0].clone()
    }

    fn at(tree: &IdTree, path: &[usize]) -> NodeId {
        let (&first, rest) = path.split_first().unwrap();
        let mut id = tree.top()[first];
        for &i in rest {
            id = tree.children(id).unwrap()[i];
        }
        id
    }

    #[test]
    fn from_document_matches_shape() {
        let doc = parse("// header\na 1 2 x=3 {\n    b {\n        c y=1 2\n    }\n    d {}\n}\ne\nf {}\n/-g\n");
        let tree = IdTree::from_document(&doc);
        tree.check_shape(&doc);
        assert_eq!(tree.top().len(), 3);
        let a = at(&tree, &[0]);
        assert_eq!(tree.entries(a).len(), 3);
        assert_eq!(tree.children(a).unwrap().len(), 2);
        assert_eq!(tree.children(at(&tree, &[0, 1])), Some(&[][..]));
        assert_eq!(tree.children(at(&tree, &[1])), None);
        assert_eq!(tree.children(at(&tree, &[2])), Some(&[][..]));
        let c = at(&tree, &[0, 0, 0]);
        assert_eq!(tree.entries(c).len(), 2);
        assert_eq!(tree.path(c), Some(vec![0, 0, 0]));
        assert_eq!(tree.parent(c), Some(at(&tree, &[0, 0])));
        assert_eq!(tree.parent(a), None);
    }

    #[test]
    fn ids_are_unique_across_trees() {
        let doc = parse("a 1\n");
        let t1 = IdTree::from_document(&doc);
        let t2 = IdTree::from_document(&doc);
        assert_ne!(t1.top()[0], t2.top()[0]);
        assert_ne!(t1.entries(t1.top()[0])[0], t2.entries(t2.top()[0])[0]);
        assert!(!t2.contains(t1.top()[0]));
    }

    #[test]
    fn insert_top_level() {
        let mut doc = parse("a\nb\n");
        let mut tree = IdTree::from_document(&doc);
        let (a, b) = (tree.top()[0], tree.top()[1]);

        for (index, text) in [(0, "first 1"), (2, "middle x=1 {\n    k\n}"), (4, "last")] {
            let new = node(text);
            let id = tree.insert_node(None, index, &new);
            doc.nodes_mut().insert(index, new);
            tree.check_shape(&doc);
            assert_eq!(tree.path(id), Some(vec![index]));
        }
        assert_eq!(tree.path(a), Some(vec![1]));
        assert_eq!(tree.path(b), Some(vec![3]));
        let middle = tree.top()[2];
        assert_eq!(tree.path(tree.children(middle).unwrap()[0]), Some(vec![2, 0]));
        assert_eq!(node_at(&doc, &[2, 0]).name().value(), "k");
    }

    #[test]
    fn insert_in_children() {
        let mut doc = parse("p {\n    a\n    b\n}\n");
        let mut tree = IdTree::from_document(&doc);
        let p = tree.top()[0];
        let (a, b) = (tree.children(p).unwrap()[0], tree.children(p).unwrap()[1]);

        for (index, text) in [(0, "first"), (2, "middle 1 2"), (4, "last {}")] {
            let new = node(text);
            let id = tree.insert_node(Some(p), index, &new);
            node_at_mut(&mut doc, &[0])
                .ensure_children()
                .nodes_mut()
                .insert(index, new);
            tree.check_shape(&doc);
            assert_eq!(tree.path(id), Some(vec![0, index]));
            assert_eq!(tree.parent(id), Some(p));
        }
        assert_eq!(tree.path(a), Some(vec![0, 1]));
        assert_eq!(tree.path(b), Some(vec![0, 3]));
        assert_eq!(node_at(&doc, &[0, 2]).name().value(), "middle");
    }

    #[test]
    fn move_subtree_between_trees() {
        let mut src_doc = parse("a\np 1 {\n    q x=1 {\n        r\n    }\n}\nz\n");
        let mut src = IdTree::from_document(&src_doc);
        let mut dst_doc = parse("m {\n    n\n}\n");
        let mut dst = IdTree::from_document(&dst_doc);

        let p = src.top()[1];
        let q = src.children(p).unwrap()[0];
        let r = src.children(q).unwrap()[0];
        let p_entry = src.entries(p)[0];
        let q_entry = src.entries(q)[0];
        let m = dst.top()[0];

        let subtree = src.take_subtree(p);
        let kdl = src_doc.nodes_mut().remove(1);
        assert_eq!(subtree.root(), p);
        src.check_shape(&src_doc);
        for id in [p, q, r] {
            assert!(!src.contains(id));
            assert_eq!(src.path(id), None);
        }
        assert_eq!(src.entry_position(p_entry), None);
        assert_eq!(src.entry_position(q_entry), None);
        assert_eq!(src.path(src.top()[1]), Some(vec![1]));

        dst.put_subtree(Some(m), 1, subtree);
        node_at_mut(&mut dst_doc, &[0])
            .ensure_children()
            .nodes_mut()
            .insert(1, kdl);
        dst.check_shape(&dst_doc);
        for id in [p, q, r] {
            assert!(dst.contains(id));
        }
        assert_eq!(dst.parent(p), Some(m));
        assert_eq!(dst.path(p), Some(vec![0, 1]));
        assert_eq!(dst.path(r), Some(vec![0, 1, 0, 0]));
        assert_eq!(dst.entry_position(q_entry), Some((q, 0)));
        assert_eq!(dst.entry_position(p_entry), Some((p, 0)));
    }

    #[test]
    fn detached_tree_from_subtree() {
        let doc = parse("a {\n    b 1\n}\n");
        let mut tree = IdTree::from_document(&doc);
        let b = tree.children(tree.top()[0]).unwrap()[0];
        let home = IdTree::from_subtree(tree.take_subtree(b));
        assert_eq!(home.top(), &[b]);
        assert_eq!(home.parent(b), None);
        assert_eq!(home.path(b), Some(vec![0]));
        assert_eq!(home.entries(b).len(), 1);
    }

    #[test]
    fn open_and_close_block() {
        let mut doc = parse("a\nb {\n    c\n    d {\n        e\n    }\n}\n");
        let mut tree = IdTree::from_document(&doc);
        let a = tree.top()[0];
        tree.open_block(a);
        node_at_mut(&mut doc, &[0]).ensure_children();
        tree.check_shape(&doc);
        assert_eq!(tree.children(a), Some(&[][..]));

        tree.close_block(a);
        node_at_mut(&mut doc, &[0]).clear_children();
        tree.check_shape(&doc);
        assert_eq!(tree.children(a), None);

        let b = tree.top()[1];
        let (c, d) = (at(&tree, &[1, 0]), at(&tree, &[1, 1]));
        let e = at(&tree, &[1, 1, 0]);
        let removed: Vec<Subtree> = [c, d].into_iter().map(|n| tree.take_subtree(n)).collect();
        tree.close_block(b);
        node_at_mut(&mut doc, &[1]).clear_children();
        tree.check_shape(&doc);
        assert_eq!(removed.iter().map(Subtree::root).collect::<Vec<_>>(), vec![c, d]);
        for id in [c, d, e] {
            assert!(!tree.contains(id));
        }
        let d_keys = removed[1].keys();
        assert!(d_keys.contains(&d.key()) && d_keys.contains(&e.key()) && !d_keys.contains(&c.key()));
        let home = IdTree::from_subtree(removed.into_iter().nth(1).unwrap());
        assert_eq!(home.path(e), Some(vec![0, 0]));
    }

    #[test]
    fn entries_insert_and_remove() {
        let mut doc = parse("a 1 x=2 3\n");
        let mut tree = IdTree::from_document(&doc);
        let a = tree.top()[0];
        let old = tree.entries(a).to_vec();
        for (i, &e) in old.iter().enumerate() {
            assert_eq!(tree.entry_position(e), Some((a, i)));
        }

        let new = tree.insert_entry(a, 1);
        node_at_mut(&mut doc, &[0])
            .entries_mut()
            .insert(1, kdl::KdlEntry::new(9));
        tree.check_shape(&doc);
        assert_eq!(tree.entry_position(new), Some((a, 1)));
        assert_eq!(tree.entry_position(old[1]), Some((a, 2)));

        tree.remove_entry(old[0]);
        node_at_mut(&mut doc, &[0]).entries_mut().remove(0);
        tree.check_shape(&doc);
        assert_eq!(tree.entry_position(old[0]), None);
        assert_eq!(tree.entry_position(new), Some((a, 0)));
        assert_eq!(tree.entry_position(old[2]), Some((a, 2)));
    }

    #[test]
    #[should_panic(expected = "shape mismatch at [0, 0]")]
    fn check_shape_finds_entry_mismatch() {
        let doc = parse("a {\n    b 1\n}\n");
        let tree = IdTree::from_document(&doc);
        tree.check_shape(&parse("a {\n    b 1 2\n}\n"));
    }

    #[test]
    #[should_panic(expected = "children block")]
    fn check_shape_finds_block_mismatch() {
        let doc = parse("a\n");
        let tree = IdTree::from_document(&doc);
        tree.check_shape(&parse("a {}\n"));
    }

    #[test]
    #[should_panic(expected = "tree has 1 nodes, document has 2")]
    fn check_shape_finds_count_mismatch() {
        let doc = parse("a\n");
        let tree = IdTree::from_document(&doc);
        tree.check_shape(&parse("a\nb\n"));
    }
}
