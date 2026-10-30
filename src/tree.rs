use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use kdl::{KdlDocument, KdlNode};

use crate::positions::{self, Origin, Pos};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct NodeId(NonZeroU64);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct EntryId(NonZeroU64);

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
    pub(crate) fn key(self) -> u64 {
        self.0.get()
    }
}

impl EntryId {
    fn fresh() -> Self {
        EntryId(next_id())
    }

    /// The id as a plain integer. Node and entry ids share one counter, so keys never collide.
    pub(crate) fn key(self) -> u64 {
        self.0.get()
    }
}

struct NodeSlot {
    parent: Option<NodeId>,
    children: Option<Vec<NodeId>>,
    entries: Vec<EntryId>,
}

/// A node's ids and everything under it, taken out of one `IdTree` to be put into another.
pub(crate) struct Subtree {
    root: NodeId,
    nodes: HashMap<NodeId, NodeSlot>,
    entries: HashMap<EntryId, NodeId>,
    /// The parse the positions came from, if any.
    origin: Option<Origin>,
    positions: HashMap<u64, Pos>,
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
    pub(crate) fn root(&self) -> NodeId {
        self.root
    }
}

/// Mirrors the shape of a `KdlDocument`: same nodes, entries and children blocks, in the same order.
pub(crate) struct IdTree {
    top: Vec<NodeId>,
    nodes: HashMap<NodeId, NodeSlot>,
    entries: HashMap<EntryId, NodeId>,
    /// The parse this tree's positions come from. None for a document that wasn't parsed, which has none.
    origin: Option<Origin>,
    /// Where each parsed node and entry was in the text, by id key. Nodes and entries the parse didn't make have none.
    positions: HashMap<u64, Pos>,
}

impl IdTree {
    fn empty() -> Self {
        IdTree {
            top: Vec::new(),
            nodes: HashMap::new(),
            entries: HashMap::new(),
            origin: None,
            positions: HashMap::new(),
        }
    }

    /// A tree without positions, for a document that wasn't parsed from text.
    pub(crate) fn from_document(doc: &KdlDocument) -> Self {
        IdTree::build(doc, None)
    }

    /// A tree for a document just parsed from `text`, with every node's and entry's position in it.
    pub(crate) fn from_parse(doc: &KdlDocument, text: &str) -> Self {
        let mut spans = Vec::new();
        let mut tree = IdTree::build(doc, Some(&mut spans));
        tree.origin = Some(Origin::fresh());
        tree.positions = positions::locate(text, spans);
        tree
    }

    /// `spans` gets each node's and entry's start offset, by id key, if given.
    fn build(doc: &KdlDocument, mut spans: Option<&mut Vec<(usize, u64)>>) -> Self {
        let mut tree = IdTree::empty();
        // A work list rather than recursion: documents built through the API can nest deeper than the stack goes.
        let mut pending = Vec::new();
        tree.top = tree.add_level(None, doc, &mut pending, spans.as_deref_mut());
        while let Some((id, block)) = pending.pop() {
            let children = tree.add_level(Some(id), block, &mut pending, spans.as_deref_mut());
            tree.slot_mut(id).children = Some(children);
        }
        tree
    }

    /// One top-level node: a detached node's home. It keeps the positions of the document the node came from.
    pub(crate) fn from_subtree(subtree: Subtree) -> Self {
        let mut tree = IdTree::empty();
        tree.origin = subtree.origin;
        tree.put_subtree(None, 0, subtree);
        tree
    }

    /// Ids for the nodes of one block. Their children blocks start empty and go on `pending` to be filled.
    fn add_level<'a>(
        &mut self,
        parent: Option<NodeId>,
        block: &'a KdlDocument,
        pending: &mut Vec<(NodeId, &'a KdlDocument)>,
        mut spans: Option<&mut Vec<(usize, u64)>>,
    ) -> Vec<NodeId> {
        let mut ids = Vec::with_capacity(block.nodes().len());
        for node in block.nodes() {
            let id = NodeId::fresh();
            let entries: Vec<EntryId> = node.entries().iter().map(|_| EntryId::fresh()).collect();
            for &e in &entries {
                self.entries.insert(e, id);
            }
            if let Some(spans) = spans.as_deref_mut() {
                // A node's span starts at its type annotation's `(`, or its name; an entry's at its first character.
                spans.push((node.span().offset(), id.key()));
                let entry_spans = node.entries().iter().map(|e| e.span().offset());
                spans.extend(entry_spans.zip(entries.iter().map(|e| e.key())));
            }
            if let Some(children) = node.children() {
                pending.push((id, children));
            }
            self.nodes.insert(
                id,
                NodeSlot {
                    parent,
                    children: node.children().map(|_| Vec::new()),
                    entries,
                },
            );
            ids.push(id);
        }
        ids
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

    pub(crate) fn top(&self) -> &[NodeId] {
        &self.top
    }

    pub(crate) fn contains(&self, node: NodeId) -> bool {
        self.nodes.contains_key(&node)
    }

    /// None for a top-level node.
    pub(crate) fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.slot(node).parent
    }

    /// None means the node has no children block.
    pub(crate) fn children(&self, node: NodeId) -> Option<&[NodeId]> {
        self.slot(node).children.as_deref()
    }

    pub(crate) fn entries(&self, node: NodeId) -> &[EntryId] {
        &self.slot(node).entries
    }

    /// Indices from the top level down.
    pub(crate) fn path(&self, node: NodeId) -> Option<Vec<usize>> {
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

    /// Where the node or entry with this id key was in the text the document was parsed from, if the parse made it.
    pub(crate) fn pos(&self, key: u64) -> Option<Pos> {
        self.positions.get(&key).copied()
    }

    pub(crate) fn entry_position(&self, entry: EntryId) -> Option<(NodeId, usize)> {
        let &node = self.entries.get(&entry)?;
        let index = self
            .entries(node)
            .iter()
            .position(|&e| e == entry)
            .expect("entry is missing from its node");
        Some((node, index))
    }

    pub(crate) fn take_subtree(&mut self, node: NodeId) -> Subtree {
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
            origin: self.origin,
            positions: HashMap::new(),
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
                if let Some(pos) = self.positions.remove(&e.key()) {
                    subtree.positions.insert(e.key(), pos);
                }
            }
            if let Some(pos) = self.positions.remove(&id.key()) {
                subtree.positions.insert(id.key(), pos);
            }
            stack.extend(slot.children.iter().flatten());
            subtree.nodes.insert(id, slot);
        }
        subtree
    }

    /// The subtree keeps its positions only if it came from this tree's parse: a position in another file means
    /// nothing here.
    pub(crate) fn put_subtree(&mut self, parent: Option<NodeId>, index: usize, subtree: Subtree) {
        let root = subtree.root;
        self.siblings_mut(parent).insert(index, root);
        self.nodes.extend(subtree.nodes);
        self.entries.extend(subtree.entries);
        if subtree.origin.is_some() && subtree.origin == self.origin {
            self.positions.extend(subtree.positions);
        }
        self.slot_mut(root).parent = parent;
    }

    /// None -> Some(empty).
    pub(crate) fn open_block(&mut self, node: NodeId) {
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

    pub(crate) fn insert_entry(&mut self, node: NodeId, index: usize) -> EntryId {
        let id = EntryId::fresh();
        self.slot_mut(node).entries.insert(index, id);
        self.entries.insert(id, node);
        id
    }

    pub(crate) fn remove_entry(&mut self, entry: EntryId) {
        let (node, index) = self.entry_position(entry).expect("entry is not in this tree");
        self.slot_mut(node).entries.remove(index);
        self.entries.remove(&entry);
        self.positions.remove(&entry.key());
    }

    /// Panics with a description of the first difference. Called after every edit in debug builds.
    pub(crate) fn check_shape(&self, doc: &KdlDocument) {
        // A work list rather than recursion, like `from_document`. Paths are only worked out for the message.
        let path = |node: Option<NodeId>| node.and_then(|n| self.path(n)).unwrap_or_default();
        let mut pending: Vec<(Option<NodeId>, &[NodeId], &KdlDocument)> = vec![(None, &self.top, doc)];
        while let Some((parent, ids, block)) = pending.pop() {
            let nodes = block.nodes();
            assert!(
                ids.len() == nodes.len(),
                "shape mismatch at {:?}: tree has {} nodes, document has {}",
                path(parent),
                ids.len(),
                nodes.len()
            );
            for (&id, node) in ids.iter().zip(nodes) {
                let slot = self.slot(id);
                assert!(
                    slot.entries.len() == node.entries().len(),
                    "shape mismatch at {:?}: tree has {} entries, document has {}",
                    path(Some(id)),
                    slot.entries.len(),
                    node.entries().len()
                );
                #[expect(
                    clippy::panic,
                    reason = "a shape mismatch is an rkdl bug, and this only runs in debug builds"
                )]
                match (&slot.children, node.children()) {
                    (None, None) => {}
                    (Some(ids), Some(block)) => pending.push((Some(id), ids, block)),
                    (tree, doc) => panic!(
                        "shape mismatch at {:?}: tree has children block {}, document has {}",
                        path(Some(id)),
                        tree.is_some(),
                        doc.is_some()
                    ),
                }
            }
        }
    }
}

pub(crate) fn node_at<'a>(doc: &'a KdlDocument, path: &[usize]) -> &'a KdlNode {
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

pub(crate) fn node_at_mut<'a>(doc: &'a mut KdlDocument, path: &[usize]) -> &'a mut KdlNode {
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

    /// A new node and its ids, taken out of its own one-node tree the way a detached node's are.
    fn detached(text: &str) -> (KdlNode, Subtree) {
        let doc = parse(text);
        let mut home = IdTree::from_document(&doc);
        let root = home.top()[0];
        (doc.nodes()[0].clone(), home.take_subtree(root))
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
            let (new, subtree) = detached(text);
            let id = subtree.root();
            tree.put_subtree(None, index, subtree);
            doc.nodes_mut().insert(index, new);
            tree.check_shape(&doc);
            assert_eq!(tree.path(id), Some(vec![index]));
            assert_eq!(tree.parent(id), None);
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
            let (new, subtree) = detached(text);
            let id = subtree.root();
            tree.put_subtree(Some(p), index, subtree);
            node_at_mut(&mut doc, &[0])
                .ensure_children()
                .nodes_mut()
                .insert(index, new);
            tree.check_shape(&doc);
            assert_eq!(tree.path(id), Some(vec![0, index]));
            assert_eq!(tree.parent(id), Some(p));
            for (i, &entry) in tree.entries(id).iter().enumerate() {
                assert_eq!(tree.entry_position(entry), Some((id, i)));
            }
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
    #[expect(
        clippy::many_single_char_names,
        reason = "each id is named after its node in the KDL text"
    )]
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
    fn positions_follow_their_ids_within_one_parse() {
        let text = "a 1\np {\n    q x=1\n}\n";
        let doc = parse(text);
        let mut tree = IdTree::from_parse(&doc, text);
        let pos = |tree: &IdTree, key: u64| tree.pos(key).map(|p| (p.line, p.column));
        let (a, p) = (tree.top()[0], tree.top()[1]);
        let q = tree.children(p).unwrap()[0];
        let x = tree.entries(q)[0];
        assert_eq!(pos(&tree, a.key()), Some((1, 1)));
        assert_eq!(pos(&tree, tree.entries(a)[0].key()), Some((1, 3)));
        assert_eq!(pos(&tree, x.key()), Some((3, 7)));

        // Detached and back: kept.
        let home = IdTree::from_subtree(tree.take_subtree(p));
        assert_eq!(pos(&tree, q.key()), None);
        assert_eq!(pos(&home, q.key()), Some((3, 5)));
        let mut home = home;
        tree.put_subtree(None, 0, home.take_subtree(p));
        assert_eq!(pos(&tree, p.key()), Some((2, 1)));
        assert_eq!(pos(&tree, x.key()), Some((3, 7)));

        // Into a tree from another parse, or none: dropped.
        let mut other = IdTree::from_parse(&parse(text), text);
        other.put_subtree(None, 0, tree.take_subtree(p));
        assert_eq!(pos(&other, p.key()), None);
        assert_eq!(pos(&other, x.key()), None);
        let mut built = IdTree::from_document(&doc);
        assert_eq!(pos(&built, built.top()[0].key()), None);
        built.put_subtree(None, 0, tree.take_subtree(a));
        assert_eq!(pos(&built, a.key()), None);

        // A removed entry's position goes with it.
        let mut tree = IdTree::from_parse(&doc, text);
        let a = tree.top()[0];
        let one = tree.entries(a)[0];
        tree.remove_entry(one);
        assert_eq!(pos(&tree, one.key()), None);
        assert_eq!(tree.positions.len(), 4);
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

    /// Building, checking and walking a tree doesn't recurse: a million levels on a 1 MB stack.
    #[test]
    fn deep_trees_need_no_stack() {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let depth = 1_000_000;
                let mut doc = KdlDocument::new();
                doc.nodes_mut().push(KdlNode::new("leaf"));
                for _ in 0..depth {
                    let mut node = KdlNode::new("a");
                    node.set_children(doc);
                    doc = KdlDocument::new();
                    doc.nodes_mut().push(node);
                }
                let tree = IdTree::from_document(&doc);
                tree.check_shape(&doc);
                let mut leaf = tree.top()[0];
                while let Some(&[child]) = tree.children(leaf) {
                    leaf = child;
                }
                assert_eq!(tree.path(leaf).map(|p| p.len()), Some(depth + 1));
                // kdl-rs's own Drop recurses per level.
                std::mem::forget(doc);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
