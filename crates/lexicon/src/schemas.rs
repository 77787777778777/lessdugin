// SPDX-License-Identifier: MIT OR Apache-2.0
//! Embedded lexicon JSON documents and the compiled schema [`Registry`].
//!
//! Every lexicon of `lexicons/online/7778777/lessdugin/` is compiled into the binary
//! ([`LESSDUGIN`]; `data-model.md` §1.4: lexicons are never fetched at runtime), together with the
//! two atproto lexicons they reference ([`EXTERNAL`]: `com.atproto.repo.strongRef` and
//! `com.atproto.label.defs`). A test compares [`LESSDUGIN`] with the directory, so the list stays
//! complete.
//!
//! **Namespace mapping (§1.3).** Lexicon files use the final prefix
//! (`online.7778777.lessdugin`). [`Registry::embedded`] rewrites every lessdugin NSID with
//! [`crate::ns::to_build_nsid`] (document ids, `ref`s and union `refs`), so the registry is keyed
//! by **build** NSIDs and a `.temp` build validates `.temp` records (`$type` values included).
//!
//! **Compilation.** Each document is compiled into [`LexiconDoc`]: references are made absolute
//! (`nsid#name`), every attribute is type-checked, unknown attributes and formats are schema
//! errors, and every `default` / `const` must satisfy its own type's constraints. Compiling never
//! panics; errors are collected in [`Registry::errors`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use lessdugin_atp::syntax::{Nsid, RecordKeyType};
use serde_json::{Map, Value};

use crate::ns::to_build_nsid;

/// One embedded lexicon file.
#[derive(Debug, Clone, Copy)]
pub struct Embedded {
    /// Path relative to its lexicon root (for lessdugin files, the repository's `lexicons/`).
    pub path: &'static str,
    /// The file contents.
    pub json: &'static str,
}

macro_rules! lessdugin {
    ($($p:literal),* $(,)?) => {
        &[$(Embedded { path: $p, json: include_str!(concat!("../../../lexicons/", $p)) }),*]
    };
}

macro_rules! external {
    ($($p:literal),* $(,)?) => {
        &[$(Embedded { path: $p, json: include_str!(concat!("../lexicons/", $p)) }),*]
    };
}

/// Every lessdugin lexicon file (`lexicons/online/7778777/lessdugin/**.json`), with final NSIDs.
pub const LESSDUGIN: &[Embedded] = lessdugin![
    "online/7778777/lessdugin/authFull.json",
    "online/7778777/lessdugin/authInteract.json",
    "online/7778777/lessdugin/comment.json",
    "online/7778777/lessdugin/defs.json",
    "online/7778777/lessdugin/forum.json",
    "online/7778777/lessdugin/markdown.json",
    "online/7778777/lessdugin/membership.json",
    "online/7778777/lessdugin/modAction.json",
    "online/7778777/lessdugin/opinion.json",
    "online/7778777/lessdugin/post.json",
    "online/7778777/lessdugin/profile.json",
    "online/7778777/lessdugin/revision.json",
    "online/7778777/lessdugin/sequence.json",
    "online/7778777/lessdugin/wikiRevision.json",
    "online/7778777/lessdugin/index/defs.json",
    "online/7778777/lessdugin/index/describeIndexer.json",
    "online/7778777/lessdugin/index/getActivity.json",
    "online/7778777/lessdugin/index/getChanges.json",
    "online/7778777/lessdugin/index/getKarma.json",
    "online/7778777/lessdugin/index/getRecordProofs.json",
    "online/7778777/lessdugin/index/getRecords.json",
    "online/7778777/lessdugin/index/getRepoHeads.json",
    "online/7778777/lessdugin/index/getTallies.json",
    "online/7778777/lessdugin/index/getThread.json",
    "online/7778777/lessdugin/index/listBacklinks.json",
    "online/7778777/lessdugin/index/listForums.json",
    "online/7778777/lessdugin/index/listParticipants.json",
    "online/7778777/lessdugin/index/listPosts.json",
    "online/7778777/lessdugin/index/listRecentDiscussion.json",
    "online/7778777/lessdugin/index/searchPosts.json",
];

/// The atproto lexicons the lessdugin lexicons reference (vendored in `crates/lexicon/lexicons/`).
pub const EXTERNAL: &[Embedded] = external![
    "com/atproto/repo/strongRef.json",
    "com/atproto/label/defs.json",
];

// ---------------------------------------------------------------------------------------------
// Compiled schema types

/// A compiled lexicon document.
#[derive(Debug, Clone, PartialEq)]
pub struct LexiconDoc {
    /// The NSID this build uses (prefix-mapped).
    pub id: String,
    /// The NSID as written in the file.
    pub file_id: String,
    /// The document's definitions by name (`main`, `media`, …).
    pub defs: BTreeMap<String, Def>,
}

/// A top-level definition.
#[derive(Debug, Clone, PartialEq)]
pub enum Def {
    /// A `record` definition (always `main`).
    Record(RecordDef),
    /// A `query` (XRPC GET).
    Query(XrpcDef),
    /// A `procedure` (XRPC POST).
    Procedure(XrpcDef),
    /// A `subscription` (event stream).
    Subscription(XrpcDef),
    /// A `permission-set`; kept as raw JSON (it describes OAuth scopes, not data).
    PermissionSet(Value),
    /// A `token`: a named constant with no data shape.
    Token,
    /// A data type definition (object, string, …).
    Type(TypeDef),
}

/// A `record` definition.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordDef {
    /// The record key type (`any`, `tid`, `nsid`, `literal:<v>`).
    pub key: RecordKeyType,
    /// The record object schema.
    pub record: ObjectDef,
}

/// An XRPC method definition.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct XrpcDef {
    /// `parameters` (a `params` object).
    pub parameters: Option<ObjectDef>,
    /// `input` body (procedures).
    pub input: Option<Body>,
    /// `output` body.
    pub output: Option<Body>,
    /// `message` schema (subscriptions).
    pub message: Option<TypeDef>,
    /// Declared error names.
    pub errors: Vec<String>,
}

/// An XRPC input or output body.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The MIME encoding (`application/json`, `application/vnd.ipld.car`, …).
    pub encoding: String,
    /// The JSON schema of the body, if any.
    pub schema: Option<TypeDef>,
}

/// A data type.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    /// `null`
    Null,
    /// `boolean`
    Boolean(BooleanDef),
    /// `integer`
    Integer(IntegerDef),
    /// `string`
    String(StringDef),
    /// `bytes` (`{"$bytes": base64}` in JSON)
    Bytes(BytesDef),
    /// `cid-link` (`{"$link": cid}` in JSON)
    CidLink,
    /// `blob`
    Blob(BlobDef),
    /// `array`
    Array(ArrayDef),
    /// `object` (or `params`)
    Object(ObjectDef),
    /// `ref`, made absolute: `nsid#name`.
    Ref(String),
    /// `union`; refs made absolute.
    Union(UnionDef),
    /// `unknown`: any object.
    Unknown,
}

/// `boolean` constraints.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BooleanDef {
    /// `default`
    pub default: Option<bool>,
    /// `const`
    pub const_value: Option<bool>,
}

/// `integer` constraints.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IntegerDef {
    /// `minimum` (inclusive)
    pub minimum: Option<i64>,
    /// `maximum` (inclusive)
    pub maximum: Option<i64>,
    /// `enum`: the closed set of allowed values.
    pub enum_values: Option<Vec<i64>>,
    /// `default`
    pub default: Option<i64>,
    /// `const`
    pub const_value: Option<i64>,
}

/// The Lexicon string formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringFormat {
    /// `at-identifier`: a DID or a handle.
    AtIdentifier,
    /// `at-uri` (Lexicon syntax; lessdugin's canonical form is a separate rule, G4).
    AtUri,
    /// `cid` (string CID, loose syntax).
    Cid,
    /// `datetime`
    Datetime,
    /// `did`
    Did,
    /// `handle`
    Handle,
    /// `nsid`
    Nsid,
    /// `tid`
    Tid,
    /// `record-key`
    RecordKey,
    /// `uri`
    Uri,
    /// `language` (BCP 47)
    Language,
}

impl StringFormat {
    /// Parses a format name.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "at-identifier" => Self::AtIdentifier,
            "at-uri" => Self::AtUri,
            "cid" => Self::Cid,
            "datetime" => Self::Datetime,
            "did" => Self::Did,
            "handle" => Self::Handle,
            "nsid" => Self::Nsid,
            "tid" => Self::Tid,
            "record-key" => Self::RecordKey,
            "uri" => Self::Uri,
            "language" => Self::Language,
            _ => return None,
        })
    }

    /// The format name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AtIdentifier => "at-identifier",
            Self::AtUri => "at-uri",
            Self::Cid => "cid",
            Self::Datetime => "datetime",
            Self::Did => "did",
            Self::Handle => "handle",
            Self::Nsid => "nsid",
            Self::Tid => "tid",
            Self::RecordKey => "record-key",
            Self::Uri => "uri",
            Self::Language => "language",
        }
    }
}

/// `string` constraints.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StringDef {
    /// `format`
    pub format: Option<StringFormat>,
    /// `minLength` in UTF-8 bytes
    pub min_length: Option<u64>,
    /// `maxLength` in UTF-8 bytes
    pub max_length: Option<u64>,
    /// `minGraphemes` (extended grapheme clusters)
    pub min_graphemes: Option<u64>,
    /// `maxGraphemes` (extended grapheme clusters)
    pub max_graphemes: Option<u64>,
    /// `knownValues` (open: never enforced)
    pub known_values: Vec<String>,
    /// `enum` (closed)
    pub enum_values: Option<Vec<String>>,
    /// `default`
    pub default: Option<String>,
    /// `const`
    pub const_value: Option<String>,
}

/// `bytes` constraints (decoded length).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BytesDef {
    /// `minLength` in bytes
    pub min_length: Option<u64>,
    /// `maxLength` in bytes
    pub max_length: Option<u64>,
}

/// `blob` constraints.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlobDef {
    /// `accept`: MIME types, optionally `type/*` or `*/*` globs. `None` accepts anything.
    pub accept: Option<Vec<String>>,
    /// `maxSize` in bytes.
    pub max_size: Option<u64>,
}

/// `array` constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayDef {
    /// Item type.
    pub items: Box<TypeDef>,
    /// `minLength` (items)
    pub min_length: Option<u64>,
    /// `maxLength` (items)
    pub max_length: Option<u64>,
}

/// `object` (and `params`) definition.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectDef {
    /// Properties in declaration order.
    pub properties: Vec<(String, TypeDef)>,
    /// `required` property names.
    pub required: Vec<String>,
    /// `nullable` property names.
    pub nullable: Vec<String>,
}

impl ObjectDef {
    /// The type of property `name`.
    pub fn property(&self, name: &str) -> Option<&TypeDef> {
        self.properties
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t)
    }
}

/// `union` definition.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnionDef {
    /// Absolute member references (`nsid#name`).
    pub refs: Vec<String>,
    /// `closed`: unknown `$type`s are rejected. Unions are open by default.
    pub closed: bool,
}

/// A schema compilation error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{doc}: {path}: {message}")]
pub struct SchemaError {
    /// The document id (or file path when the id is unknown).
    pub doc: String,
    /// Where in the document (`defs.main.record.properties.forum`).
    pub path: String,
    /// What is wrong.
    pub message: String,
}

// ---------------------------------------------------------------------------------------------
// Registry

/// The compiled lexicon documents, keyed by build NSID.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    docs: BTreeMap<String, LexiconDoc>,
    errors: Vec<SchemaError>,
}

/// The registry of every embedded lexicon (compiled once, on first use).
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::embedded)
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compiles [`LESSDUGIN`] and [`EXTERNAL`]. Never panics: documents that fail to compile are
    /// left out and their errors are kept in [`Registry::errors`] (a test asserts there are none).
    pub fn embedded() -> Self {
        let mut r = Self::new();
        for e in LESSDUGIN.iter().chain(EXTERNAL) {
            if let Err(err) = r.add_json(e.json) {
                r.errors.push(SchemaError {
                    doc: if err.doc.is_empty() {
                        e.path.to_owned()
                    } else {
                        err.doc.clone()
                    },
                    ..err
                });
            }
        }
        r
    }

    /// Compiles one lexicon document (final NSIDs are mapped to this build's prefix) and adds it.
    /// A document with the same id replaces the previous one.
    pub fn add_json(&mut self, json: &str) -> Result<(), SchemaError> {
        let v: Value = serde_json::from_str(json).map_err(|e| SchemaError {
            doc: String::new(),
            path: String::new(),
            message: format!("invalid JSON: {e}"),
        })?;
        let doc = compile_doc(&v)?;
        self.docs.insert(doc.id.clone(), doc);
        Ok(())
    }

    /// Compilation errors of [`Registry::embedded`].
    pub fn errors(&self) -> &[SchemaError] {
        &self.errors
    }

    /// The document with this build NSID.
    pub fn doc(&self, nsid: &str) -> Option<&LexiconDoc> {
        self.docs.get(nsid)
    }

    /// Every document, by build NSID.
    pub fn docs(&self) -> impl Iterator<Item = &LexiconDoc> {
        self.docs.values()
    }

    /// Resolves a reference: `nsid` (the `main` def) or `nsid#name`.
    pub fn def(&self, reference: &str) -> Option<&Def> {
        let (nsid, name) = reference.split_once('#').unwrap_or((reference, "main"));
        self.docs.get(nsid)?.defs.get(name)
    }

    /// The record definition of a collection NSID.
    pub fn record(&self, nsid: &str) -> Option<&RecordDef> {
        match self.def(nsid)? {
            Def::Record(r) => Some(r),
            _ => None,
        }
    }

    /// The build NSIDs of every record definition.
    pub fn record_nsids(&self) -> Vec<&str> {
        self.docs
            .values()
            .filter(|d| matches!(d.defs.get("main"), Some(Def::Record(_))))
            .map(|d| d.id.as_str())
            .collect()
    }

    /// Every reference (`ref` and union member) that does not resolve, as
    /// `(location, reference)`. Empty for the embedded set (tested).
    pub fn unresolved_refs(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for doc in self.docs.values() {
            for (name, def) in &doc.defs {
                let at = format!("{}#{name}", doc.id);
                let mut types: Vec<&TypeDef> = Vec::new();
                match def {
                    Def::Record(r) => types.extend(r.record.properties.iter().map(|(_, t)| t)),
                    Def::Query(x) | Def::Procedure(x) | Def::Subscription(x) => {
                        if let Some(p) = &x.parameters {
                            types.extend(p.properties.iter().map(|(_, t)| t));
                        }
                        for b in [&x.input, &x.output].into_iter().flatten() {
                            types.extend(b.schema.iter());
                        }
                        types.extend(x.message.iter());
                    }
                    Def::Type(t) => types.push(t),
                    Def::PermissionSet(_) | Def::Token => {}
                }
                while let Some(t) = types.pop() {
                    match t {
                        TypeDef::Ref(r) => {
                            if self.def(r).is_none() {
                                out.push((at.clone(), r.clone()));
                            }
                        }
                        TypeDef::Union(u) => {
                            for r in &u.refs {
                                if self.def(r).is_none() {
                                    out.push((at.clone(), r.clone()));
                                }
                            }
                        }
                        TypeDef::Array(a) => types.push(&a.items),
                        TypeDef::Object(o) => types.extend(o.properties.iter().map(|(_, t)| t)),
                        _ => {}
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Compiler

/// Maps a final-prefix NSID to this build's prefix; other NSIDs are unchanged.
fn map_nsid(nsid: &str) -> String {
    to_build_nsid(nsid).unwrap_or_else(|| nsid.to_owned())
}

struct Cx<'a> {
    doc: &'a str,
    path: Vec<String>,
}

impl Cx<'_> {
    fn err(&self, message: impl Into<String>) -> SchemaError {
        SchemaError {
            doc: self.doc.to_owned(),
            path: self.path.join("."),
            message: message.into(),
        }
    }

    fn with<T>(
        &mut self,
        seg: &str,
        f: impl FnOnce(&mut Self) -> Result<T, SchemaError>,
    ) -> Result<T, SchemaError> {
        self.path.push(seg.to_owned());
        let r = f(self);
        self.path.pop();
        r
    }

    /// Makes a `ref` absolute: `#x` → `<doc>#x`, `nsid` → `nsid#main`, with prefix mapping.
    fn absolute(&self, r: &str) -> Result<String, SchemaError> {
        if let Some(name) = r.strip_prefix('#') {
            if name.is_empty() {
                return Err(self.err("empty local reference"));
            }
            return Ok(format!("{}#{name}", self.doc));
        }
        let (nsid, name) = r.split_once('#').unwrap_or((r, "main"));
        if Nsid::parse(nsid).is_err() || name.is_empty() {
            return Err(self.err(format!("invalid reference {r:?}")));
        }
        Ok(format!("{}#{name}", map_nsid(nsid)))
    }
}

fn compile_doc(v: &Value) -> Result<LexiconDoc, SchemaError> {
    let mut cx = Cx {
        doc: "",
        path: Vec::new(),
    };
    let obj = v
        .as_object()
        .ok_or_else(|| cx.err("document must be an object"))?;
    check_keys(
        &cx,
        obj,
        &["lexicon", "id", "description", "defs", "revision", "$type"],
    )?;
    if obj.get("lexicon") != Some(&Value::from(1)) {
        return Err(cx.err("\"lexicon\" must be 1"));
    }
    let file_id = obj
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| cx.err("\"id\" must be a string"))?;
    if Nsid::parse(file_id).is_err() {
        return Err(cx.err(format!("\"id\" is not an NSID: {file_id:?}")));
    }
    let id = map_nsid(file_id);
    cx.doc = &id;
    opt_str(&cx, obj, "description")?;
    let defs_v = obj
        .get("defs")
        .and_then(Value::as_object)
        .ok_or_else(|| cx.err("\"defs\" must be an object"))?;
    let mut defs = BTreeMap::new();
    for (name, d) in defs_v {
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric())
            || !name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(cx.err(format!("invalid def name {name:?}")));
        }
        let def = cx.with("defs", |cx| cx.with(name, |cx| compile_def(cx, name, d)))?;
        defs.insert(name.clone(), def);
    }
    check_local_union_members(&cx, &defs)?;
    Ok(LexiconDoc {
        id: id.clone(),
        file_id: file_id.to_owned(),
        defs,
    })
}

/// `/specs/lexicon`: "The schema definitions pointed to by a `union` must have type `object` or
/// `record`." Checked here for members in the same document; members in other documents are
/// checked when validating (a [`crate::validate::Code::Schema`] violation).
fn check_local_union_members(cx: &Cx<'_>, defs: &BTreeMap<String, Def>) -> Result<(), SchemaError> {
    let mut types: Vec<(&str, &TypeDef)> = Vec::new();
    for (name, def) in defs {
        match def {
            Def::Record(r) => {
                types.extend(r.record.properties.iter().map(|(_, t)| (name.as_str(), t)))
            }
            Def::Query(x) | Def::Procedure(x) | Def::Subscription(x) => {
                if let Some(p) = &x.parameters {
                    types.extend(p.properties.iter().map(|(_, t)| (name.as_str(), t)));
                }
                for b in [&x.input, &x.output].into_iter().flatten() {
                    types.extend(b.schema.iter().map(|t| (name.as_str(), t)));
                }
                types.extend(x.message.iter().map(|t| (name.as_str(), t)));
            }
            Def::Type(t) => types.push((name.as_str(), t)),
            Def::PermissionSet(_) | Def::Token => {}
        }
    }
    let local = format!("{}#", cx.doc);
    while let Some((at, t)) = types.pop() {
        match t {
            TypeDef::Union(u) => {
                for r in &u.refs {
                    let Some(target) = r.strip_prefix(&local).and_then(|n| defs.get(n)) else {
                        continue;
                    };
                    if !matches!(target, Def::Record(_) | Def::Type(TypeDef::Object(_))) {
                        return Err(SchemaError {
                            doc: cx.doc.to_owned(),
                            path: format!("defs.{at}"),
                            message: format!(
                                "union member {r:?} is not an object or record definition"
                            ),
                        });
                    }
                }
            }
            TypeDef::Array(a) => types.push((at, &a.items)),
            TypeDef::Object(o) => types.extend(o.properties.iter().map(|(_, t)| (at, t))),
            _ => {}
        }
    }
    Ok(())
}

fn obj_of<'v>(cx: &Cx<'_>, v: &'v Value) -> Result<&'v Map<String, Value>, SchemaError> {
    v.as_object()
        .ok_or_else(|| cx.err("definition must be an object"))
}

fn type_of<'v>(cx: &Cx<'_>, o: &'v Map<String, Value>) -> Result<&'v str, SchemaError> {
    o.get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| cx.err("\"type\" must be a string"))
}

fn check_keys(cx: &Cx<'_>, o: &Map<String, Value>, allowed: &[&str]) -> Result<(), SchemaError> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(cx.err(format!("unknown attribute {k:?}")));
        }
    }
    Ok(())
}

fn opt_str<'v>(
    cx: &Cx<'_>,
    o: &'v Map<String, Value>,
    k: &str,
) -> Result<Option<&'v str>, SchemaError> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(cx.err(format!("{k:?} must be a string"))),
    }
}

fn opt_u64(cx: &Cx<'_>, o: &Map<String, Value>, k: &str) -> Result<Option<u64>, SchemaError> {
    match o.get(k) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| cx.err(format!("{k:?} must be a non-negative integer"))),
    }
}

fn opt_i64(cx: &Cx<'_>, o: &Map<String, Value>, k: &str) -> Result<Option<i64>, SchemaError> {
    match o.get(k) {
        None => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| cx.err(format!("{k:?} must be an integer"))),
    }
}

fn opt_bool(cx: &Cx<'_>, o: &Map<String, Value>, k: &str) -> Result<Option<bool>, SchemaError> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(cx.err(format!("{k:?} must be a boolean"))),
    }
}

fn opt_strings(
    cx: &Cx<'_>,
    o: &Map<String, Value>,
    k: &str,
) -> Result<Option<Vec<String>>, SchemaError> {
    match o.get(k) {
        None => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| cx.err(format!("{k:?} must be an array of strings")))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(cx.err(format!("{k:?} must be an array of strings"))),
    }
}

const TOP_LEVEL_ONLY: &[&str] = &[
    "record",
    "query",
    "procedure",
    "subscription",
    "permission-set",
];

fn compile_def(cx: &mut Cx<'_>, name: &str, v: &Value) -> Result<Def, SchemaError> {
    let o = obj_of(cx, v)?;
    let ty = type_of(cx, o)?;
    if TOP_LEVEL_ONLY.contains(&ty) && name != "main" {
        return Err(cx.err(format!("a {ty} definition must be named main")));
    }
    Ok(match ty {
        "record" => {
            check_keys(cx, o, &["type", "description", "key", "record"])?;
            opt_str(cx, o, "description")?;
            let key_s = opt_str(cx, o, "key")?.unwrap_or("any");
            let key = RecordKeyType::parse(key_s)
                .map_err(|_| cx.err(format!("invalid record key type {key_s:?}")))?;
            let rec = o
                .get("record")
                .ok_or_else(|| cx.err("record definition needs \"record\""))?;
            let record = cx.with("record", |cx| {
                let ro = obj_of(cx, rec)?;
                if type_of(cx, ro)? != "object" {
                    return Err(cx.err("\"record\" must be an object type"));
                }
                compile_object(cx, ro, false)
            })?;
            Def::Record(RecordDef { key, record })
        }
        "query" | "procedure" | "subscription" => {
            check_keys(
                cx,
                o,
                &[
                    "type",
                    "description",
                    "parameters",
                    "input",
                    "output",
                    "message",
                    "errors",
                ],
            )?;
            opt_str(cx, o, "description")?;
            let x = compile_xrpc(cx, ty, o)?;
            match ty {
                "query" => Def::Query(x),
                "procedure" => Def::Procedure(x),
                _ => Def::Subscription(x),
            }
        }
        "permission-set" => Def::PermissionSet(v.clone()),
        "token" => {
            check_keys(cx, o, &["type", "description"])?;
            opt_str(cx, o, "description")?;
            Def::Token
        }
        // `/specs/lexicon`: "Unions can not be declared as top-level named types"
        "union" => return Err(cx.err("a union cannot be a named definition")),
        _ => Def::Type(compile_type(cx, v)?),
    })
}

fn compile_xrpc(cx: &mut Cx<'_>, ty: &str, o: &Map<String, Value>) -> Result<XrpcDef, SchemaError> {
    let mut x = XrpcDef::default();
    if let Some(p) = o.get("parameters") {
        x.parameters = Some(cx.with("parameters", |cx| {
            let po = obj_of(cx, p)?;
            if type_of(cx, po)? != "params" {
                return Err(cx.err("\"parameters\" must have type params"));
            }
            compile_object(cx, po, true)
        })?);
    }
    for (k, slot) in [("input", &mut x.input), ("output", &mut x.output)] {
        if let Some(b) = o.get(k) {
            if ty == "query" && k == "input" || ty == "subscription" {
                return Err(cx.err(format!("a {ty} has no {k}")));
            }
            *slot = Some(cx.with(k, |cx| compile_body(cx, b))?);
        }
    }
    if let Some(m) = o.get("message") {
        if ty != "subscription" {
            return Err(cx.err("only a subscription has a message"));
        }
        x.message = Some(cx.with("message", |cx| {
            let mo = obj_of(cx, m)?;
            check_keys(cx, mo, &["description", "schema"])?;
            let s = mo
                .get("schema")
                .ok_or_else(|| cx.err("message needs a schema"))?;
            cx.with("schema", |cx| compile_type(cx, s))
        })?);
    }
    if let Some(e) = o.get("errors") {
        let arr = e
            .as_array()
            .ok_or_else(|| cx.err("\"errors\" must be an array"))?;
        for item in arr {
            let eo = obj_of(cx, item)?;
            check_keys(cx, eo, &["name", "description"])?;
            let name = opt_str(cx, eo, "name")?.ok_or_else(|| cx.err("error needs a name"))?;
            opt_str(cx, eo, "description")?;
            x.errors.push(name.to_owned());
        }
    }
    Ok(x)
}

fn compile_body(cx: &mut Cx<'_>, v: &Value) -> Result<Body, SchemaError> {
    let o = obj_of(cx, v)?;
    check_keys(cx, o, &["description", "encoding", "schema"])?;
    opt_str(cx, o, "description")?;
    let encoding = opt_str(cx, o, "encoding")?
        .ok_or_else(|| cx.err("body needs an encoding"))?
        .to_owned();
    let schema = match o.get("schema") {
        Some(s) => Some(cx.with("schema", |cx| {
            let t = compile_type(cx, s)?;
            match t {
                TypeDef::Object(_) | TypeDef::Ref(_) | TypeDef::Union(_) => Ok(t),
                _ => Err(cx.err("a body schema must be an object, ref or union")),
            }
        })?),
        None => None,
    };
    Ok(Body { encoding, schema })
}

fn compile_object(
    cx: &mut Cx<'_>,
    o: &Map<String, Value>,
    params: bool,
) -> Result<ObjectDef, SchemaError> {
    check_keys(
        cx,
        o,
        &["type", "description", "required", "nullable", "properties"],
    )?;
    opt_str(cx, o, "description")?;
    let required = opt_strings(cx, o, "required")?.unwrap_or_default();
    let nullable = opt_strings(cx, o, "nullable")?.unwrap_or_default();
    if params && !nullable.is_empty() {
        return Err(cx.err("params have no nullable fields"));
    }
    let mut properties = Vec::new();
    if let Some(p) = o.get("properties") {
        let po = p
            .as_object()
            .ok_or_else(|| cx.err("\"properties\" must be an object"))?;
        for (name, pv) in po {
            if name.is_empty() {
                return Err(cx.err("empty property name"));
            }
            let t = cx.with("properties", |cx| cx.with(name, |cx| compile_type(cx, pv)))?;
            if params {
                let ok = match &t {
                    TypeDef::Boolean(_)
                    | TypeDef::Integer(_)
                    | TypeDef::String(_)
                    | TypeDef::Unknown => true,
                    TypeDef::Array(a) => matches!(
                        *a.items,
                        TypeDef::Boolean(_) | TypeDef::Integer(_) | TypeDef::String(_)
                    ),
                    _ => false,
                };
                if !ok {
                    return Err(cx.err(format!("params property {name:?} has a non-scalar type")));
                }
            }
            properties.push((name.clone(), t));
        }
    }
    for r in required.iter().chain(&nullable) {
        if !properties.iter().any(|(n, _)| n == r) {
            return Err(cx.err(format!("required/nullable field {r:?} is not a property")));
        }
    }
    Ok(ObjectDef {
        properties,
        required,
        nullable,
    })
}

/// Compiles a field type (any data type, not the top-level-only ones).
fn compile_type(cx: &mut Cx<'_>, v: &Value) -> Result<TypeDef, SchemaError> {
    let o = obj_of(cx, v)?;
    let ty = type_of(cx, o)?;
    let t = match ty {
        "null" => {
            check_keys(cx, o, &["type", "description"])?;
            TypeDef::Null
        }
        "boolean" => {
            check_keys(cx, o, &["type", "description", "default", "const"])?;
            TypeDef::Boolean(BooleanDef {
                default: opt_bool(cx, o, "default")?,
                const_value: opt_bool(cx, o, "const")?,
            })
        }
        "integer" => {
            check_keys(
                cx,
                o,
                &[
                    "type",
                    "description",
                    "minimum",
                    "maximum",
                    "enum",
                    "default",
                    "const",
                ],
            )?;
            let enum_values = match o.get("enum") {
                None => None,
                Some(Value::Array(a)) => Some(
                    a.iter()
                        .map(|v| v.as_i64().ok_or_else(|| cx.err("enum must hold integers")))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Some(_) => return Err(cx.err("enum must be an array")),
            };
            let d = IntegerDef {
                minimum: opt_i64(cx, o, "minimum")?,
                maximum: opt_i64(cx, o, "maximum")?,
                enum_values,
                default: opt_i64(cx, o, "default")?,
                const_value: opt_i64(cx, o, "const")?,
            };
            if let (Some(a), Some(b)) = (d.minimum, d.maximum)
                && a > b
            {
                return Err(cx.err("minimum > maximum"));
            }
            TypeDef::Integer(d)
        }
        "string" => {
            check_keys(
                cx,
                o,
                &[
                    "type",
                    "description",
                    "format",
                    "minLength",
                    "maxLength",
                    "minGraphemes",
                    "maxGraphemes",
                    "knownValues",
                    "enum",
                    "default",
                    "const",
                ],
            )?;
            let format = match opt_str(cx, o, "format")? {
                None => None,
                Some(f) => Some(
                    StringFormat::parse(f)
                        .ok_or_else(|| cx.err(format!("unknown string format {f:?}")))?,
                ),
            };
            let d = StringDef {
                format,
                min_length: opt_u64(cx, o, "minLength")?,
                max_length: opt_u64(cx, o, "maxLength")?,
                min_graphemes: opt_u64(cx, o, "minGraphemes")?,
                max_graphemes: opt_u64(cx, o, "maxGraphemes")?,
                known_values: opt_strings(cx, o, "knownValues")?.unwrap_or_default(),
                enum_values: opt_strings(cx, o, "enum")?,
                default: opt_str(cx, o, "default")?.map(str::to_owned),
                const_value: opt_str(cx, o, "const")?.map(str::to_owned),
            };
            for (a, b) in [
                (d.min_length, d.max_length),
                (d.min_graphemes, d.max_graphemes),
            ] {
                if let (Some(a), Some(b)) = (a, b)
                    && a > b
                {
                    return Err(cx.err("minimum length > maximum length"));
                }
            }
            TypeDef::String(d)
        }
        "bytes" => {
            check_keys(cx, o, &["type", "description", "minLength", "maxLength"])?;
            TypeDef::Bytes(BytesDef {
                min_length: opt_u64(cx, o, "minLength")?,
                max_length: opt_u64(cx, o, "maxLength")?,
            })
        }
        "cid-link" => {
            check_keys(cx, o, &["type", "description"])?;
            TypeDef::CidLink
        }
        "blob" => {
            check_keys(cx, o, &["type", "description", "accept", "maxSize"])?;
            let accept = opt_strings(cx, o, "accept")?;
            for a in accept.iter().flatten() {
                if !valid_accept_pattern(a) {
                    return Err(cx.err(format!("invalid accept pattern {a:?}")));
                }
            }
            TypeDef::Blob(BlobDef {
                accept,
                max_size: opt_u64(cx, o, "maxSize")?,
            })
        }
        "array" => {
            check_keys(
                cx,
                o,
                &["type", "description", "items", "minLength", "maxLength"],
            )?;
            let items = o.get("items").ok_or_else(|| cx.err("array needs items"))?;
            let items = cx.with("items", |cx| compile_type(cx, items))?;
            TypeDef::Array(ArrayDef {
                items: Box::new(items),
                min_length: opt_u64(cx, o, "minLength")?,
                max_length: opt_u64(cx, o, "maxLength")?,
            })
        }
        "object" => TypeDef::Object(compile_object(cx, o, false)?),
        "ref" => {
            check_keys(cx, o, &["type", "description", "ref"])?;
            let r = opt_str(cx, o, "ref")?.ok_or_else(|| cx.err("ref needs \"ref\""))?;
            TypeDef::Ref(cx.absolute(r)?)
        }
        "union" => {
            check_keys(cx, o, &["type", "description", "refs", "closed"])?;
            let refs = opt_strings(cx, o, "refs")?.ok_or_else(|| cx.err("union needs refs"))?;
            let refs = refs
                .iter()
                .map(|r| cx.absolute(r))
                .collect::<Result<Vec<_>, _>>()?;
            TypeDef::Union(UnionDef {
                refs,
                closed: opt_bool(cx, o, "closed")?.unwrap_or(false),
            })
        }
        "unknown" => {
            check_keys(cx, o, &["type", "description"])?;
            TypeDef::Unknown
        }
        other if TOP_LEVEL_ONLY.contains(&other) || other == "token" || other == "params" => {
            return Err(cx.err(format!("type {other:?} is not allowed here")));
        }
        other => return Err(cx.err(format!("unknown type {other:?}"))),
    };
    opt_str(cx, o, "description")?;
    check_self_consistent(cx, &t)?;
    Ok(t)
}

/// `type/subtype`, `type/*` or `*/*`.
fn valid_accept_pattern(a: &str) -> bool {
    let Some((t, s)) = a.split_once('/') else {
        return false;
    };
    let token = |x: &str| {
        !x.is_empty()
            && x.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    (t == "*" && s == "*") || (token(t) && (s == "*" || token(s)))
}

/// Every `default` and `const` must satisfy the type's own constraints.
fn check_self_consistent(cx: &Cx<'_>, t: &TypeDef) -> Result<(), SchemaError> {
    let values: Vec<Value> = match t {
        TypeDef::Boolean(b) => [b.default, b.const_value]
            .into_iter()
            .flatten()
            .map(Value::from)
            .collect(),
        TypeDef::Integer(i) => [i.default, i.const_value]
            .into_iter()
            .flatten()
            .map(Value::from)
            .collect(),
        TypeDef::String(s) => [&s.default, &s.const_value]
            .into_iter()
            .flatten()
            .map(|v| Value::from(v.as_str()))
            .collect(),
        _ => Vec::new(),
    };
    for v in values {
        if let Some(problem) = crate::validate::scalar_problem(t, &v) {
            return Err(cx.err(format!("default/const {v} violates its type: {problem}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::NS;

    #[test]
    fn embedded_compiles() {
        let r = registry();
        assert!(r.errors().is_empty(), "{:?}", r.errors());
        assert_eq!(r.docs().count(), LESSDUGIN.len() + EXTERNAL.len());
        assert!(r.unresolved_refs().is_empty(), "{:?}", r.unresolved_refs());
    }

    #[test]
    fn keyed_by_build_nsid() {
        let r = registry();
        assert!(r.doc(&format!("{NS}.post")).is_some());
        assert!(r.record(&format!("{NS}.post")).is_some());
        assert!(r.doc(&format!("{NS}.index.listPosts")).is_some());
        assert!(r.doc("com.atproto.repo.strongRef").is_some());
        if NS != crate::ns::NS_FINAL {
            assert!(r.doc("online.7778777.lessdugin.post").is_none());
        }
        let Some(TypeDef::Union(u)) = r
            .record(&format!("{NS}.post"))
            .and_then(|p| p.record.property("body"))
        else {
            panic!("post.body is a union");
        };
        assert_eq!(u.refs, vec![format!("{NS}.markdown#main")]);
        assert_eq!(r.record_nsids().len(), 10);
    }

    #[test]
    fn rejects_bad_schemas() {
        let mut r = Registry::new();
        for bad in [
            r#"{"lexicon":2,"id":"a.b.c","defs":{}}"#,
            r#"{"lexicon":1,"id":"nope","defs":{}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"strnig"}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"string","maxLenght":3}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"string","format":"email"}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"x":{"type":"record","record":{"type":"object"}}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"object","required":["x"]}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"integer","minimum":1,"default":0}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"string","maxLength":2,"default":"abc"}}}"#,
            r##"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"ref","ref":"#"}}}"##,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"blob","accept":["image"]}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"array"}}}"#,
            r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"object","properties":{"a":{"type":"token"}}}}}"#,
            // a union cannot be a named definition, and its members are objects or records
            r##"{"lexicon":1,"id":"a.b.c","defs":{"u":{"type":"union","refs":["#u"]}}}"##,
            r##"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"object","properties":{"u":{"type":"union","refs":["#s"]}}},"s":{"type":"string"}}}"##,
            r##"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"object","properties":{"u":{"type":"union","refs":["#r"]}}},"r":{"type":"ref","ref":"#main"}}}"##,
            r##"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"object","properties":{"u":{"type":"array","items":{"type":"union","refs":["#t"]}}}},"t":{"type":"token"}}}"##,
        ] {
            assert!(r.add_json(bad).is_err(), "{bad}");
        }
        assert!(
            r.add_json(r#"{"lexicon":1,"id":"a.b.c","defs":{"main":{"type":"token"}}}"#)
                .is_ok()
        );
    }

    #[test]
    fn accept_patterns() {
        for ok in [
            "*/*",
            "image/*",
            "image/png",
            "text/markdown",
            "application/vnd.ipld.car",
        ] {
            assert!(valid_accept_pattern(ok), "{ok}");
        }
        for bad in ["*", "image", "*/png", "/", "image/", "ima ge/png"] {
            assert!(!valid_accept_pattern(bad), "{bad}");
        }
    }
}
