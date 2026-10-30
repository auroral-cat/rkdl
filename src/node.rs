use std::sync::Arc;

use kdl::{KdlDocument, KdlEntry, KdlIdentifier, KdlNode};
use pyo3::PyTraverseError;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString, PyTuple};

use crate::args::{self, Args};
use crate::children::{self, At, Children, Owner};
use crate::document::Document;
use crate::entry::{Entries, new_entry};
use crate::handles::{self, Fail, Home, stale};
use crate::parser;
use crate::props::{self, Props};
use crate::spelling;
use crate::tree::{IdTree, NodeId, node_at, node_at_mut};
use crate::values::{self, TypeRegistry};

/// One KDL node: a name, an optional type annotation, arguments, properties and children.
///
/// A node read from a document is a live handle: edits through it change the document, and reading the same node
/// twice gives the same object. `Node(...)` makes a detached node, which belongs to no document until you add it to
/// one. A node can be in only one place at a time: take it out first (`pop`, `remove`, `del`, `detach()`), or add a
/// `copy()`.
///
/// A `Value` among the arguments or properties gives that entry a type annotation: `Node("server", Value(8080,
/// type="u16"))` is `server (u16)8080`.
///
/// Args:
///     name: The node's name.
///     *args: The arguments.
///     type: The type annotation, if any.
///     props: Properties, for keys that can't be keyword arguments, such as `type` or `children`.
///     children: Detached nodes to put in a children block. None means no block, and `[]` an empty one.
///     **kwargs: More properties.
///
/// Raises:
///     TypeError: A value can't be converted to KDL.
///     ValueError: A property is given twice, one of `children` is already in a document, or a `Value` doesn't fit its
///         annotation.
#[pyclass(module = "rkdl", frozen, weakref)]
pub struct Node {
    // Ids are global, so moving the node to another document only repoints `home`.
    pub(crate) id: NodeId,
    pub(crate) home: Arc<Home>,
}

impl Node {
    pub(crate) fn new(id: NodeId, home: Arc<Home>) -> Self {
        Node { id, home }
    }

    /// Runs `f` on this node's `KdlNode` under its document's lock.
    pub(crate) fn with_node<R: Send>(&self, py: Python<'_>, f: impl FnOnce(&mut KdlNode) -> R + Send) -> PyResult<R> {
        let id = self.id;
        let mut f = Some(f);
        handles::on_home(py, &self.home, |doc| {
            doc.get().with_state(py, |s| {
                let path = s.ids.path(id).ok_or_else(|| Fail::Missing(stale()))?;
                let f = f.take().expect("runs once it finds the node");
                Ok(f(node_at_mut(&mut s.kdl, &path)))
            })
        })
    }

    /// `<Node name>`. Never raises, so the views' reprs can use it.
    pub(crate) fn repr(&self, py: Python<'_>) -> String {
        match self.with_node(py, |n| values::identifier(n.name().value()).to_string()) {
            Ok(name) => format!("<Node {name}>"),
            Err(_) => "<Node (unreadable)>".to_owned(),
        }
    }
}

/// Wraps one `KdlNode` in a private document of its own, with fresh ids, that reads and writes through `types` and
/// writes line endings in the style `eol`.
pub(crate) fn detached<'py>(
    py: Python<'py>,
    node: KdlNode,
    types: Py<TypeRegistry>,
    eol: &'static str,
) -> PyResult<Bound<'py, Node>> {
    let mut kdl = KdlDocument::new();
    kdl.nodes_mut().push(node);
    let ids = IdTree::from_document(&kdl);
    let &[id] = ids.top() else {
        return Err(stale());
    };
    let home = Bound::new(py, Document::empty_home(types))?;
    home.get().fill(kdl, ids, eol);
    // Nothing else can reach a new home, so the node is still there.
    Document::node_handle(&home, id)?.ok_or_else(stale)
}

fn entry(value: &Bound<'_, PyAny>, key: Option<String>, types: &TypeRegistry) -> PyResult<KdlEntry> {
    Ok(new_entry(key, values::encode(value, None, types)?))
}

/// The keyword arguments of `Node(...)`, `parser.node(...)` and `doc.node(...)` other than `**kwargs`. `type` and
/// `children` are common property names, so a value that can't be either says how to set a property by that name.
pub(crate) struct Spec<'a, 'py> {
    pub(crate) ty: Option<&'a Bound<'py, PyAny>>,
    pub(crate) props: Option<&'a Bound<'py, PyDict>>,
    pub(crate) kwargs: Option<&'a Bound<'py, PyDict>>,
    pub(crate) children: Option<&'a Bound<'py, PyAny>>,
}

/// Builds a detached node that reads and writes through `types`, with line endings `eol`.
pub(crate) fn build<'py>(
    name: String,
    args: &Bound<'py, PyTuple>,
    spec: &Spec<'_, 'py>,
    types: Py<TypeRegistry>,
    eol: &'static str,
) -> PyResult<Bound<'py, Node>> {
    let py = args.py();
    let mut node = KdlNode::new(values::identifier(name));
    match spec.ty.filter(|ty| !ty.is_none()) {
        None => {}
        Some(ty) if ty.is_instance_of::<PyString>() => node.set_ty(values::identifier(ty.extract::<String>()?)),
        Some(_) => {
            return Err(PyTypeError::new_err(
                "type= is the node's type annotation and must be a str or None; for a property named \"type\", \
                 pass props={\"type\": ...}",
            ));
        }
    }
    for value in args.iter() {
        node.entries_mut().push(entry(&value, None, types.get())?);
    }
    let mut keys: Vec<String> = Vec::new();
    for dict in [spec.props, spec.kwargs].into_iter().flatten() {
        for (key, value) in dict.iter() {
            #[expect(
                clippy::map_err_ignore,
                reason = "this message says which argument was wrong; PyO3's doesn't"
            )]
            let key: String = key
                .extract()
                .map_err(|_| PyTypeError::new_err("property keys must be str"))?;
            if keys.contains(&key) {
                return Err(PyValueError::new_err(format!(
                    "property {key:?} is given both in props= and as a keyword argument"
                )));
            }
            keys.push(key.clone());
            node.entries_mut().push(entry(&value, Some(key), types.get())?);
        }
    }
    let children = match spec.children.filter(|c| !c.is_none()) {
        Some(children) => Some(children::nodes_in(
            children,
            "children= (for a property named \"children\", pass props={\"children\": ...})",
        )?),
        None => None,
    };
    let node = detached(py, node, types, eol)?;
    if let Some(children) = children {
        children::splice(py, &Owner::Node(node.clone().unbind()), &At::All, &children, true)?;
    }
    Ok(node)
}

#[pymethods]
impl Node {
    #[new]
    #[pyo3(signature = (name, *args, r#type=None, props=None, children=None, **kwargs))]
    fn py_new<'py>(
        name: String,
        args: &Bound<'py, PyTuple>,
        r#type: Option<&Bound<'py, PyAny>>,
        props: Option<&Bound<'py, PyDict>>,
        children: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Py<Node>> {
        let spec = Spec {
            ty: r#type,
            props,
            kwargs,
            children,
        };
        let types = TypeRegistry::shared_builtin(args.py())?;
        Ok(build(name, args, &spec, types, "\n")?.unbind())
    }

    /// The node's name.
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        self.with_node(py, |n| n.name().value().to_owned())
    }

    // A quoted name stays quoted.
    #[setter]
    fn set_name(&self, py: Python<'_>, name: String) -> PyResult<()> {
        self.with_node(py, |n| {
            if n.name().value() != name {
                n.set_name(spelling::identifier(Some(n.name()), name));
            }
        })
    }

    /// The node's type annotation, or None. Setting None removes it.
    #[getter]
    fn r#type(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.with_node(py, |n| n.ty().map(|t| t.value().to_owned()))
    }

    // A quoted annotation stays quoted.
    #[setter]
    fn set_type(&self, py: Python<'_>, ty: Option<String>) -> PyResult<()> {
        self.with_node(py, |n| {
            if n.ty().map(KdlIdentifier::value) != ty.as_deref() {
                let new = ty.map(|ty| spelling::identifier(n.ty(), ty));
                *n.ty_mut() = new;
            }
        })
    }

    /// The arguments, as a live list of values.
    ///
    /// Assigning a list replaces them all.
    #[getter]
    fn args(slf: &Bound<'_, Self>) -> Args {
        Args::new(slf.clone().unbind())
    }

    // Replaces the arguments, as `args[:] = values` does. Assigning the node's own view does nothing, so
    // `node.args += values` works.
    #[setter]
    fn set_args(slf: &Bound<'_, Self>, values: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let values = values.ok_or_else(|| PyTypeError::new_err("can't delete node.args; use node.args.clear()"))?;
        if Args::is_view_of(values, slf) {
            return Ok(());
        }
        args::assign(slf.get(), None, values)
    }

    /// The properties, as a live dict of values.
    ///
    /// Assigning a mapping replaces them all. Keys the node already had keep their place in the text.
    #[getter]
    fn props(slf: &Bound<'_, Self>) -> Props {
        Props::new(slf.clone().unbind())
    }

    // Replaces the properties with a mapping's. Keys the node had keep their place and spelling. Assigning the
    // node's own view does nothing, so `node.props |= mapping` works.
    #[setter]
    fn set_props(slf: &Bound<'_, Self>, mapping: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let mapping = mapping.ok_or_else(|| PyTypeError::new_err("can't delete node.props; use node.props.clear()"))?;
        if Props::is_view_of(mapping, slf) {
            return Ok(());
        }
        props::assign(slf.get(), mapping)
    }

    /// The arguments and properties as `Entry` objects, as a live, read-only list in the order of the text.
    #[getter]
    fn entries(slf: &Bound<'_, Self>) -> Entries {
        Entries::new(slf.clone().unbind())
    }

    /// The child nodes as a live list, or None if the node has no children block.
    ///
    /// Assigning a list of detached nodes replaces them all, and gives the node a block if it had none: `[]` makes
    /// an empty `{}` block. Assigning None removes the block.
    #[getter]
    fn children(slf: &Bound<'_, Self>) -> PyResult<Option<Children>> {
        let has_block = slf.get().with_node(slf.py(), |n| n.children().is_some())?;
        Ok(has_block.then(|| Children::new(Owner::Node(slf.clone().unbind()))))
    }

    // Replaces the children with detached nodes, all or nothing, giving the node a block if it has none: `[]` is
    // an empty block. None removes the block. Assigning the node's own view does nothing, so
    // `node.children += nodes` works.
    #[setter]
    fn set_children(slf: &Bound<'_, Self>, value: Option<Bound<'_, PyAny>>) -> PyResult<()> {
        let py = slf.py();
        let owner = Owner::Node(slf.clone().unbind());
        match value {
            Some(v) if children::is_view_of(&v, slf.as_any()) => Ok(()),
            Some(v) if !v.is_none() => {
                let nodes = children::nodes_in(&v, "node.children")?;
                children::splice(py, &owner, &At::All, &nodes, true)
            }
            _ => children::close_block(py, &owner),
        }
    }

    /// The node this one is a child of. None for a top-level node, and for a detached node.
    #[getter]
    fn parent<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, Node>>> {
        children::parent(py, self)
    }

    /// Takes the node out of its document and returns it.
    ///
    /// Afterwards the node is detached, ready to be added somewhere else. A detached node stays as it is.
    fn detach<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, Self>> {
        children::detach_node(slf.py(), slf.get())?;
        Ok(slf.clone())
    }

    // The children, or nothing for a node without a block.
    fn __iter__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        children::iter_children(slf.py(), &Owner::Node(slf.clone().unbind()))
    }

    // Whether a child matches a name or a `(type, name)` key, as `node[key]` would find it.
    fn __contains__(slf: &Bound<'_, Self>, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        children::contains(&Owner::Node(slf.clone().unbind()), key)
    }

    // Unlike `ElementTree`'s `iter`, the node itself isn't included, so a node and a document behave alike.
    /// Iterates over every node below this one, at any depth, depth first in file order.
    ///
    /// The node itself isn't included. Editing while iterating is safe: the nodes are found up front.
    ///
    /// Args:
    ///     key: Only nodes that match: a name, or a `(type, name)` tuple. None means every node.
    #[pyo3(signature = (key=None))]
    fn iter<'py>(slf: &Bound<'py, Self>, key: Option<&Bound<'py, PyAny>>) -> PyResult<Bound<'py, PyAny>> {
        let none = slf.py().None().into_bound(slf.py());
        children::descendants(&Owner::Node(slf.clone().unbind()), key.unwrap_or(&none))
    }

    fn __copy__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, Node>> {
        self.copy(py)
    }

    // A copy shares nothing with the node, so `copy.deepcopy`'s memo has nothing to record.
    fn __deepcopy__<'py>(&self, py: Python<'py>, _memo: &Bound<'py, PyAny>) -> PyResult<Bound<'py, Node>> {
        self.copy(py)
    }

    /// Returns a detached copy of the node and everything in it.
    fn copy<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, Node>> {
        let ((node, eol), types) = handles::on_home(py, &self.home, |doc| {
            let node = doc.get().with_state(py, |s| {
                let path = s.ids.path(self.id).ok_or_else(|| Fail::Missing(stale()))?;
                let node = node_at(&s.kdl, &path);
                let deep = parser::too_deep_for_the_stack(std::slice::from_ref(node));
                Ok::<_, Fail>((parser::recursing(deep, || node.clone())?, s.eol))
            })?;
            Ok((node, doc.get().types.clone_ref(py)))
        })?;
        detached(py, node, types, eol)
    }

    fn __getitem__<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Bound<'py, Node>> {
        children::lookup_one(&Owner::Node(slf.clone().unbind()), key)
    }

    /// Returns the first child that matches a key, or `default`.
    ///
    /// Args:
    ///     key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
    ///     default: What to return if no child matches.
    #[pyo3(signature = (key, default=None))]
    fn get<'py>(
        slf: &Bound<'py, Self>,
        key: &Bound<'py, PyAny>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        children::lookup_get(&Owner::Node(slf.clone().unbind()), key, default)
    }

    /// Returns every child that matches a key, in order.
    ///
    /// Args:
    ///     key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
    fn get_all<'py>(slf: &Bound<'py, Self>, key: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, Node>>> {
        children::lookup(&Owner::Node(slf.clone().unbind()), key, false)
    }

    fn __str__(&self, py: Python<'_>) -> PyResult<String> {
        self.with_node(py, |n| {
            let deep = parser::too_deep_for_the_stack(std::slice::from_ref(n));
            let text = parser::recursing(deep, || n.to_string())?;
            // The leading text is the indentation and comments above the node, which belong to its surroundings.
            let leading = n.format().map_or("", |f| f.leading.as_str());
            Ok(text.strip_prefix(leading).unwrap_or(&text).trim().to_owned())
        })?
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        self.repr(py)
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "PyO3 extracts the other operand as a PyRef"
    )]
    fn __eq__(&self, other: PyRef<'_, Node>) -> bool {
        self.id == other.id
    }

    fn __hash__(&self) -> u64 {
        self.id.key()
    }

    // The home is the node's only strong reference. No `__clear__`: a cycle through a node also passes a Type (behind
    // the document's only strong reference), which clears.
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        self.home.traverse(&visit)
    }
}
