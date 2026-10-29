use std::collections::HashMap;

use kdl::{KdlEntry, KdlEntryFormat, KdlIdentifier, KdlValue};
use pyo3::IntoPyObjectExt;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyString, PyType};

use crate::errors::parse_error;

/// One type annotation that converts values. Exposed to Python as rkdl.Type(name, cls, decode, encode).
#[pyclass(module = "rkdl", frozen)]
pub struct Type {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    cls: Py<PyType>,
    #[pyo3(get)]
    decode: Py<PyAny>,
    #[pyo3(get)]
    encode: Py<PyAny>,
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
            cls,
            decode,
            encode,
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let cls = self.cls.bind(py).qualname()?;
        Ok(format!("Type({:?}, {cls})", self.name))
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

const BUILTINS: &[(&str, Builtin)] = &[
    (
        "i8",
        Builtin::Int {
            min: i8::MIN as i128,
            max: i8::MAX as i128,
        },
    ),
    (
        "i16",
        Builtin::Int {
            min: i16::MIN as i128,
            max: i16::MAX as i128,
        },
    ),
    (
        "i32",
        Builtin::Int {
            min: i32::MIN as i128,
            max: i32::MAX as i128,
        },
    ),
    (
        "i64",
        Builtin::Int {
            min: i64::MIN as i128,
            max: i64::MAX as i128,
        },
    ),
    (
        "i128",
        Builtin::Int {
            min: i128::MIN,
            max: i128::MAX,
        },
    ),
    (
        "isize",
        Builtin::Int {
            min: i64::MIN as i128,
            max: i64::MAX as i128,
        },
    ),
    (
        "u8",
        Builtin::Int {
            min: 0,
            max: u8::MAX as i128,
        },
    ),
    (
        "u16",
        Builtin::Int {
            min: 0,
            max: u16::MAX as i128,
        },
    ),
    (
        "u32",
        Builtin::Int {
            min: 0,
            max: u32::MAX as i128,
        },
    ),
    (
        "u64",
        Builtin::Int {
            min: 0,
            max: u64::MAX as i128,
        },
    ),
    // kdl-rs stores integers as i128, so the top half of u128 can't be represented anyway.
    ("u128", Builtin::Int { min: 0, max: i128::MAX }),
    (
        "usize",
        Builtin::Int {
            min: 0,
            max: u64::MAX as i128,
        },
    ),
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

enum Converter {
    Builtin(Builtin),
    User(Py<Type>),
}

/// Built-in reserved types plus any user Types. Immutable once built, shared by Arc.
pub struct TypeRegistry {
    by_name: HashMap<String, Converter>,
    // In the order given, which is the order inference tries them.
    user: Vec<Py<Type>>,
}

impl TypeRegistry {
    pub fn builtin() -> Self {
        TypeRegistry {
            by_name: BUILTINS
                .iter()
                .map(|&(name, b)| (name.to_owned(), Converter::Builtin(b)))
                .collect(),
            user: Vec::new(),
        }
    }

    pub fn with_user_types(py: Python<'_>, types: Vec<Py<Type>>) -> PyResult<Self> {
        let mut registry = TypeRegistry::builtin();
        for ty in types {
            registry
                .by_name
                .insert(ty.get().name.clone(), Converter::User(ty.clone_ref(py)));
            registry.user.push(ty);
        }
        Ok(registry)
    }

    /// User types still registered under their name, in order. A later Type with the same name shadows an earlier one.
    fn active_user_types(&self) -> impl Iterator<Item = &Py<Type>> {
        self.user
            .iter()
            .filter(|ty| matches!(self.by_name.get(&ty.get().name), Some(Converter::User(current)) if current.is(*ty)))
    }
}

/// KDL value plus its entry's type annotation -> Python object. Raises if a known type's contents don't fit.
pub fn to_python(py: Python<'_>, value: &KdlValue, ty: Option<&str>, types: &TypeRegistry) -> PyResult<Py<PyAny>> {
    let Some(ty) = ty else {
        return plain_to_python(py, value);
    };
    match types.by_name.get(ty) {
        None => plain_to_python(py, value),
        Some(Converter::User(user)) => {
            let plain = plain_to_python(py, value)?;
            user.get().decode.call1(py, (plain,))
        }
        Some(Converter::Builtin(b)) => decode_builtin(py, *b, value).map_err(|reason| {
            PyValueError::new_err(format!("({ty}){} is not a valid {ty}: {reason}", canonical_repr(value)))
        }),
    }
}

/// Python object -> KDL value plus the annotation to write, applying the "keep the current annotation if the value
/// fits it" rule from plan.md.
pub fn from_python(
    value: &Bound<'_, PyAny>,
    current_ty: Option<&str>,
    types: &TypeRegistry,
) -> PyResult<(KdlValue, Option<String>)> {
    let py = value.py();
    if let Some(ty) = current_ty {
        let kept = match types.by_name.get(ty) {
            // An unknown annotation is the user's own label, so any plain value keeps it.
            None => plain_to_kdl(value)?,
            Some(Converter::Builtin(b)) => encode_builtin(*b, value)?,
            Some(Converter::User(user)) => encode_user(user.bind(py), value)?,
        };
        if let Some(kdl) = kept {
            return Ok((kdl, Some(ty.to_owned())));
        }
    }

    if is_exact_plain(value) {
        let kdl = plain_to_kdl(value)?.expect("exact plain types always convert");
        return Ok((kdl, None));
    }
    for user in types.active_user_types() {
        if let Some(kdl) = encode_user(user.bind(py), value)? {
            return Ok((kdl, Some(user.get().name.clone())));
        }
    }
    for &(name, b) in INFERRED {
        if !matches!(types.by_name.get(name), Some(Converter::Builtin(_))) {
            continue;
        }
        if let Some(kdl) = encode_builtin(b, value)? {
            return Ok((kdl, Some(name.to_owned())));
        }
    }
    // Subclasses of the plain types, such as an IntEnum no Type claimed.
    if let Some(kdl) = plain_to_kdl(value)? {
        return Ok((kdl, None));
    }
    Err(PyTypeError::new_err(format!(
        "can't convert {} to a KDL value",
        value.get_type().qualname()?
    )))
}

/// Canonical KDL text for a value, written into KdlEntryFormat::value_repr after every value write.
pub fn canonical_repr(value: &KdlValue) -> String {
    let text = value.to_string();
    fixed_repr(&text, value).unwrap_or(text)
}

/// Sets the text an entry's value prints as. Only `value_repr` changes: the rest of the format holds the entry's
/// whitespace and any `/-` comment in front of it. An entry without a format prints its value through kdl-rs, so it
/// needs one only when `repr` differs from kdl-rs's text, and then the format has to carry the space in front of the
/// entry that kdl-rs would have printed.
pub fn set_value_repr(entry: &mut KdlEntry, repr: String) {
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
pub fn identifier(name: impl Into<String>) -> KdlIdentifier {
    let mut ident = KdlIdentifier::from(name.into());
    fix_identifier(&mut ident);
    ident
}

/// Gives an identifier the escaped, quoted spelling if the text it prints wouldn't read back as its value.
pub fn fix_identifier(ident: &mut KdlIdentifier) {
    if !reads_back_as(&ident.to_string(), ident.value()) {
        ident.set_repr(quoted(ident.value()));
    }
}

/// The escaped, quoted text to print instead of `text`, if `text` stands for a string value and wouldn't read back
/// as it. kdl-rs 6.7.1 prints strings like "+.7" bare, which its own parser rejects, and copies code points KDL bans
/// into its quoted form unescaped.
pub fn fixed_repr(text: &str, value: &KdlValue) -> Option<String> {
    match value {
        KdlValue::String(s) if !reads_back_as(text, s) => Some(quoted(s)),
        _ => None,
    }
}

/// Whether `text` parses as the string `s` and holds no code point KDL bans literally. kdl-rs's parser accepts some
/// of those (U+007F), so the parse alone isn't enough.
fn reads_back_as(text: &str, s: &str) -> bool {
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
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\u{0B}' | '\u{85}' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c if is_disallowed(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
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

/// None if the value isn't one of the plain types. An int outside i128 raises OverflowError and a str with lone
/// surrogates raises UnicodeEncodeError, a ValueError.
fn plain_to_kdl(value: &Bound<'_, PyAny>) -> PyResult<Option<KdlValue>> {
    Ok(Some(if value.is_none() {
        KdlValue::Null
    } else if value.is_instance_of::<PyBool>() {
        // bool is a subclass of int, so it has to be checked first.
        KdlValue::Bool(value.is_truthy()?)
    } else if value.is_instance_of::<PyInt>() {
        KdlValue::Integer(value.extract()?)
    } else if value.is_instance_of::<PyFloat>() {
        KdlValue::Float(value.extract()?)
    } else if value.is_instance_of::<PyString>() {
        KdlValue::String(value.extract()?)
    } else {
        return Ok(None);
    }))
}

/// None if the value isn't an instance of the Type's class.
fn encode_user(ty: &Bound<'_, Type>, value: &Bound<'_, PyAny>) -> PyResult<Option<KdlValue>> {
    let ty = ty.get();
    let py = value.py();
    if !value.is_instance(ty.cls.bind(py))? {
        return Ok(None);
    }
    let plain = ty.encode.bind(py).call1((value,))?;
    match plain_to_kdl(&plain)? {
        Some(kdl) => Ok(Some(kdl)),
        None => Err(PyTypeError::new_err(format!(
            "encode for type {:?} returned {}, not str, int, float, bool or None",
            ty.name,
            plain.get_type().qualname()?
        ))),
    }
}

fn call<'py>(py: Python<'py>, module: &str, attr: &str, arg: impl IntoPyObject<'py>) -> PyResult<Bound<'py, PyAny>> {
    py.import(module)?.getattr(attr)?.call1((arg,))
}

fn is_instance(value: &Bound<'_, PyAny>, module: &str, attr: &str) -> PyResult<bool> {
    value.is_instance(&value.py().import(module)?.getattr(attr)?)
}

/// The Err is the reason the value doesn't fit, for the caller's message.
fn decode_builtin(py: Python<'_>, b: Builtin, value: &KdlValue) -> Result<Py<PyAny>, String> {
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
                KdlValue::Integer(i) => *i as f64,
                KdlValue::Float(f) => *f,
                _ => return Err("expected a number".to_owned()),
            };
            if matches!(b, Builtin::F32) && f.is_finite() && f.abs() > f32::MAX as f64 {
                return Err("out of range for f32".to_owned());
            }
            f.into_bound_py_any(py)
        }
        Builtin::DecimalNumber => {
            // The literal's own text isn't available here, only the parsed value. The shortest text that
            // round-trips is the next best thing: 0.1 becomes Decimal("0.1"), not the binary expansion.
            let text = match value {
                KdlValue::Integer(i) => i.to_string(),
                KdlValue::Float(f) if f.is_nan() => "NaN".to_owned(),
                KdlValue::Float(f) if *f == f64::INFINITY => "Infinity".to_owned(),
                KdlValue::Float(f) if *f == f64::NEG_INFINITY => "-Infinity".to_owned(),
                KdlValue::Float(f) => format!("{f:?}"),
                _ => return Err("expected a number".to_owned()),
            };
            call(py, "decimal", "Decimal", text)
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

/// None if the value doesn't fit the type: the wrong class, or an integer out of range.
fn encode_builtin(b: Builtin, value: &Bound<'_, PyAny>) -> PyResult<Option<KdlValue>> {
    let is_int = value.is_instance_of::<PyInt>() && !value.is_instance_of::<PyBool>();
    let string = |v: Bound<'_, PyAny>| -> PyResult<Option<KdlValue>> { Ok(Some(KdlValue::String(v.extract()?))) };
    match b {
        Builtin::Int { min, max } => Ok(match is_int.then(|| value.extract::<i128>()) {
            Some(Ok(i)) if (min..=max).contains(&i) => Some(KdlValue::Integer(i)),
            _ => None,
        }),
        Builtin::F32 | Builtin::F64 => {
            if !value.is_instance_of::<PyFloat>() && !is_int {
                return Ok(None);
            }
            // An int too big for a float doesn't fit.
            let Ok(f) = value.extract::<f64>() else {
                return Ok(None);
            };
            if matches!(b, Builtin::F32) && f.is_finite() && f.abs() > f32::MAX as f64 {
                return Ok(None);
            }
            Ok(Some(KdlValue::Float(f)))
        }
        Builtin::DecimalNumber if is_instance(value, "decimal", "Decimal")? => {
            Ok(Some(KdlValue::Float(value.call_method0("__float__")?.extract()?)))
        }
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
            string(pattern)
        }
        Builtin::Base64 if value.is_instance_of::<PyBytes>() => {
            string(call(value.py(), "base64", "b64encode", value)?.call_method1("decode", ("ascii",))?)
        }
        Builtin::PlainString if value.is_instance_of::<PyString>() => string(value.clone()),
        _ => Ok(None),
    }
}

fn registry(py: Python<'_>, types: Option<Vec<Py<Type>>>) -> PyResult<TypeRegistry> {
    match types {
        Some(types) => TypeRegistry::with_user_types(py, types),
        None => Ok(TypeRegistry::builtin()),
    }
}

/// Test hook until the handle API exists: parses one entry and converts its value.
#[pyfunction]
#[pyo3(signature = (kdl_entry_text, types=None))]
pub fn _debug_to_python(py: Python<'_>, kdl_entry_text: &str, types: Option<Vec<Py<Type>>>) -> PyResult<Py<PyAny>> {
    let entry = KdlEntry::parse(kdl_entry_text).map_err(|e| parse_error(py, &e))?;
    let types = registry(py, types)?;
    to_python(py, entry.value(), entry.ty().map(|t| t.value()), &types)
}

/// Test hook until the handle API exists: returns (canonical_repr, annotation).
#[pyfunction]
#[pyo3(signature = (value, current_type=None, types=None))]
pub fn _debug_from_python(
    value: &Bound<'_, PyAny>,
    current_type: Option<&str>,
    types: Option<Vec<Py<Type>>>,
) -> PyResult<(String, Option<String>)> {
    let types = registry(value.py(), types)?;
    let (kdl, ty) = from_python(value, current_type, &types)?;
    Ok((canonical_repr(&kdl), ty))
}
