// SPDX-License-Identifier: MIT OR Apache-2.0
//! Data-driven Lexicon validator over `serde_json::Value` (`/specs/lexicon`, `/specs/data-model`).
//!
//! The validator walks a JSON value against a compiled schema from the [`Registry`] and returns
//! **every** violation with a JSON path (`$.body.media[0].alt`). Semantics:
//!
//! | Type | Checks |
//! |---|---|
//! | `object` | JSON object; `required` present; `null` only for `nullable` fields; unknown fields allowed and not inspected |
//! | `record` | object whose `$type` equals the collection NSID; optional record-key type check |
//! | `string` | JSON string; `const`, `enum` (closed); `minLength`/`maxLength` in UTF-8 bytes; `minGraphemes`/`maxGraphemes` in extended grapheme clusters; `format`. `knownValues` are open and never enforced; `default` is informational |
//! | `integer` | JSON integer (an integral float such as `3.0` within ±(2^53−1) counts, as in the data model); `const`, `enum`, `minimum`, `maximum` |
//! | `boolean` | JSON boolean; `const` |
//! | `bytes` | `{"$bytes": base64}`, exactly one key; decoded `minLength`/`maxLength` |
//! | `cid-link` | `{"$link": cid}`, exactly one key, canonical CID string |
//! | `blob` | typed `{"$type":"blob","ref":{"$link"},"mimeType","size"}`; `accept` globs against the MIME essence; `maxSize` against `size`. The legacy `{"cid","mimeType"}` form is accepted for reading unless [`Options::allow_legacy_blobs`] is off |
//! | `array` | JSON array; `minLength`/`maxLength` (items); each item |
//! | `ref` | local (`#def`) and cross-document (`nsid#def`) references, resolved in the registry |
//! | `union` | object with a string `$type`; a known member is validated against its def; an unknown `$type` passes an open union (the default) and fails a closed one |
//! | `unknown` | any JSON object |
//!
//! **Bounded work.** Nesting is limited to [`Options::max_depth`] containers (32, rule G10:
//! `$link` and `$bytes` objects are leaves, as in the data model, so a value has one depth in JSON
//! and DAG-CBOR). The value's depth is measured first, iteratively; the walk never descends past
//! the limit. Chains of `ref` → `ref` are cut after [`MAX_REF_HOPS`] hops, the number of visited
//! values is capped by [`Options::max_steps`] and the number of reported violations by
//! [`Options::max_violations`] (the report says when it was truncated). Validation never panics.

pub mod formats;

use lessdugin_atp::syntax::RecordKey;
use serde_json::{Map, Value};
use unicode_segmentation::UnicodeSegmentation;

use crate::collection::type_matches;
use crate::schemas::{
    ArrayDef, BlobDef, BytesDef, Def, IntegerDef, ObjectDef, Registry, StringDef, TypeDef,
    UnionDef, registry,
};

/// Maximum consecutive `ref` → `ref` resolutions without consuming a value level.
pub const MAX_REF_HOPS: usize = 16;

/// The largest integer exactly representable in binary64, the data model's float bound.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Validator options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Accept the legacy blob form `{"cid": …, "mimeType": …}` (the Lexicon spec allows it for
    /// reading). lessdugin records reject it (rule G3).
    pub allow_legacy_blobs: bool,
    /// Maximum nesting depth (containers on the path from the root; the root object is 1).
    pub max_depth: usize,
    /// Maximum number of values visited before validation stops with a [`Code::Budget`]
    /// violation.
    pub max_steps: usize,
    /// Maximum number of violations reported; further ones only set [`Report::truncated`].
    pub max_violations: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            allow_legacy_blobs: true,
            max_depth: crate::limits::NESTING_MAX_RECORD,
            max_steps: 4_000_000,
            max_violations: 1000,
        }
    }
}

/// What kind of rule a [`Violation`] breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// Wrong JSON type (including `null` for a non-nullable field and floats for integers).
    Type,
    /// A required field is missing.
    Required,
    /// A string does not match its `format`.
    Format,
    /// String byte length, array length or bytes length outside `minLength`/`maxLength`.
    Length,
    /// Grapheme count outside `minGraphemes`/`maxGraphemes`.
    Graphemes,
    /// Integer outside `minimum`/`maximum`.
    Range,
    /// Value not in a closed `enum`.
    Enum,
    /// Value differs from `const`.
    Const,
    /// Malformed `$bytes` object.
    Bytes,
    /// Malformed `$link` object.
    Link,
    /// Malformed blob object.
    Blob,
    /// Blob MIME type not in `accept`.
    BlobAccept,
    /// Blob `size` above `maxSize`.
    BlobSize,
    /// Legacy `{cid, mimeType}` blob where only the typed form is allowed.
    LegacyBlob,
    /// Union member without a string `$type`, or an unknown member of a closed union.
    Union,
    /// A record's `$type` is missing or differs from the collection NSID.
    RecordType,
    /// The record key does not match the record's key type.
    RecordKey,
    /// Nesting deeper than [`Options::max_depth`].
    Depth,
    /// The schema is missing or broken (unresolved reference, too many ref hops, …).
    Schema,
    /// [`Options::max_steps`] exceeded.
    Budget,
}

impl Code {
    /// A short stable name (`"maxLength"`-style names are folded into `"length"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Required => "required",
            Self::Format => "format",
            Self::Length => "length",
            Self::Graphemes => "graphemes",
            Self::Range => "range",
            Self::Enum => "enum",
            Self::Const => "const",
            Self::Bytes => "bytes",
            Self::Link => "link",
            Self::Blob => "blob",
            Self::BlobAccept => "blobAccept",
            Self::BlobSize => "blobSize",
            Self::LegacyBlob => "legacyBlob",
            Self::Union => "union",
            Self::RecordType => "recordType",
            Self::RecordKey => "recordKey",
            Self::Depth => "depth",
            Self::Schema => "schema",
            Self::Budget => "budget",
        }
    }
}

impl core::fmt::Display for Code {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation: where (a JSON path such as `$.chapters[2].posts[0]`), what kind, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// JSON path: `$` is the root; `.key` for identifier-like keys, `["key"]` otherwise, `[i]`
    /// for array items.
    pub path: String,
    /// The kind of violation.
    pub code: Code,
    /// A human-readable explanation.
    pub message: String,
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {} ({})", self.path, self.message, self.code)
    }
}

/// The result of a validation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Every violation found, in walk order.
    pub violations: Vec<Violation>,
    /// `true` if violations were dropped ([`Options::max_violations`]) or the walk stopped early
    /// ([`Options::max_steps`]).
    pub truncated: bool,
}

impl Report {
    /// `true` if there is no violation.
    pub fn is_valid(&self) -> bool {
        self.violations.is_empty() && !self.truncated
    }

    /// `true` if some violation has this code.
    pub fn has(&self, code: Code) -> bool {
        self.violations.iter().any(|v| v.code == code)
    }
}

/// Validates a record value against its collection's lexicon (global registry, default options).
pub fn validate_record(nsid: &str, value: &Value) -> Report {
    Validator::new(registry(), Options::default()).record(nsid, None, value)
}

/// Validates a value against a definition reference (`nsid` or `nsid#name`), for example a
/// `defs#media` object (global registry, default options).
pub fn validate_def(reference: &str, value: &Value) -> Report {
    Validator::new(registry(), Options::default()).def(reference, value)
}

/// A validator bound to a registry and options.
#[derive(Debug, Clone, Copy)]
pub struct Validator<'r> {
    registry: &'r Registry,
    options: Options,
}

impl<'r> Validator<'r> {
    /// A validator over `registry`.
    pub fn new(registry: &'r Registry, options: Options) -> Self {
        Self { registry, options }
    }

    /// Validates a record: `value` must be an object whose `$type` is `nsid`, valid against the
    /// record schema; if `rkey` is given it must be a record key of the definition's key type.
    pub fn record(&self, nsid: &str, rkey: Option<&str>, value: &Value) -> Report {
        let mut w = Walker::new(self.registry, &self.options, value);
        let Some(rd) = self.registry.record(nsid) else {
            w.push(Code::Schema, format!("no record lexicon for {nsid:?}"));
            return w.finish();
        };
        if let Some(k) = rkey {
            match RecordKey::parse(k) {
                Err(e) => w.push(Code::RecordKey, format!("invalid record key {k:?}: {e}")),
                Ok(key) => {
                    if let Err(e) = rd.key.check(&key) {
                        w.push(
                            Code::RecordKey,
                            format!("record key {k:?} is not of type {}: {e}", rd.key),
                        );
                    }
                }
            }
        }
        match value.as_object() {
            None => w.push(Code::Type, "a record must be a JSON object".into()),
            Some(m) => {
                match m.get("$type") {
                    None => w.push_at("$type", Code::RecordType, "missing $type".into()),
                    Some(Value::String(t)) if t == nsid => {}
                    Some(Value::String(t)) => w.push_at(
                        "$type",
                        Code::RecordType,
                        format!("$type {t:?} is not the collection {nsid:?}"),
                    ),
                    Some(_) => {
                        w.push_at("$type", Code::RecordType, "$type must be a string".into())
                    }
                }
                w.object(&rd.record, value, 0);
            }
        }
        w.finish()
    }

    /// Validates `value` against the definition `reference` (`nsid` for `main`, or `nsid#name`).
    pub fn def(&self, reference: &str, value: &Value) -> Report {
        let mut w = Walker::new(self.registry, &self.options, value);
        let r = if reference.contains('#') {
            reference.to_owned()
        } else {
            format!("{reference}#main")
        };
        w.reference(&r, value, 0, 0);
        w.finish()
    }

    /// Validates a type directly (for callers holding a [`TypeDef`]).
    pub fn type_def(&self, ty: &TypeDef, value: &Value) -> Report {
        let mut w = Walker::new(self.registry, &self.options, value);
        w.check(ty, value, 0, 0);
        w.finish()
    }

    /// Validates the JSON output of an XRPC query or procedure `nsid` against its output schema.
    pub fn output(&self, nsid: &str, value: &Value) -> Report {
        let mut w = Walker::new(self.registry, &self.options, value);
        let schema = match self.registry.def(nsid) {
            Some(Def::Query(x) | Def::Procedure(x)) => {
                x.output.as_ref().and_then(|b| b.schema.as_ref())
            }
            _ => None,
        };
        match schema {
            Some(t) => w.check(t, value, 0, 0),
            None => w.push(Code::Schema, format!("no JSON output schema for {nsid:?}")),
        }
        w.finish()
    }

    /// A copy of `value` with the lexicon `default` of every absent boolean, integer and string
    /// property filled in, following the schema of `reference` (`nsid` or `nsid#name`) through
    /// objects, refs, known union members and array items, up to [`Options::max_depth`].
    ///
    /// This is the Lexicon "default" semantics for **reading** (for example `forum.membership`
    /// absent reads as `approval`). Never write the result back: it changes the record's CID,
    /// and writers keep the value as read (W3).
    pub fn with_defaults(&self, reference: &str, value: &Value) -> Value {
        let mut out = value.clone();
        let r = if reference.contains('#') {
            reference.to_owned()
        } else {
            format!("{reference}#main")
        };
        self.fill_ref(&r, &mut out, 0, 0);
        out
    }

    fn fill_ref(&self, r: &str, v: &mut Value, depth: usize, hops: usize) {
        if hops >= MAX_REF_HOPS {
            return;
        }
        match self.registry.def(r) {
            Some(Def::Type(t)) => self.fill(t, v, depth, hops + 1),
            Some(Def::Record(rd)) => self.fill_object(&rd.record, v, depth),
            _ => {}
        }
    }

    fn fill(&self, t: &TypeDef, v: &mut Value, depth: usize, hops: usize) {
        match t {
            TypeDef::Object(o) => self.fill_object(o, v, depth),
            TypeDef::Ref(r) => self.fill_ref(r, v, depth, hops),
            TypeDef::Union(u) => {
                let member = v
                    .get("$type")
                    .and_then(Value::as_str)
                    .and_then(|t| u.refs.iter().find(|r| type_matches(t, r)));
                if let Some(r) = member {
                    self.fill_ref(r, v, depth, 0);
                }
            }
            TypeDef::Array(a) => {
                if let Value::Array(items) = v
                    && depth < self.options.max_depth
                {
                    for item in items {
                        self.fill(&a.items, item, depth + 1, 0);
                    }
                }
            }
            _ => {}
        }
    }

    fn fill_object(&self, o: &ObjectDef, v: &mut Value, depth: usize) {
        let Value::Object(m) = v else {
            return;
        };
        if depth >= self.options.max_depth {
            return;
        }
        for (name, t) in &o.properties {
            match m.get_mut(name) {
                Some(child) => self.fill(t, child, depth + 1, 0),
                None => {
                    let d = match t {
                        TypeDef::Boolean(b) => b.default.map(Value::from),
                        TypeDef::Integer(i) => i.default.map(Value::from),
                        TypeDef::String(s) => s.default.as_deref().map(Value::from),
                        _ => None,
                    };
                    if let Some(d) = d {
                        m.insert(name.clone(), d);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Depth

/// `true` for a `{"$link": "…"}` or `{"$bytes": "…"}` object: a leaf in the data model.
fn is_data_model_leaf(m: &Map<String, Value>) -> bool {
    m.len() == 1
        && (m.get("$link").is_some_and(Value::is_string)
            || m.get("$bytes").is_some_and(Value::is_string))
}

/// The nesting depth of a JSON value: the number of containers (objects and arrays) on the
/// longest path from the root, so `{}` is 1 and a scalar 0 (`limits.md` §1). `$link` and
/// `$bytes` objects are leaves, as in DAG-CBOR. Iterative; the walk stops as soon as `cap` is
/// exceeded, returning `cap + 1` and the path of the first too-deep container (document order).
pub fn nesting_depth(v: &Value, cap: usize) -> (usize, Option<String>) {
    // (value, number of enclosing containers, the value's segment in its parent)
    let mut stack: Vec<(&Value, usize, Option<Seg<'_>>)> = vec![(v, 0, None)];
    let mut segs: Vec<Seg<'_>> = Vec::new();
    let mut max = 0usize;
    while let Some((cur, enclosing, seg)) = stack.pop() {
        if let Some(s) = seg {
            segs.truncate(enclosing.saturating_sub(1));
            segs.push(s);
        }
        let d = enclosing + 1;
        match cur {
            Value::Array(a) => {
                max = max.max(d);
                if d > cap {
                    return (d, Some(render_path(&segs)));
                }
                for (i, x) in a.iter().enumerate().rev() {
                    stack.push((x, d, Some(Seg::Index(i))));
                }
            }
            Value::Object(m) if !is_data_model_leaf(m) => {
                max = max.max(d);
                if d > cap {
                    return (d, Some(render_path(&segs)));
                }
                for (k, x) in m.iter().rev() {
                    stack.push((x, d, Some(Seg::Key(k.as_str()))));
                }
            }
            _ => {}
        }
    }
    (max, None)
}

// ---------------------------------------------------------------------------------------------
// Scalar checks (shared with the schema compiler for defaults and consts)

/// The first problem of a scalar `v` against a boolean, integer or string type, if any.
pub(crate) fn scalar_problem(t: &TypeDef, v: &Value) -> Option<String> {
    let mut out = Vec::new();
    match t {
        TypeDef::Boolean(b) => boolean_checks(b.const_value, v, &mut out),
        TypeDef::Integer(i) => integer_checks(i, v, &mut out),
        TypeDef::String(s) => string_checks(s, v, &mut out),
        _ => {}
    }
    out.into_iter().next().map(|(_, m)| m)
}

fn boolean_checks(const_value: Option<bool>, v: &Value, out: &mut Vec<(Code, String)>) {
    match v.as_bool() {
        None => out.push((Code::Type, format!("expected a boolean, found {}", kind(v)))),
        Some(b) => {
            if let Some(c) = const_value
                && b != c
            {
                out.push((Code::Const, format!("must be {c}")));
            }
        }
    }
}

/// A JSON number as a data-model integer.
pub(crate) fn as_integer(v: &Value) -> Result<i64, String> {
    let Value::Number(n) = v else {
        return Err(format!("expected an integer, found {}", kind(v)));
    };
    if let Some(i) = n.as_i64() {
        return Ok(i);
    }
    if n.as_u64().is_some() {
        return Err("integer out of the signed 64-bit range".into());
    }
    match n.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER => Ok(f as i64),
        _ => Err("expected an integer, found a float".into()),
    }
}

fn integer_checks(d: &IntegerDef, v: &Value, out: &mut Vec<(Code, String)>) {
    let i = match as_integer(v) {
        Ok(i) => i,
        Err(e) => return out.push((Code::Type, e)),
    };
    if let Some(c) = d.const_value
        && i != c
    {
        out.push((Code::Const, format!("must be {c}")));
    }
    if let Some(e) = &d.enum_values
        && !e.contains(&i)
    {
        out.push((Code::Enum, format!("{i} is not one of {e:?}")));
    }
    if let Some(min) = d.minimum
        && i < min
    {
        out.push((Code::Range, format!("{i} is below the minimum {min}")));
    }
    if let Some(max) = d.maximum
        && i > max
    {
        out.push((Code::Range, format!("{i} is above the maximum {max}")));
    }
}

fn string_checks(d: &StringDef, v: &Value, out: &mut Vec<(Code, String)>) {
    let Some(s) = v.as_str() else {
        return out.push((Code::Type, format!("expected a string, found {}", kind(v))));
    };
    if let Some(c) = &d.const_value
        && s != c
    {
        out.push((Code::Const, format!("must be {c:?}")));
    }
    if let Some(e) = &d.enum_values
        && !e.iter().any(|x| x == s)
    {
        out.push((Code::Enum, format!("{s:?} is not one of {e:?}")));
    }
    let bytes = s.len() as u64;
    if let Some(max) = d.max_length
        && bytes > max
    {
        out.push((
            Code::Length,
            format!("{bytes} UTF-8 bytes, more than maxLength {max}"),
        ));
    }
    if let Some(min) = d.min_length
        && bytes < min
    {
        out.push((
            Code::Length,
            format!("{bytes} UTF-8 bytes, fewer than minLength {min}"),
        ));
    }
    // Every grapheme is at least one byte, so counting is skipped when the byte length decides.
    let need_max = d.max_graphemes.is_some_and(|max| bytes > max);
    let need_min = d.min_graphemes.is_some();
    if need_max || need_min {
        let g = s.graphemes(true).count() as u64;
        if let Some(max) = d.max_graphemes
            && g > max
        {
            out.push((
                Code::Graphemes,
                format!("{g} graphemes, more than maxGraphemes {max}"),
            ));
        }
        if let Some(min) = d.min_graphemes
            && g < min
        {
            out.push((
                Code::Graphemes,
                format!("{g} graphemes, fewer than minGraphemes {min}"),
            ));
        }
    }
    if let Some(f) = d.format
        && let Err(why) = formats::check_format(f, s)
    {
        out.push((Code::Format, format!("{why} ({} format)", f.as_str())));
    }
}

/// The JSON kind of a value, for messages.
pub(crate) fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(n) if n.is_f64() => "a float",
        Value::Number(_) => "an integer",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The essence (`type/subtype`, lowercase, without parameters) of a MIME type.
pub fn mime_essence(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// `true` if `mime` matches an `accept` pattern: `*/*` matches everything, `type/*` every
/// subtype of `type`, anything else the exact essence (case-insensitive, parameters ignored).
pub fn accept_matches(pattern: &str, mime: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let m = mime_essence(mime);
    let Some((mt, ms)) = m.split_once('/') else {
        return false;
    };
    if mt.is_empty() || ms.is_empty() {
        return false;
    }
    if p == "*/*" {
        return true;
    }
    match p.strip_suffix("/*") {
        Some(t) => t == mt,
        None => p == m,
    }
}

// ---------------------------------------------------------------------------------------------
// Walker

#[derive(Debug, Clone, Copy)]
enum Seg<'a> {
    Key(&'a str),
    Index(usize),
}

fn render_path(segs: &[Seg<'_>]) -> String {
    let mut s = String::from("$");
    for seg in segs {
        match seg {
            Seg::Index(i) => {
                s.push('[');
                s.push_str(&i.to_string());
                s.push(']');
            }
            Seg::Key(k) => {
                let ident = !k.is_empty()
                    && k.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
                    && !k.as_bytes()[0].is_ascii_digit();
                if ident {
                    s.push('.');
                    s.push_str(k);
                } else {
                    s.push('[');
                    s.push_str(&Value::from(*k).to_string());
                    s.push(']');
                }
            }
        }
    }
    s
}

struct Walker<'a> {
    reg: &'a Registry,
    opts: &'a Options,
    path: Vec<Seg<'a>>,
    out: Vec<Violation>,
    truncated: bool,
    steps: usize,
    stopped: bool,
}

impl<'a> Walker<'a> {
    fn new(reg: &'a Registry, opts: &'a Options, root: &Value) -> Self {
        let mut w = Self {
            reg,
            opts,
            path: Vec::new(),
            out: Vec::new(),
            truncated: false,
            steps: 0,
            stopped: false,
        };
        let (depth, at) = nesting_depth(root, opts.max_depth);
        if depth > opts.max_depth {
            w.out.push(Violation {
                path: at.unwrap_or_else(|| "$".into()),
                code: Code::Depth,
                message: format!("nesting deeper than {} containers", opts.max_depth),
            });
        }
        w
    }

    fn finish(self) -> Report {
        Report {
            violations: self.out,
            truncated: self.truncated,
        }
    }

    fn push(&mut self, code: Code, message: String) {
        if self.out.len() >= self.opts.max_violations {
            self.truncated = true;
            return;
        }
        self.out.push(Violation {
            path: render_path(&self.path),
            code,
            message,
        });
    }

    fn push_at(&mut self, key: &'a str, code: Code, message: String) {
        self.path.push(Seg::Key(key));
        self.push(code, message);
        self.path.pop();
    }

    fn push_all(&mut self, items: Vec<(Code, String)>) {
        for (c, m) in items {
            self.push(c, m);
        }
    }

    /// Counts one visited value; `false` once the budget is exhausted.
    fn step(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        self.steps += 1;
        if self.steps > self.opts.max_steps {
            self.stopped = true;
            self.truncated = true;
            self.push(Code::Budget, "validation work budget exceeded".into());
            return false;
        }
        true
    }

    /// `depth` is the number of containers enclosing `v`.
    fn check(&mut self, t: &'a TypeDef, v: &'a Value, depth: usize, hops: usize) {
        if !self.step() {
            return;
        }
        let mut out = Vec::new();
        match t {
            TypeDef::Null => {
                if !v.is_null() {
                    out.push((Code::Type, format!("expected null, found {}", kind(v))));
                }
            }
            TypeDef::Boolean(b) => boolean_checks(b.const_value, v, &mut out),
            TypeDef::Integer(i) => integer_checks(i, v, &mut out),
            TypeDef::String(s) => string_checks(s, v, &mut out),
            TypeDef::Bytes(b) => self.bytes(b, v),
            TypeDef::CidLink => {
                if let Err(e) = cid_link(v) {
                    self.push(Code::Link, e);
                }
            }
            TypeDef::Blob(b) => self.blob(b, v),
            TypeDef::Array(a) => self.array(a, v, depth),
            TypeDef::Object(o) => self.object(o, v, depth),
            TypeDef::Ref(r) => self.reference(r, v, depth, hops),
            TypeDef::Union(u) => self.union(u, v, depth),
            TypeDef::Unknown => {
                if !v.is_object() {
                    out.push((Code::Type, format!("expected an object, found {}", kind(v))));
                }
            }
        }
        self.push_all(out);
    }

    fn reference(&mut self, r: &'a str, v: &'a Value, depth: usize, hops: usize) {
        if hops >= MAX_REF_HOPS {
            self.push(Code::Schema, format!("too many reference hops at {r:?}"));
            return;
        }
        match self.reg.def(r) {
            Some(Def::Type(t)) => self.check(t, v, depth, hops + 1),
            Some(Def::Record(rd)) => self.object(&rd.record, v, depth),
            Some(Def::Token) => {
                if v.as_str() != Some(r) && v.as_str() != r.strip_suffix("#main") {
                    self.push(Code::Type, format!("expected the token {r:?}"));
                }
            }
            Some(_) => self.push(Code::Schema, format!("{r:?} is not a data definition")),
            None => self.push(Code::Schema, format!("unresolved reference {r:?}")),
        }
    }

    fn object(&mut self, o: &'a ObjectDef, v: &'a Value, depth: usize) {
        let Some(m) = v.as_object() else {
            return self.push(Code::Type, format!("expected an object, found {}", kind(v)));
        };
        let d = depth + 1;
        if d > self.opts.max_depth {
            return; // reported once by the depth pre-check
        }
        for req in &o.required {
            if !m.contains_key(req) {
                self.push_at(req, Code::Required, "required field is missing".into());
            }
        }
        for (name, t) in &o.properties {
            let Some(fv) = m.get(name) else { continue };
            if fv.is_null() && o.nullable.iter().any(|n| n == name) {
                continue;
            }
            self.path.push(Seg::Key(name));
            self.check(t, fv, d, 0);
            self.path.pop();
            if self.stopped {
                return;
            }
        }
    }

    fn array(&mut self, a: &'a ArrayDef, v: &'a Value, depth: usize) {
        let Some(items) = v.as_array() else {
            return self.push(Code::Type, format!("expected an array, found {}", kind(v)));
        };
        let d = depth + 1;
        if d > self.opts.max_depth {
            return;
        }
        let n = items.len() as u64;
        if let Some(max) = a.max_length
            && n > max
        {
            self.push(
                Code::Length,
                format!("{n} items, more than maxLength {max}"),
            );
        }
        if let Some(min) = a.min_length
            && n < min
        {
            self.push(
                Code::Length,
                format!("{n} items, fewer than minLength {min}"),
            );
        }
        for (i, item) in items.iter().enumerate() {
            self.path.push(Seg::Index(i));
            self.check(&a.items, item, d, 0);
            self.path.pop();
            if self.stopped {
                return;
            }
        }
    }

    fn union(&mut self, u: &'a UnionDef, v: &'a Value, depth: usize) {
        let Some(m) = v.as_object() else {
            return self.push(
                Code::Type,
                format!("expected a union member object, found {}", kind(v)),
            );
        };
        let t = match m.get("$type") {
            Some(Value::String(t)) if !t.is_empty() => t,
            Some(_) => {
                return self.push_at(
                    "$type",
                    Code::Union,
                    "a union member's $type must be a non-empty string".into(),
                );
            }
            None => {
                return self.push_at("$type", Code::Union, "a union member needs $type".into());
            }
        };
        match u.refs.iter().find(|r| type_matches(t, r)) {
            Some(r) => self.reference(r, v, depth, 0),
            None if u.closed => self.push_at(
                "$type",
                Code::Union,
                format!("{t:?} is not a member of this closed union"),
            ),
            None => {} // open union: unknown members are left alone
        }
    }

    fn bytes(&mut self, b: &BytesDef, v: &Value) {
        let decoded = match v.as_object() {
            Some(m) if m.len() == 1 => match m.get("$bytes") {
                Some(Value::String(s)) => lessdugin_atp::datamodel::decode_base64(s)
                    .map_err(|_| "$bytes is not valid base64".to_owned()),
                Some(_) => Err("$bytes must be a string".to_owned()),
                None => Err("expected a {\"$bytes\": …} object".to_owned()),
            },
            Some(_) => Err("a $bytes object must have exactly one key".to_owned()),
            None => Err(format!("expected a $bytes object, found {}", kind(v))),
        };
        match decoded {
            Err(e) => self.push(Code::Bytes, e),
            Ok(bytes) => {
                let n = bytes.len() as u64;
                if let Some(max) = b.max_length
                    && n > max
                {
                    self.push(
                        Code::Length,
                        format!("{n} bytes, more than maxLength {max}"),
                    );
                }
                if let Some(min) = b.min_length
                    && n < min
                {
                    self.push(
                        Code::Length,
                        format!("{n} bytes, fewer than minLength {min}"),
                    );
                }
            }
        }
    }

    fn blob(&mut self, b: &BlobDef, v: &Value) {
        let Some(m) = v.as_object() else {
            return self.push(
                Code::Type,
                format!("expected a blob object, found {}", kind(v)),
            );
        };
        match m.get("$type") {
            Some(Value::String(t)) if t == "blob" => {
                let mut problems = Vec::new();
                match m.get("ref") {
                    Some(r) => {
                        if let Err(e) = cid_link(r) {
                            problems.push(format!("ref: {e}"));
                        }
                    }
                    None => problems.push("missing ref".to_owned()),
                }
                let mime = match m.get("mimeType") {
                    Some(Value::String(s)) if !s.is_empty() => Some(s.as_str()),
                    _ => {
                        problems.push("mimeType must be a non-empty string".to_owned());
                        None
                    }
                };
                let size = match m.get("size").map(as_integer) {
                    Some(Ok(n)) if n >= 0 => Some(n),
                    _ => {
                        problems.push("size must be a non-negative integer".to_owned());
                        None
                    }
                };
                for p in problems {
                    self.push(Code::Blob, p);
                }
                if let Some(mime) = mime {
                    self.accept(b, mime);
                }
                if let (Some(size), Some(max)) = (size, b.max_size)
                    && size as u64 > max
                {
                    self.push(
                        Code::BlobSize,
                        format!("blob size {size} is above maxSize {max}"),
                    );
                }
            }
            Some(_) => self.push(Code::Blob, "a blob's $type must be \"blob\"".into()),
            None => {
                let legacy = match (m.get("cid"), m.get("mimeType")) {
                    (Some(Value::String(c)), Some(Value::String(mt))) => Some((c, mt)),
                    _ => None,
                };
                match legacy {
                    None => self.push(Code::Blob, "not a blob object".into()),
                    Some(_) if !self.opts.allow_legacy_blobs => self.push(
                        Code::LegacyBlob,
                        "legacy {cid, mimeType} blob; the typed form is required".into(),
                    ),
                    Some((c, mt)) => {
                        if !lessdugin_atp::cid::is_valid_cid_syntax(c) {
                            self.push(Code::Blob, "legacy blob cid is not a CID".into());
                        }
                        self.accept(b, mt);
                    }
                }
            }
        }
    }

    fn accept(&mut self, b: &BlobDef, mime: &str) {
        if let Some(acc) = &b.accept
            && !acc.iter().any(|p| accept_matches(p, mime))
        {
            self.push(
                Code::BlobAccept,
                format!("MIME type {mime:?} is not accepted ({})", acc.join(", ")),
            );
        }
    }
}

/// Checks a `{"$link": cid}` object.
fn cid_link(v: &Value) -> Result<(), String> {
    let Some(m) = v.as_object() else {
        return Err(format!("expected a $link object, found {}", kind(v)));
    };
    if m.len() != 1 {
        return Err("a $link object must have exactly one key".into());
    }
    match m.get("$link") {
        Some(Value::String(s)) => lessdugin_atp::cid::parse(s)
            .map(|_| ())
            .map_err(|e| format!("$link: {e}")),
        Some(_) => Err("$link must be a string".into()),
        None => Err("expected a {\"$link\": …} object".into()),
    }
}

#[cfg(test)]
mod tests;
