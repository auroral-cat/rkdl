pub(crate) mod annotation;
mod blocks;
pub(crate) mod prescan;

use std::sync::Arc;

use kdl::{KdlDiagnostic, KdlDocument, KdlError, KdlNode};
use miette::Severity;
use pyo3::PyTraverseError;
use pyo3::exceptions::PyRuntimeError;
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use crate::document::Document;
use crate::errors::parse_error_in;
use crate::format::Formatter;
use crate::layout;
use crate::node::{self, Node, Spec};
use crate::tree::IdTree;
use crate::values::{Type, TypeRegistry};

/// Parses KDL text into documents, with custom types and formatting.
///
/// Args:
///     types: Custom type annotations, used by every document this parser makes. A later Type replaces an earlier
///         one with the same name.
///     formatter: If given, each document is formatted with it right after parsing.
#[pyclass(module = "rkdl", frozen)]
pub struct Parser {
    types: Py<TypeRegistry>,
    // A Formatter holds no Python references, so it can't be part of a cycle.
    formatter: Option<Py<Formatter>>,
}

#[pymethods]
impl Parser {
    #[new]
    #[pyo3(signature = (types=None, formatter=None))]
    fn new(py: Python<'_>, types: Option<Vec<Py<Type>>>, formatter: Option<Py<Formatter>>) -> PyResult<Self> {
        let types = match types {
            Some(types) => Py::new(py, TypeRegistry::with_user_types(py, types))?,
            None => TypeRegistry::shared_builtin(py)?,
        };
        Ok(Parser { types, formatter })
    }

    /// Parses a KDL document.
    ///
    /// Args:
    ///     text: The KDL text.
    ///
    /// Returns:
    ///     The document, which reads and writes values through this parser's types.
    ///
    /// Raises:
    ///     ParseError: The text isn't valid KDL.
    fn parse(&self, py: Python<'_>, text: &str) -> PyResult<Document> {
        self.parse_from(py, text, None)
    }

    /// Parses a KDL document from a path, a binary file or a text file.
    ///
    /// Paths and binary files are read as UTF-8 bytes, so line endings survive exactly. A text file opened without
    /// `newline=""` turns CRLF into LF as it reads.
    ///
    /// Args:
    ///     source: A path, or a file opened for reading.
    ///
    /// Returns:
    ///     The document, which reads and writes values through this parser's types.
    ///
    /// Raises:
    ///     ParseError: The text isn't valid KDL. Its `path` names the file, when it has a name.
    ///     UnicodeDecodeError: The bytes aren't UTF-8.
    fn load(&self, source: &Bound<'_, PyAny>) -> PyResult<Document> {
        let py = source.py();
        // Reading files is plainer in Python, which knows paths, file objects and their names.
        let (text, path): (String, Option<String>) =
            py.import("rkdl")?.getattr("_read")?.call1((source,))?.extract()?;
        self.parse_from(py, &text, path.as_deref())
    }

    /// Makes a detached node that reads and writes values through this parser's types.
    ///
    /// Takes the same arguments as `Node`.
    #[pyo3(signature = (name, *args, r#type=None, props=None, children=None, **kwargs))]
    fn node<'py>(
        &self,
        name: String,
        args: &Bound<'py, PyTuple>,
        r#type: Option<&Bound<'py, PyAny>>,
        props: Option<&Bound<'py, PyDict>>,
        children: Option<&Bound<'py, PyAny>>,
        kwargs: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Bound<'py, Node>> {
        let spec = Spec {
            ty: r#type,
            props,
            kwargs,
            children,
        };
        node::build(name, args, &spec, self.types.clone_ref(args.py()), "\n")
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        visit.call(&self.types)
    }
}

impl Parser {
    /// `path` is the file the text came from, for the `ParseError`.
    fn parse_from(&self, py: Python<'_>, text: &str, path: Option<&str>) -> PyResult<Document> {
        let formatter = self.formatter.as_ref().map(Py::get);
        let (kdl, ids) = py
            .detach(|| on_big_stack(|| parse_text(text, formatter)))?
            .map_err(|e| parse_error_in(py, &e, path))?;
        Ok(Document::parsed(kdl, ids, self.types.clone_ref(py), path))
    }
}

/// The document, and its ids with every node's and entry's position in `text`.
fn parse_text(text: &str, formatter: Option<&Formatter>) -> Result<(KdlDocument, IdTree), KdlError> {
    let scan = prescan::scan(text);
    if let Some(refusal) = scan.refusal {
        return Err(refused(text, refusal));
    }
    let mut kdl = text.parse::<KdlDocument>()?;
    // The scan follows every valid document, but kdl-rs may take text the spec doesn't, and then the scan breaks and
    // its record of the blocks is incomplete. Without it the text after a `}` comes back only in part (see blocks.rs).
    let closes = scan.broke_at.is_none().then_some(&scan.closes[..]);
    blocks::restore(text, &mut kdl, closes);
    annotation::restore(&mut kdl);
    // Positions come from the spans, which say where things were in `text`, so they're read before formatting.
    let ids = IdTree::from_parse(&kdl, text);
    // Formatting before the Document exists is the same as doc.fmt() but needs no lock.
    if let Some(formatter) = formatter {
        let eol = layout::line_ending(&kdl);
        formatter.apply(&mut kdl, eol);
    }
    Ok((kdl, ids))
}

/// The prescan's refusal as a kdl-rs error, so it reaches Python as a `ParseError` like any other.
fn refused(text: &str, refusal: prescan::Refusal) -> KdlError {
    let input = Arc::new(text.to_owned());
    KdlError {
        input: Arc::clone(&input),
        diagnostics: vec![KdlDiagnostic {
            input,
            span: (refusal.offset, refusal.len).into(),
            message: Some(refusal.message),
            label: Some(refusal.label),
            help: Some(refusal.help),
            severity: Severity::Error,
        }],
    }
}

/// Reserved address space, committed only as it's used. kdl-rs's parser needs about 32 KB per block level on a debug
/// build and 6 KB on a release build; the prescan's limits keep it under 140 MB and 30 MB. Printing, cloning and
/// formatting need far less.
const STACK_SIZE: usize = 256 << 20;

/// Block levels up to which kdl-rs's printing, cloning and formatting, which recurse once per level, run on the
/// caller's own stack. Cloning needs the most, about 3.3 KB a level on a debug build, so this takes up to about 110 KB
/// of a thread that may have only 512 KB (macOS's default for threads other than the main one).
const SHALLOW: usize = 32;

/// Whether `nodes` nest too deep for kdl-rs's recursive printing, cloning and formatting to run on the caller's stack.
pub(crate) fn too_deep_for_the_stack(nodes: &[KdlNode]) -> bool {
    levels(nodes, SHALLOW) > SHALLOW
}

/// Runs kdl-rs's recursive printing, cloning or formatting, on a thread with a big stack if `deep` says the nodes it
/// works on are `too_deep_for_the_stack`. The GIL is released already: the callers hold a document's lock.
pub(crate) fn recursing<T: Send>(deep: bool, f: impl FnOnce() -> T + Send) -> PyResult<T> {
    if deep { on_big_stack(f) } else { Ok(f()) }
}

/// How many levels of blocks `nodes` nest, counting a node's own block as one, or `past + 1` if it's more than `past`.
/// Stops looking as soon as it knows.
pub(crate) fn levels<'a>(nodes: &'a [KdlNode], past: usize) -> usize {
    // A work list rather than recursion: this is how rkdl finds out whether recursion is safe.
    let blocks =
        |nodes: &'a [KdlNode], level: usize| nodes.iter().filter_map(KdlNode::children).map(move |b| (b, level));
    let mut pending: Vec<(&KdlDocument, usize)> = blocks(nodes, 1).collect();
    let mut deepest = 0;
    while let Some((block, level)) = pending.pop() {
        deepest = deepest.max(level);
        if deepest > past {
            return past + 1;
        }
        pending.extend(blocks(block.nodes(), level + 1));
    }
    deepest
}

/// Runs `f` on a new thread with a big stack, since kdl-rs's parser, printing, cloning and formatting recurse once per
/// nesting level.
fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> PyResult<T> {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .name("rkdl-deep".into())
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, f)
            .map_err(|e| PyRuntimeError::new_err(format!("couldn't start a thread with a big stack: {e}")))?;
        Ok(thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    })
}

/// Parses a KDL document.
///
/// The same as `Parser().parse(text)`.
///
/// Args:
///     text: The KDL text.
///
/// Returns:
///     The document.
///
/// Raises:
///     ParseError: The text isn't valid KDL.
#[pyfunction]
pub fn parse(py: Python<'_>, text: &str) -> PyResult<Document> {
    let parser = Parser {
        types: TypeRegistry::shared_builtin(py)?,
        formatter: None,
    };
    parser.parse(py, text)
}
