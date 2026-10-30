use kdl::{KdlEntry, KdlEntryFormat, KdlIdentifier, KdlValue};
use pyo3::IntoPyObjectExt;
use pyo3::exceptions::{PyException, PyOverflowError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::gc::PyVisit;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyString, PyType};
use pyo3::{PyTraverseError, PyTypeInfo};

use crate::errors::parse_error;

// Not frozen: `__clear__` has to drop the hooks, since a hook that refers to a document using this Type is a
// reference cycle.
/// A custom type annotation: how to read and write the values annotated with it.
///
/// Pass Types to a `Parser`. A Type with the same name as a reserved one, such as `date`, replaces it.
///
/// Args:
///     name: The annotation, without parentheses: `"celsius"` for `(celsius)22.5`.
///     cls: The Python class of the values. Writing an instance of it annotates the entry with `name`.
///     decode: Turns the plain value from the document (a str, int, float, bool or None) into a Python value.
///     encode: Turns an instance of `cls` into a plain value to write.
///
/// Raises:
///     TypeError: `decode` or `encode` isn't callable.
#[pyclass(module = "rkdl")]
pub(crate) struct Type {
    /// The annotation this Type handles.
    #[pyo3(get)]
    name: String,
    /// None once the cycle collector has cleared this Type.
    hooks: Option<Hooks>,
}

struct Hooks {
    cls: Py<PyType>,
    decode: Py<PyAny>,
    encode: Py<PyAny>,
}

impl Type {
    /// One of the hooks. Only a finalizer running during cycle collection can see a cleared Type.
    fn hook<'py, T: PyTypeInfo>(ty: &Bound<'py, Type>, pick: impl FnOnce(&Hooks) -> &Py<T>) -> PyResult<Bound<'py, T>> {
        let py = ty.py();
        match &ty.borrow().hooks {
            Some(hooks) => Ok(pick(hooks).bind(py).clone()),
            None => Err(PyRuntimeError::new_err("Type was cleared by the garbage collector")),
        }
    }
}

#[pymethods]
impl Type {
    #[new]
    #[pyo3(signature = (name, cls, decode, encode))]
    fn new(py: Python<'_>, name: String, cls: Py<PyType>, decode: Py<PyAny>, encode: Py<PyAny>) -> PyResult<Self> {
        if !decode.bind(py).is_callable() || !encode.bind(py).is_callable() {
            return Err(PyTypeError::new_err("Type decode and encode must be callable"));
        }
        Ok(Type {
            name,
            hooks: Some(Hooks { cls, decode, encode }),
        })
    }

    /// The Python class of the values.
    #[getter]
    fn cls(slf: &Bound<'_, Self>) -> PyResult<Py<PyType>> {
        Ok(Type::hook(slf, |h| &h.cls)?.unbind())
    }

    /// Turns a plain value from the document into a Python value.
    #[getter]
    fn decode(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        Ok(Type::hook(slf, |h| &h.decode)?.unbind())
    }

    /// Turns a Python value into a plain value to write.
    #[getter]
    fn encode(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        Ok(Type::hook(slf, |h| &h.encode)?.unbind())
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let cls = Type::hook(slf, |h| &h.cls)?.qualname()?;
        Ok(format!("Type({:?}, {cls})", slf.borrow().name))
    }

    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        if let Some(hooks) = &self.hooks {
            visit.call(&hooks.cls)?;
            visit.call(&hooks.decode)?;
            visit.call(&hooks.encode)?;
        }
        Ok(())
    }

    fn __clear__(&mut self) {
        self.hooks = None;
    }
}

#[derive(Clone, Copy)]
enum Builtin {
    Int { min: i128, max: i128 },
    F32,
    F64,
    // decimal64 and decimal128 annotate numbers, while plain `decimal` annotates a string.
    DecimalNumber,
    DecimalString,
    DateTime,
    Date,
    Time,
    Ipv4,
    Ipv6,
    Uuid,
    Regex,
    Base64,
    // Reserved, but no standard-library type fits, so the value stays a str.
    PlainString,
}

const fn int(min: i128, max: i128) -> Builtin {
    Builtin::Int { min, max }
}

const BUILTINS: &[(&str, Builtin)] = &[
    ("i8", int(i8::MIN as i128, i8::MAX as i128)),
    ("i16", int(i16::MIN as i128, i16::MAX as i128)),
    ("i32", int(i32::MIN as i128, i32::MAX as i128)),
    ("i64", int(i64::MIN as i128, i64::MAX as i128)),
    ("i128", int(i128::MIN, i128::MAX)),
    ("isize", int(i64::MIN as i128, i64::MAX as i128)),
    ("u8", int(0, u8::MAX as i128)),
    ("u16", int(0, u16::MAX as i128)),
    ("u32", int(0, u32::MAX as i128)),
    ("u64", int(0, u64::MAX as i128)),
    // kdl-rs stores integers as i128, so the top half of u128 can't be represented anyway.
    ("u128", int(0, i128::MAX)),
    ("usize", int(0, u64::MAX as i128)),
    ("f32", Builtin::F32),
    ("f64", Builtin::F64),
    ("decimal64", Builtin::DecimalNumber),
    ("decimal128", Builtin::DecimalNumber),
    ("decimal", Builtin::DecimalString),
    ("date-time", Builtin::DateTime),
    ("date", Builtin::Date),
    ("time", Builtin::Time),
    ("ipv4", Builtin::Ipv4),
    ("ipv6", Builtin::Ipv6),
    ("uuid", Builtin::Uuid),
    ("regex", Builtin::Regex),
    ("base64", Builtin::Base64),
    ("duration", Builtin::PlainString),
    ("currency", Builtin::PlainString),
    ("country-2", Builtin::PlainString),
    ("country-3", Builtin::PlainString),
    ("country-subdivision", Builtin::PlainString),
    ("email", Builtin::PlainString),
    ("idn-email", Builtin::PlainString),
    ("hostname", Builtin::PlainString),
    ("idn-hostname", Builtin::PlainString),
    ("url", Builtin::PlainString),
    ("url-reference", Builtin::PlainString),
    ("irl", Builtin::PlainString),
    ("irl-reference", Builtin::PlainString),
    ("url-template", Builtin::PlainString),
    ("base85", Builtin::PlainString),
];

// Which annotation a non-plain Python value gets when nothing else claims it. datetime comes before date because
// it's a subclass of date.
const INFERRED: &[(&str, Builtin)] = &[
    ("date-time", Builtin::DateTime),
    ("date", Builtin::Date),
    ("time", Builtin::Time),
    ("ipv4", Builtin::Ipv4),
    ("ipv6", Builtin::Ipv6),
    ("uuid", Builtin::Uuid),
    ("regex", Builtin::Regex),
    ("base64", Builtin::Base64),
    // `decimal` rather than decimal64, since a string keeps every digit.
    ("decimal", Builtin::DecimalString),
];

enum Converter<'a> {
    Builtin(Builtin),
    User(&'a UserType),
}

struct UserType {
    // Copied out of the Type so lookups don't have to borrow it.
    name: String,
    ty: Py<Type>,
}

/// Built-in reserved types plus any user Types. Immutable once built, and shared as a Python object by parsers,
/// documents and detached nodes' homes, each of which reports its own reference to the cycle collector.
///
/// No `__clear__`: its references never change, and every cycle through it passes a Type, which clears.
#[pyclass(module = "rkdl", frozen)]
pub(crate) struct TypeRegistry {
    // In the order given, which is the order inference tries them. A later Type shadows an earlier one with the same
    // name.
    user: Vec<UserType>,
}

impl TypeRegistry {
    pub(crate) const fn builtin() -> Self {
        TypeRegistry { user: Vec::new() }
    }

    pub(crate) fn with_user_types(py: Python<'_>, types: Vec<Py<Type>>) -> Self {
        let user = types
            .into_iter()
            .map(|ty| UserType {
                name: ty.bind(py).borrow().name.clone(),
                ty,
            })
            .collect();
        TypeRegistry { user }
    }

    /// The registry for documents and nodes without user types. It holds no Python references, so they can all share
    /// it.
    pub(crate) fn shared_builtin(py: Python<'_>) -> PyResult<Py<TypeRegistry>> {
        static BUILTIN: PyOnceLock<Py<TypeRegistry>> = PyOnceLock::new();
        Ok(BUILTIN
            .get_or_try_init(py, || Py::new(py, TypeRegistry::builtin()))?
            .clone_ref(py))
    }

    fn get(&self, name: &str) -> Option<Converter<'_>> {
        if let Some(user) = self.user.iter().rev().find(|u| u.name == name) {
            return Some(Converter::User(user));
        }
        BUILTINS
            .iter()
            .find(|&&(builtin, _)| builtin == name)
            .map(|&(_, b)| Converter::Builtin(b))
    }

    /// User types not shadowed by a later one, in order.
    fn active_user_types(&self) -> impl Iterator<Item = &UserType> {
        self.user
            .iter()
            .enumerate()
            .filter(|&(i, u)| !self.user.iter().skip(i + 1).any(|later| later.name == u.name))
            .map(|(_, u)| u)
    }
}

#[pymethods]
impl TypeRegistry {
    fn __traverse__(&self, visit: PyVisit<'_>) -> Result<(), PyTraverseError> {
        for user in &self.user {
            visit.call(&user.ty)?;
        }
        Ok(())
    }
}

/// KDL value plus its entry's type annotation -> Python object. Raises if a known type's contents don't fit.
pub(crate) fn to_python(
    py: Python<'_>,
    value: &KdlValue,
    ty: Option<&str>,
    types: &TypeRegistry,
) -> PyResult<Py<PyAny>> {
    to_python_spelled(py, value, None, ty, types)
}

/// `to_python`, given the value's text in the file too, if there is one. `decimal64` and `decimal128` read their
/// digits from it, since the parsed value is only an f64.
pub(crate) fn to_python_spelled(
    py: Python<'_>,
    value: &KdlValue,
    text: Option<&str>,
    ty: Option<&str>,
    types: &TypeRegistry,
) -> PyResult<Py<PyAny>> {
    let Some(ty) = ty else {
        return plain_to_python(py, value);
    };
    match types.get(ty) {
        None => plain_to_python(py, value),
        Some(Converter::User(user)) => {
            let plain = plain_to_python(py, value)?;
            let decode = Type::hook(user.ty.bind(py), |h| &h.decode)?;
            Ok(decode.call1((plain,))?.unbind())
        }
        Some(Converter::Builtin(b)) => {
            decode_builtin(py, b, value, text).map_err(|reason| unreadable(ty, value, text, &reason))
        }
    }
}

/// The error for a value a built-in type can't read. Valid KDL can hold values Python's own types refuse, such as
/// `(regex)#"\p{L}+"#`, so it says how to read the value anyway.
fn unreadable(ty: &str, value: &KdlValue, text: Option<&str>, reason: &str) -> PyErr {
    let text = text.map_or_else(|| canonical_repr(value), str::to_owned);
    let plain = match value {
        KdlValue::String(_) => Some("str"),
        KdlValue::Integer(_) => Some("int"),
        KdlValue::Float(_) => Some("float"),
        KdlValue::Bool(_) => Some("bool"),
        KdlValue::Null => None,
    };
    let example = plain.map_or_else(String::new, |cls| {
        format!(": Parser(types=[Type({ty:?}, {cls}, {cls}, {cls})]) reads it as a plain {cls}")
    });
    PyValueError::new_err(format!(
        "({}){text} can't be read as {ty}: {reason}. A Type named {ty:?} overrides the built-in one{example}",
        identifier(ty)
    ))
}

/// What writing a Python value stores: the KDL value, its annotation, and the text to print it as when the value
/// alone doesn't say (a `decimal64`'s digits, which an f64 can't hold).
pub(crate) struct Encoded {
    pub(crate) value: KdlValue,
    pub(crate) ty: Option<String>,
    pub(crate) repr: Option<String>,
}

impl Encoded {
    fn new(value: KdlValue, ty: Option<String>) -> Self {
        Encoded { value, ty, repr: None }
    }
}

/// `encode` without the text. Only a kept `decimal64` or `decimal128` annotation brings text of its own, so nothing
/// is lost without a current annotation.
pub(crate) fn from_python(
    value: &Bound<'_, PyAny>,
    current_ty: Option<&str>,
    types: &TypeRegistry,
) -> PyResult<(KdlValue, Option<String>)> {
    let encoded = encode(value, current_ty, types)?;
    Ok((encoded.value, encoded.ty))
}

/// Python object -> what to write, applying the "keep the current annotation if the value fits it" rule from
/// plan.md.
pub(crate) fn encode(value: &Bound<'_, PyAny>, current_ty: Option<&str>, types: &TypeRegistry) -> PyResult<Encoded> {
    if let Some(ty) = current_ty {
        let kept = match types.get(ty) {
            // An unknown annotation is the user's own label, so any plain value keeps it.
            None => plain_to_kdl(value)?.map(|kdl| (kdl, None)),
            Some(Converter::Builtin(b)) => {
                let kept = encode_builtin(b, value)?;
                // A number that doesn't fit a numeric type is a mistake, as with `entry.type = ...`, rather than a
                // reason to drop the annotation.
                if kept.is_none()
                    && let Some(reason) = misfit(b, value)?
                {
                    return Err(PyValueError::new_err(format!(
                        "{} doesn't fit {}: {reason}",
                        value.repr()?,
                        annotation(ty)
                    )));
                }
                // A plain value keeps the annotation if the type reads it, as with custom types.
                match kept {
                    None if is_exact_plain(value) => plain_to_kdl(value)?
                        .filter(|kdl| decode_builtin(value.py(), b, kdl, None).is_ok())
                        .map(|kdl| (kdl, None)),
                    kept => kept,
                }
            }
            Some(Converter::User(user)) => match encode_user(user, value)? {
                Some(kdl) => Some((kdl, None)),
                // A plain value keeps a custom annotation if the type reads it, as `(celsius)22.0` would be read.
                None if is_exact_plain(value) && decodes(user, value)? => plain_to_kdl(value)?.map(|kdl| (kdl, None)),
                None => None,
            },
        };
        if let Some((kdl, repr)) = kept {
            return Ok(Encoded {
                value: kdl,
                ty: Some(ty.to_owned()),
                repr,
            });
        }
    }

    if is_exact_plain(value) {
        let kdl = plain_to_kdl(value)?.expect("exact plain types always convert");
        return Ok(Encoded::new(kdl, None));
    }
    for user in types.active_user_types() {
        if let Some(kdl) = encode_user(user, value)? {
            return Ok(Encoded::new(kdl, Some(user.name.clone())));
        }
    }
    for &(name, b) in INFERRED {
        if !matches!(types.get(name), Some(Converter::Builtin(_))) {
            continue;
        }
        if let Some((kdl, repr)) = encode_builtin(b, value)? {
            return Ok(Encoded {
                value: kdl,
                ty: Some(name.to_owned()),
                repr,
            });
        }
    }
    // Subclasses of the plain types, such as an IntEnum no Type claimed.
    if let Some(kdl) = plain_to_kdl(value)? {
        return Ok(Encoded::new(kdl, None));
    }
    Err(PyTypeError::new_err(format!(
        "can't convert {} to a KDL value",
        value.get_type().qualname()?
    )))
}

/// Canonical KDL text for a value, written into `KdlEntryFormat::value_repr` after every value write.
pub(crate) fn canonical_repr(value: &KdlValue) -> String {
    let text = value.to_string();
    fixed_repr(&text, value).unwrap_or(text)
}

/// Sets the text an entry's value prints as. Only `value_repr` changes: the rest of the format holds the entry's
/// whitespace and any `/-` comment in front of it. An entry without a format prints its value through kdl-rs, so it
/// needs one only when `repr` differs from kdl-rs's text, and then the format has to carry the space in front of the
/// entry that kdl-rs would have printed.
pub(crate) fn set_value_repr(entry: &mut KdlEntry, repr: String) {
    if let Some(f) = entry.format_mut() {
        f.value_repr = repr;
    } else if repr != entry.value().to_string() {
        entry.set_format(KdlEntryFormat {
            value_repr: repr,
            leading: " ".into(),
            ..Default::default()
        });
    }
}

/// An identifier (node name, property key or type annotation) that prints as text which parses back. kdl-rs prints
/// identifiers the way it prints string values, so it has the same problems.
pub(crate) fn identifier(name: impl Into<String>) -> KdlIdentifier {
    let mut ident = KdlIdentifier::from(name.into());
    fix_identifier(&mut ident);
    ident
}

/// Gives an identifier the escaped, quoted spelling if the text it prints wouldn't read back as its value.
pub(crate) fn fix_identifier(ident: &mut KdlIdentifier) {
    if !reads_back_as(&ident.to_string(), ident.value()) {
        ident.set_repr(quoted(ident.value()));
    }
}

/// The escaped, quoted text to print instead of `text`, if `text` stands for a string value and wouldn't read back
/// as it. kdl-rs 6.7.1 prints strings like "+.7" bare, which its own parser rejects, and copies code points KDL bans
/// into its quoted form unescaped.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only strings need fixing, whatever other values kdl-rs grows"
)]
pub(crate) fn fixed_repr(text: &str, value: &KdlValue) -> Option<String> {
    match value {
        KdlValue::String(s) if !reads_back_as(text, s) => Some(quoted(s)),
        _ => None,
    }
}

/// Whether `text` parses as the string `s` and holds no code point KDL bans literally. kdl-rs's parser accepts some
/// of those (U+007F), so the parse alone isn't enough.
pub(crate) fn reads_back_as(text: &str, s: &str) -> bool {
    !text.chars().any(is_disallowed)
        && KdlEntry::parse(text).is_ok_and(|e| e.name().is_none() && e.value().as_string() == Some(s))
}

/// The spec's "Disallowed Literal Code Points". Surrogates can't occur in a Rust string.
fn is_disallowed(c: char) -> bool {
    matches!(c,
        '\u{0000}'..='\u{0008}'
        | '\u{000E}'..='\u{001F}'
        | '\u{007F}'
        | '\u{200E}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}'
    )
}

/// The quoted form: kdl-rs's escapes, plus `\u{…}` for the newlines and banned code points a single-line string
/// can't hold literally.
pub(crate) fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => escape_into(c, &mut out),
        }
    }
    out.push('"');
    out
}

/// `c` as it goes inside a quoted string, single- or multi-line. `"` and `\n` are the caller's: the two kinds of
/// string treat them differently.
pub(crate) fn escape_into(c: char, out: &mut String) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{08}' => out.push_str("\\b"),
        '\u{0C}' => out.push_str("\\f"),
        c if matches!(c, '\u{0B}' | '\u{85}' | '\u{2028}' | '\u{2029}') || is_disallowed(c) => {
            out.extend(c.escape_unicode());
        }
        c => out.push(c),
    }
}

/// The same KDL value. Floats compare by their bits, so a NaN is the same as itself and `-0.0` isn't `0.0`.
pub(crate) fn same_value(a: &KdlValue, b: &KdlValue) -> bool {
    match (a, b) {
        (KdlValue::Float(a), KdlValue::Float(b)) => a.to_bits() == b.to_bits(),
        (a, b) => a == b,
    }
}

/// Whether `f` is in f32's range. f32 has infinities and NaN too.
fn fits_f32(f: f64) -> bool {
    !f.is_finite() || f.abs() <= f64::from(f32::MAX)
}

fn plain_to_python(py: Python<'_>, value: &KdlValue) -> PyResult<Py<PyAny>> {
    Ok(match value {
        KdlValue::String(s) => s.into_pyobject(py)?.into_any().unbind(),
        KdlValue::Integer(i) => i.into_pyobject(py)?.into_any().unbind(),
        KdlValue::Float(f) => f.into_pyobject(py)?.into_any().unbind(),
        KdlValue::Bool(b) => b.into_pyobject(py)?.to_owned().into_any().unbind(),
        KdlValue::Null => py.None(),
    })
}

fn is_exact_plain(value: &Bound<'_, PyAny>) -> bool {
    value.is_none()
        || value.is_exact_instance_of::<PyBool>()
        || value.is_exact_instance_of::<PyInt>()
        || value.is_exact_instance_of::<PyFloat>()
        || value.is_exact_instance_of::<PyString>()
}

/// None if the value isn't one of the plain types. An int outside i128 raises `OverflowError` and a str with lone
/// surrogates raises `UnicodeEncodeError`, a `ValueError`.
fn plain_to_kdl(value: &Bound<'_, PyAny>) -> PyResult<Option<KdlValue>> {
    Ok(Some(if value.is_none() {
        KdlValue::Null
    } else if value.is_instance_of::<PyBool>() {
        // bool is a subclass of int, so it has to be checked first.
        KdlValue::Bool(value.is_truthy()?)
    } else if value.is_instance_of::<PyInt>() {
        KdlValue::Integer(value.extract().map_err(|e: PyErr| {
            if e.is_instance_of::<PyOverflowError>(value.py()) {
                PyOverflowError::new_err("int doesn't fit in a KDL integer (i128)")
            } else {
                e
            }
        })?)
    } else if value.is_instance_of::<PyFloat>() {
        KdlValue::Float(value.extract()?)
    } else if value.is_instance_of::<PyString>() {
        KdlValue::String(value.extract()?)
    } else {
        return Ok(None);
    }))
}

/// None if the value isn't an instance of the Type's class.
fn encode_user(user: &UserType, value: &Bound<'_, PyAny>) -> PyResult<Option<KdlValue>> {
    let ty = user.ty.bind(value.py());
    if !value.is_instance(Type::hook(ty, |h| &h.cls)?.as_any())? {
        return Ok(None);
    }
    let plain = Type::hook(ty, |h| &h.encode)?.call1((value,))?;
    match plain_to_kdl(&plain)? {
        Some(kdl) => Ok(Some(kdl)),
        None => Err(PyTypeError::new_err(format!(
            "encode for type {:?} returned {}, not str, int, float, bool or None",
            user.name,
            plain.get_type().qualname()?
        ))),
    }
}

/// Whether the Type's decode accepts the plain value. Only an `Exception` counts as "no"; anything else, such as
/// `KeyboardInterrupt`, propagates.
fn decodes(user: &UserType, plain: &Bound<'_, PyAny>) -> PyResult<bool> {
    let py = plain.py();
    match Type::hook(user.ty.bind(py), |h| &h.decode)?.call1((plain,)) {
        Ok(_) => Ok(true),
        Err(err) if err.is_instance_of::<PyException>(py) => Ok(false),
        Err(err) => Err(err),
    }
}

/// Checks that a value reads as type `ty`, so annotating it with `ty` doesn't leave an entry that raises when read.
pub(crate) fn check_fits(py: Python<'_>, value: &KdlValue, ty: &str, types: &TypeRegistry) -> PyResult<()> {
    match to_python(py, value, Some(ty), types) {
        Ok(_) => Ok(()),
        Err(err) if err.is_instance_of::<PyValueError>(py) => Err(err),
        Err(err) if err.is_instance_of::<PyException>(py) => {
            let refused = PyValueError::new_err(format!(
                "{} doesn't fit {}: its decode raised {}",
                canonical_repr(value),
                annotation(ty),
                err.value(py).repr()?
            ));
            refused.set_cause(py, Some(err));
            Err(refused)
        }
        Err(err) => Err(err),
    }
}

fn call<'py>(py: Python<'py>, module: &str, attr: &str, arg: impl IntoPyObject<'py>) -> PyResult<Bound<'py, PyAny>> {
    py.import(module)?.getattr(attr)?.call1((arg,))
}

fn is_instance(value: &Bound<'_, PyAny>, module: &str, attr: &str) -> PyResult<bool> {
    value.is_instance(&value.py().import(module)?.getattr(attr)?)
}

/// The Err is the reason the value doesn't fit, for the caller's message.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "each type accepts one or two kinds of value and rejects the rest"
)]
fn decode_builtin(py: Python<'_>, b: Builtin, value: &KdlValue, text: Option<&str>) -> Result<Py<PyAny>, String> {
    let string = || match value {
        KdlValue::String(s) => Ok(s.as_str()),
        _ => Err("expected a string".to_owned()),
    };
    let decoded = match b {
        Builtin::Int { min, max } => {
            let KdlValue::Integer(i) = value else {
                return Err("expected an integer".to_owned());
            };
            if *i < min || *i > max {
                return Err(format!("out of range {min}..={max}"));
            }
            i.into_bound_py_any(py)
        }
        Builtin::F32 | Builtin::F64 => {
            let f = match value {
                #[expect(clippy::cast_precision_loss, reason = "an f64 annotation asks for the nearest float")]
                KdlValue::Integer(i) => *i as f64,
                KdlValue::Float(f) => *f,
                _ => return Err("expected a number".to_owned()),
            };
            if matches!(b, Builtin::F32) && !fits_f32(f) {
                return Err("out of range for f32".to_owned());
            }
            f.into_bound_py_any(py)
        }
        Builtin::DecimalNumber => {
            // The parsed value is only an f64, so the digits come from the literal's text. Without one (an entry
            // kdl-rs prints itself), the shortest text that round-trips is the next best thing: 0.1 becomes
            // Decimal("0.1"), not the binary expansion.
            let digits = match value {
                KdlValue::Integer(i) => i.to_string(),
                KdlValue::Float(f) if f.is_nan() => "NaN".to_owned(),
                KdlValue::Float(f) if *f == f64::INFINITY => "Infinity".to_owned(),
                KdlValue::Float(f) if *f == f64::NEG_INFINITY => "-Infinity".to_owned(),
                KdlValue::Float(f) => text.and_then(decimal_digits).unwrap_or_else(|| format!("{f:?}")),
                _ => return Err("expected a number".to_owned()),
            };
            call(py, "decimal", "Decimal", digits)
        }
        Builtin::DecimalString => call(py, "decimal", "Decimal", string()?),
        Builtin::DateTime => from_iso(py, "datetime", string()?),
        Builtin::Date => from_iso(py, "date", string()?),
        Builtin::Time => from_iso(py, "time", string()?),
        Builtin::Ipv4 => call(py, "ipaddress", "IPv4Address", string()?),
        Builtin::Ipv6 => call(py, "ipaddress", "IPv6Address", string()?),
        Builtin::Uuid => call(py, "uuid", "UUID", string()?),
        Builtin::Regex => call(py, "re", "compile", string()?),
        Builtin::Base64 => b64decode(py, string()?),
        Builtin::PlainString => string()?.into_bound_py_any(py),
    };
    decoded.map(Bound::unbind).map_err(|e| e.value(py).to_string())
}

/// A decimal float literal's digits as `Decimal` reads them, without the `_` separators. KDL's decimal syntax is a
/// subset of `Decimal`'s.
fn decimal_digits(text: &str) -> Option<String> {
    let digits = text.replace('_', "");
    digits
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))
        .then_some(digits)
}

fn from_iso<'py>(py: Python<'py>, class: &str, text: &str) -> PyResult<Bound<'py, PyAny>> {
    py.import("datetime")?
        .getattr(class)?
        .call_method1("fromisoformat", (text,))
}

fn b64decode<'py>(py: Python<'py>, text: &str) -> PyResult<Bound<'py, PyAny>> {
    // Without validate, b64decode silently skips characters outside the alphabet.
    let kwargs = PyDict::new(py);
    kwargs.set_item("validate", true)?;
    py.import("base64")?.getattr("b64decode")?.call((text,), Some(&kwargs))
}

/// The KDL value, and the text to print it as if the value alone doesn't say. None if the value doesn't fit the
/// type: the wrong class, or a number out of range.
fn encode_builtin(b: Builtin, value: &Bound<'_, PyAny>) -> PyResult<Option<(KdlValue, Option<String>)>> {
    let is_int = value.is_instance_of::<PyInt>() && !value.is_instance_of::<PyBool>();
    let string = |v: Bound<'_, PyAny>| -> PyResult<Option<(KdlValue, Option<String>)>> {
        Ok(Some((KdlValue::String(v.extract()?), None)))
    };
    match b {
        Builtin::Int { min, max } => {
            let whole = if is_int {
                value.extract::<i128>().ok()
            } else if value.is_instance_of::<PyFloat>() {
                whole_float(value.extract()?)
            } else {
                None
            };
            Ok(whole
                .filter(|i| (min..=max).contains(i))
                .map(|i| (KdlValue::Integer(i), None)))
        }
        Builtin::F32 | Builtin::F64 => {
            if !value.is_instance_of::<PyFloat>() && !is_int {
                return Ok(None);
            }
            // An int too big for a float doesn't fit.
            let Ok(f) = value.extract::<f64>() else {
                return Ok(None);
            };
            if matches!(b, Builtin::F32) && !fits_f32(f) {
                return Ok(None);
            }
            Ok(Some((KdlValue::Float(f), None)))
        }
        Builtin::DecimalNumber if is_instance(value, "decimal", "Decimal")? => decimal_number(value),
        Builtin::DecimalString if is_instance(value, "decimal", "Decimal")? => string(value.str()?.into_any()),
        Builtin::DateTime if is_instance(value, "datetime", "datetime")? => string(value.call_method0("isoformat")?),
        Builtin::Date if is_instance(value, "datetime", "date")? && !is_instance(value, "datetime", "datetime")? => {
            string(value.call_method0("isoformat")?)
        }
        Builtin::Time if is_instance(value, "datetime", "time")? => string(value.call_method0("isoformat")?),
        Builtin::Ipv4 if is_instance(value, "ipaddress", "IPv4Address")? => string(value.str()?.into_any()),
        Builtin::Ipv6 if is_instance(value, "ipaddress", "IPv6Address")? => string(value.str()?.into_any()),
        Builtin::Uuid if is_instance(value, "uuid", "UUID")? => string(value.str()?.into_any()),
        Builtin::Regex if is_instance(value, "re", "Pattern")? => {
            // A bytes pattern has no KDL spelling.
            let pattern = value.getattr("pattern")?;
            if !pattern.is_instance_of::<PyString>() {
                return Ok(None);
            }
            // Only the pattern is written, so flags passed to compile() would be lost. Inline ones like `(?i)` are
            // part of the pattern, and compiling the pattern alone gives them back.
            let flags: u32 = value.getattr("flags")?.extract()?;
            let own: u32 = call(value.py(), "re", "compile", &pattern)?
                .getattr("flags")?
                .extract()?;
            let extra = flags & !own;
            if extra != 0 {
                let names = call(value.py(), "re", "RegexFlag", extra)?.str()?;
                return Err(PyValueError::new_err(format!(
                    "can't write a regex compiled with {names}: KDL keeps only the pattern, so put flags inline, like (?i)"
                )));
            }
            string(pattern)
        }
        Builtin::Base64 if value.is_instance_of::<PyBytes>() => {
            string(call(value.py(), "base64", "b64encode", value)?.call_method1("decode", ("ascii",))?)
        }
        Builtin::PlainString if value.is_instance_of::<PyString>() => string(value.clone()),
        Builtin::DecimalNumber
        | Builtin::DecimalString
        | Builtin::DateTime
        | Builtin::Date
        | Builtin::Time
        | Builtin::Ipv4
        | Builtin::Ipv6
        | Builtin::Uuid
        | Builtin::Regex
        | Builtin::Base64
        | Builtin::PlainString => Ok(None),
    }
}

/// Why a number doesn't fit a numeric built-in type, if `value` is an `int` or `float` (`bool` isn't a number here)
/// and `b` holds a range. None for anything else, which keeps the "infer the annotation" rule.
#[expect(clippy::wildcard_enum_match_arm, reason = "only the numeric types have a range")]
fn misfit(b: Builtin, value: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    let is_int = value.is_instance_of::<PyInt>() && !value.is_instance_of::<PyBool>();
    let is_float = value.is_instance_of::<PyFloat>();
    Ok(match b {
        Builtin::Int { .. } if is_float => {
            let f: f64 = value.extract()?;
            // A whole float converts, so one that gets here is either not whole or out of range.
            if f.is_finite() && f.fract() == 0.0 {
                range_of(b)
            } else {
                Some("not a whole number".to_owned())
            }
        }
        Builtin::Int { .. } | Builtin::F32 | Builtin::F64 if is_int || is_float => range_of(b),
        _ => None,
    })
}

/// "out of range ..." for a numeric built-in type.
#[expect(clippy::wildcard_enum_match_arm, reason = "only the numeric types have a range")]
fn range_of(b: Builtin) -> Option<String> {
    match b {
        Builtin::Int { min, max } => Some(format!("out of range {min}..={max}")),
        Builtin::F32 => Some("out of range ±3.4028234663852886e+38".to_owned()),
        Builtin::F64 => Some("out of range ±1.7976931348623157e+308".to_owned()),
        _ => None,
    }
}

/// An annotation spelled the way a file does: `(u8)`, or `("my type")`.
fn annotation(ty: &str) -> String {
    format!("({})", identifier(ty))
}

/// A float that's a whole number in i128's range, as that integer. A float over an integer annotation converts, as an
/// int over a float annotation does.
fn whole_float(f: f64) -> Option<i128> {
    // i128::MAX as f64 rounds up to 2^127, which is out of range, so the upper bound is exclusive.
    #[expect(clippy::cast_precision_loss, reason = "-2^127 and 2^127 are exact in an f64")]
    let (low, high) = (i128::MIN as f64, i128::MAX as f64);
    #[expect(clippy::cast_possible_truncation, reason = "checked whole and in range just above")]
    (f.is_finite() && f.fract() == 0.0 && f >= low && f < high).then_some(f as i128)
}

/// A `Decimal` as a `decimal64` or `decimal128`, written with its own digits so none are lost to the f64 kdl-rs
/// keeps. Only the range is checked, against that f64. None if it's out of range.
fn decimal_number(value: &Bound<'_, PyAny>) -> PyResult<Option<(KdlValue, Option<String>)>> {
    let f: f64 = value.call_method0("__float__")?.extract()?;
    if !value.call_method0("is_finite")?.is_truthy()? {
        return Ok(Some((KdlValue::Float(f), None)));
    }
    if !f.is_finite() {
        return Ok(None);
    }
    // `str` gives the Decimal's own digits, which are a KDL number unless they're an integer too large for i128, or
    // `-0`, which would read back as `0`. Those get the float's text.
    let digits: String = value.str()?.extract()?;
    let parsed = KdlEntry::parse(&digits)
        .ok()
        .filter(|e| e.name().is_none() && e.ty().is_none());
    let spelled = parsed.and_then(|e| {
        let read = match *e.value() {
            #[expect(
                clippy::cast_precision_loss,
                reason = "compared with the f64 Python rounds the Decimal to"
            )]
            KdlValue::Integer(i) => i as f64,
            KdlValue::Float(g) => g,
            KdlValue::String(_) | KdlValue::Bool(_) | KdlValue::Null => return None,
        };
        (read.to_bits() == f.to_bits()).then(|| e.value().clone())
    });
    Ok(Some(match spelled {
        Some(kdl) => (kdl, Some(digits)),
        None => (KdlValue::Float(f), None),
    }))
}

fn registry(py: Python<'_>, types: Option<Vec<Py<Type>>>) -> TypeRegistry {
    types.map_or_else(TypeRegistry::builtin, |types| TypeRegistry::with_user_types(py, types))
}

/// Whether this build runs the debug-only checks, such as the id-tree shape check after every edit. `test_build.py`
/// asserts it, so the Python suite can't silently run against a release build.
#[pyfunction]
pub(crate) fn _debug_assertions() -> bool {
    cfg!(debug_assertions)
}

/// The built-in reserved type names, each with whether its value stays a plain `str`, so the tests don't keep a hand
/// copy of the table.
#[pyfunction]
pub(crate) fn _debug_reserved_types() -> Vec<(&'static str, bool)> {
    BUILTINS
        .iter()
        .map(|&(name, b)| (name, matches!(b, Builtin::PlainString)))
        .collect()
}

/// Direct conversion hook for `test_values.py`: parses one entry and converts its value.
#[pyfunction]
#[pyo3(signature = (kdl_entry_text, types=None))]
pub(crate) fn _debug_to_python(
    py: Python<'_>,
    kdl_entry_text: &str,
    types: Option<Vec<Py<Type>>>,
) -> PyResult<Py<PyAny>> {
    let entry = KdlEntry::parse(kdl_entry_text).map_err(|e| parse_error(py, &e))?;
    let types = registry(py, types);
    let text = entry.format().map(|f| f.value_repr.as_str());
    to_python_spelled(py, entry.value(), text, entry.ty().map(KdlIdentifier::value), &types)
}

/// Direct conversion hook for `test_values.py`: returns `(text, annotation)`, the text a new entry would print.
#[pyfunction]
#[pyo3(signature = (value, current_type=None, types=None))]
pub(crate) fn _debug_from_python(
    value: &Bound<'_, PyAny>,
    current_type: Option<&str>,
    types: Option<Vec<Py<Type>>>,
) -> PyResult<(String, Option<String>)> {
    let types = registry(value.py(), types);
    let encoded = encode(value, current_type, &types)?;
    Ok((
        encoded.repr.unwrap_or_else(|| canonical_repr(&encoded.value)),
        encoded.ty,
    ))
}
