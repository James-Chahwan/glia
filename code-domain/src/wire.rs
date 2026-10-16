//! The contract-model wire vocabulary (DT.1a): one JSON form for the models a
//! client and a server exchange, written by the TypeScript, Go, Dart and
//! GraphQL extractors and the Go build, and read by the models pass and
//! `contract_fields`. Defining it once lets every writer and reader agree.
//!
//! Three payloads, all written here and nowhere else:
//!
//! - **A model's declared fields** ([`ModelSchema`]): a SCHEMA_FIELDS cell
//!   ([`crate::cell_type::SCHEMA_FIELDS`]) on an INTERFACE / CLASS / STRUCT, or
//!   a GraphQL MESSAGE_TYPE node:
//!
//!   ```text
//!   {"extends":["Base"],"fields":[{"array":true,"bind":"required","field":"Items",
//!    "model":"models::Item","name":"items","required":true,"type":"[]Item"}],
//!    "format":"go","open":true}
//!   ```
//!
//!   `extends`, `open` and each field's `array`, `bind`, `field` and `model`
//!   appear only when set; `field` (the source member) only when it differs
//!   from `name` (the JSON name). `model` is written by a builder that typed
//!   the field to an in-repo model. Every field keeps `name` / `type` /
//!   `required`, so `contract_fields`' field reader reads it unchanged.
//! - **A wire shape** ([`Shape`]): the value a call site sends or receives, as
//!   written (a type, a call chain, a field chain, an inline object, a union)
//!   or open. On an ENDPOINT_HIT it is the `response` / `request` key
//!   ([`append_hit_fields`] / [`hit_shapes`]); on a server site it is
//!   [`ServerWire::shape`].
//! - **A server site** ([`ServerWire`]): a MESSAGE_TYPE cell
//!   ([`crate::cell_type::MESSAGE_TYPE`]) on a handler or a GraphQL resolver /
//!   operation. It keeps A12.1's leading keys (`type`, `raw`, `form`,
//!   `window`; the queue extractor's MESSAGE_TYPE cell), with `window`
//!   `site`, which no queue cell writes, so [`ServerWire::parse`] never
//!   mistakes one for the other.
//!
//! **Sorted keys.** [`ModelSchema`] and [`Shape`] write their keys in sorted
//! (byte) order at every level, so a `serde_json::Value` round trip, which
//! sorts keys when serde_json is built without `preserve_order` and keeps them
//! when it is built with it, gives back the same bytes either way. That is
//! what keeps an ENDPOINT_HIT byte-stable through the endpoint fold, which
//! keeps the payload's top-level order and re-serialises nested objects
//! through `serde_json::Value` (`engine/src/endpoint_fold.rs` `Fields`).
//! [`ServerWire`]'s top-level order is fixed by hand (A12.1's leading keys);
//! its builder rewrite goes through [`ServerWire::to_payload`], never a
//! `Value` round trip. A later packet that needs a new key adds it here and
//! keeps the sorted rule.
//!
//! **Kinds.** [`json_kind`] reads a field's declared type text as the JSON
//! kind its value takes on the wire, per source [`WireFormat`];
//! [`json_schema_kind`] reads an OpenAPI / JSON-Schema type as
//! `contracts.rs` renders it. [`element_type`] peels pointers, nullability and
//! one array layer off a type text to the model name a builder resolves.
//!
//! Strings are escaped through `serde_json::to_string`. Readers are strict
//! about the type of every key they know (a mistyped key rejects the
//! payload) and ignore keys they do not know.

use glia_core::{Cell, CellPayload};
use serde_json::{Map, Value};

use crate::cell_type;

/// The longest `raw` text a payload keeps, in chars ([`clip_raw`]).
pub const RAW_MAX: usize = 120;

/// The `window` of every [`ServerWire`] cell: the value was read at one site.
/// A12.1's queue cells write `near` / `file`.
pub const SERVER_WINDOW: &str = "site";

// ---- formats and forms ------------------------------------------------------

/// The source language a model or a wire shape was read from: the `format`
/// of a [`ModelSchema`], and the type grammar [`json_kind`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WireFormat {
    /// TypeScript interfaces and field-only classes.
    Ts,
    /// Go structs (encoding/json tag rules).
    Go,
    /// Dart `fromJson` / json_serializable classes.
    Dart,
    /// GraphQL SDL object / input / interface types.
    Graphql,
}

impl WireFormat {
    /// Every format, in declaration order.
    pub const ALL: [WireFormat; 4] = [WireFormat::Ts, WireFormat::Go, WireFormat::Dart, WireFormat::Graphql];

    /// The `format` text: `ts`, `go`, `dart` or `graphql`.
    pub fn as_str(self) -> &'static str {
        match self {
            WireFormat::Ts => "ts",
            WireFormat::Go => "go",
            WireFormat::Dart => "dart",
            WireFormat::Graphql => "graphql",
        }
    }

    /// The format a `format` text names; `None` for any other schema format
    /// (`proto`, `avro`, `openapi`, ...), which this vocabulary does not own.
    pub fn parse(s: &str) -> Option<WireFormat> {
        WireFormat::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

/// What a [`ServerWire`] site does with its value (`form`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WireForm {
    /// A handler writes the value as its JSON response body.
    JsonResponse,
    /// A handler binds the JSON request body into the value.
    JsonBind,
    /// A GraphQL root field's declared return type.
    GraphqlReturns,
    /// A GraphQL operation's selection set.
    GraphqlSelection,
    /// The TypeScript result type a GraphQL client call names.
    GraphqlResult,
}

impl WireForm {
    /// Every form, in declaration order.
    pub const ALL: [WireForm; 5] = [
        WireForm::JsonResponse,
        WireForm::JsonBind,
        WireForm::GraphqlReturns,
        WireForm::GraphqlSelection,
        WireForm::GraphqlResult,
    ];

    /// The `form` text.
    pub fn as_str(self) -> &'static str {
        match self {
            WireForm::JsonResponse => "json_response",
            WireForm::JsonBind => "json_bind",
            WireForm::GraphqlReturns => "graphql_returns",
            WireForm::GraphqlSelection => "graphql_selection",
            WireForm::GraphqlResult => "graphql_result",
        }
    }

    /// The form a `form` text names; `None` for A12.1's queue forms
    /// (`generic`, `struct_literal`) and anything else.
    pub fn parse(s: &str) -> Option<WireForm> {
        WireForm::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

// ---- model schema (SCHEMA_FIELDS) -------------------------------------------

/// One declared field of a model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelField {
    /// The JSON name the field travels under (`name`).
    pub name: String,
    /// The source member, written only when it differs from `name` (a Go
    /// json tag rename, a Dart `@JsonKey(name:)`).
    pub field: Option<String>,
    /// The declared type text, as written (`type`).
    pub ty: String,
    /// Always present on the wire (`required`): not optional, not omitempty.
    pub required: bool,
    /// The type is an array of one element type (`array`, [`element_type`]).
    /// A TS tuple is not (its [`json_kind`] is still array), nor is Go
    /// `[]byte` (a base64 string on the wire).
    pub array: bool,
    /// A request binder rejects a body without it (`"bind":"required"`: Go
    /// `binding:"required"` / `validate:"required"`).
    pub bind_required: bool,
    /// The in-repo model qname the element type resolved to (`model`),
    /// written by a builder.
    pub model: Option<String>,
}

impl ModelField {
    /// A field named `name` with type text `ty`; `array` is read off `ty`
    /// through [`element_type`].
    pub fn new(name: &str, ty: &str, required: bool) -> ModelField {
        ModelField {
            name: name.to_string(),
            ty: ty.to_string(),
            required,
            array: element_type(ty).1,
            ..ModelField::default()
        }
    }

    /// The source member: `field` when recorded, else the JSON name.
    pub fn member(&self) -> &str {
        self.field.as_deref().unwrap_or(&self.name)
    }

    fn write(&self, out: &mut String) {
        let mut o = Obj::new(out);
        if self.array {
            o.bool("array", true);
        }
        if self.bind_required {
            o.str("bind", "required");
        }
        if let Some(f) = self.field.as_deref().filter(|f| *f != self.name) {
            o.str("field", f);
        }
        if let Some(m) = &self.model {
            o.str("model", m);
        }
        o.str("name", &self.name);
        o.bool("required", self.required);
        o.str("type", &self.ty);
        o.end();
    }

    fn from_value(v: &Value) -> Option<ModelField> {
        let o = v.as_object()?;
        Some(ModelField {
            name: req_str(o, "name")?,
            field: opt_str(o, "field")?,
            ty: req_str(o, "type")?,
            required: o.get("required")?.as_bool()?,
            array: opt_flag(o, "array")?,
            bind_required: bind_flag(o)?,
            model: opt_str(o, "model")?,
        })
    }
}

/// A model's declared fields: the SCHEMA_FIELDS payload of a TypeScript
/// interface / field-only class, a Go struct, a Dart model class or a GraphQL
/// type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSchema {
    /// The source format (`format`).
    pub format: WireFormat,
    /// The fields, in source order (`fields`).
    pub fields: Vec<ModelField>,
    /// The bases whose fields this model also carries, as written (`extends`:
    /// a TS `extends` list, a Go untagged embed with the pointer stripped).
    pub extends: Vec<String>,
    /// Fields may exist that are not listed (`open`): an index signature, a
    /// computed key, a generic or utility base.
    pub open: bool,
}

impl ModelSchema {
    /// An empty, closed schema of `format`.
    pub fn new(format: WireFormat) -> ModelSchema {
        ModelSchema {
            format,
            fields: Vec::new(),
            extends: Vec::new(),
            open: false,
        }
    }

    /// The field whose JSON name is `name`.
    pub fn field(&self, name: &str) -> Option<&ModelField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The SCHEMA_FIELDS payload, sorted keys at every level.
    pub fn to_payload(&self) -> String {
        let mut out = String::new();
        let mut o = Obj::new(&mut out);
        if !self.extends.is_empty() {
            o.key("extends");
            write_array(o.buf(), &self.extends, |b, e| b.push_str(&q(e)));
        }
        o.key("fields");
        write_array(o.buf(), &self.fields, |b, f| f.write(b));
        o.str("format", self.format.as_str());
        if self.open {
            o.bool("open", true);
        }
        o.end();
        out
    }

    /// The schema a SCHEMA_FIELDS payload holds; `None` when it is not JSON,
    /// its `format` is not a [`WireFormat`] (a proto / Avro / contract-op
    /// schema) or a known key is mistyped.
    pub fn parse(payload: &str) -> Option<ModelSchema> {
        let v: Value = serde_json::from_str(payload).ok()?;
        let o = v.as_object()?;
        let format = WireFormat::parse(o.get("format")?.as_str()?)?;
        let fields = o
            .get("fields")?
            .as_array()?
            .iter()
            .map(ModelField::from_value)
            .collect::<Option<Vec<_>>>()?;
        let extends = match o.get("extends") {
            None => Vec::new(),
            Some(e) => e
                .as_array()?
                .iter()
                .map(|x| x.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()?,
        };
        Some(ModelSchema {
            format,
            fields,
            extends,
            open: opt_flag(o, "open")?,
        })
    }

    /// The schema a SCHEMA_FIELDS cell holds ([`ModelSchema::parse`]); `None`
    /// for any other cell.
    pub fn from_cell(cell: &Cell) -> Option<ModelSchema> {
        (cell.kind == cell_type::SCHEMA_FIELDS)
            .then(|| json_payload(cell))
            .flatten()
            .and_then(ModelSchema::parse)
    }

    /// The SCHEMA_FIELDS cell.
    pub fn cell(&self) -> Cell {
        Cell {
            kind: cell_type::SCHEMA_FIELDS,
            payload: CellPayload::Json(self.to_payload()),
        }
    }
}

// ---- wire shape -------------------------------------------------------------

/// The value a call site sends or receives, classified as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A declared type: `{"array"?,"model"?,"type":"Order"}`. `ty` is the
    /// element type ([`element_type`]), qualifier kept; `array` the one
    /// array layer peeled off it.
    Named {
        ty: String,
        array: bool,
        model: Option<String>,
    },
    /// A call's result: `{"array"?,"call":"self.store.Orders()","model"?}`.
    Call {
        chain: String,
        array: bool,
        model: Option<String>,
    },
    /// A field chain: `{"array"?,"chain":"self.cfg.X","model"?}`.
    Chain {
        chain: String,
        array: bool,
        model: Option<String>,
    },
    /// An inline object (an envelope literal, an anonymous struct, a TS
    /// object type, a GraphQL selection): `{"fields":[SF..],"open"?}`.
    Object { fields: Vec<ShapeField>, open: bool },
    /// One of several shapes: `{"union":[S..]}`.
    Union(Vec<Shape>),
    /// Nothing is known (`any`, an unread expression):
    /// `{"open":true,"raw":"<= 120 chars"}`.
    Open { raw: String },
}

/// One key of an inline [`Shape::Object`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeField {
    /// The response / request key (`name`).
    pub name: String,
    /// The schema field when `name` is an alias (a GraphQL `alias: field`),
    /// written only when it differs from `name`.
    pub field: Option<String>,
    /// Always present (`required`).
    pub required: bool,
    /// A request binder rejects a body without it (`"bind":"required"`).
    pub bind_required: bool,
    /// The key's value (`shape`).
    pub shape: Shape,
}

impl ShapeField {
    /// A key `name` holding `shape`.
    pub fn new(name: &str, required: bool, shape: Shape) -> ShapeField {
        ShapeField {
            name: name.to_string(),
            field: None,
            required,
            bind_required: false,
            shape,
        }
    }

    fn write(&self, out: &mut String) {
        let mut o = Obj::new(out);
        if self.bind_required {
            o.str("bind", "required");
        }
        if let Some(f) = self.field.as_deref().filter(|f| *f != self.name) {
            o.str("field", f);
        }
        o.str("name", &self.name);
        o.bool("required", self.required);
        o.key("shape");
        self.shape.write(o.buf());
        o.end();
    }

    fn from_value(v: &Value) -> Option<ShapeField> {
        let o = v.as_object()?;
        Some(ShapeField {
            name: req_str(o, "name")?,
            field: opt_str(o, "field")?,
            required: o.get("required")?.as_bool()?,
            bind_required: bind_flag(o)?,
            shape: Shape::from_value(o.get("shape")?)?,
        })
    }
}

impl Shape {
    /// A declared type read from its text: [`Shape::Named`] of the element
    /// type, `array` when one array layer was peeled off.
    pub fn named(text: &str) -> Shape {
        let (ty, array) = element_type(text);
        Shape::Named { ty, array, model: None }
    }

    /// An [`Shape::Open`] whose `raw` is `raw` clipped to [`RAW_MAX`] chars.
    pub fn open(raw: &str) -> Shape {
        Shape::Open { raw: clip_raw(raw) }
    }

    /// The in-repo model a builder resolved this shape to.
    pub fn model(&self) -> Option<&str> {
        match self {
            Shape::Named { model, .. } | Shape::Call { model, .. } | Shape::Chain { model, .. } => model.as_deref(),
            _ => None,
        }
    }

    /// Record the model a [`Shape::Named`] / [`Shape::Call`] /
    /// [`Shape::Chain`] resolved to; `false` (unchanged) for any other shape.
    pub fn set_model(&mut self, qname: &str) -> bool {
        match self {
            Shape::Named { model, .. } | Shape::Call { model, .. } | Shape::Chain { model, .. } => {
                *model = Some(qname.to_string());
                true
            }
            _ => false,
        }
    }

    /// The value is an array of the shape's element (`array`).
    pub fn is_array(&self) -> bool {
        matches!(
            self,
            Shape::Named { array: true, .. } | Shape::Call { array: true, .. } | Shape::Chain { array: true, .. }
        )
    }

    /// Fields may exist that the shape does not list: an [`Shape::Open`], an
    /// open [`Shape::Object`], or a union with such a member.
    pub fn is_open(&self) -> bool {
        match self {
            Shape::Open { .. } => true,
            Shape::Object { open, .. } => *open,
            Shape::Union(members) => members.iter().any(Shape::is_open),
            _ => false,
        }
    }

    /// The simple name of a [`Shape::Named`] type (after the last `.` /
    /// `::`, type arguments dropped): A12.1's `type`. Empty for every other
    /// shape.
    pub fn simple_name(&self) -> &str {
        match self {
            Shape::Named { ty, .. } => simple_type_name(ty),
            _ => "",
        }
    }

    /// The shape as JSON, sorted keys at every level.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    /// The shape a JSON text holds ([`Shape::from_value`]).
    pub fn parse(json: &str) -> Option<Shape> {
        Shape::from_value(&serde_json::from_str(json).ok()?)
    }

    /// The shape a JSON value holds, told apart by its defining key
    /// (`union`, `fields`, `type`, `call`, `chain`, then `open` + `raw`);
    /// `None` when none is present or a known key is mistyped.
    pub fn from_value(v: &Value) -> Option<Shape> {
        let o = v.as_object()?;
        if let Some(u) = o.get("union") {
            let members = u
                .as_array()?
                .iter()
                .map(Shape::from_value)
                .collect::<Option<Vec<_>>>()?;
            return Some(Shape::Union(members));
        }
        if let Some(fs) = o.get("fields") {
            let fields = fs
                .as_array()?
                .iter()
                .map(ShapeField::from_value)
                .collect::<Option<Vec<_>>>()?;
            return Some(Shape::Object {
                fields,
                open: opt_flag(o, "open")?,
            });
        }
        let array = opt_flag(o, "array")?;
        let model = opt_str(o, "model")?;
        if let Some(t) = o.get("type") {
            return Some(Shape::Named {
                ty: t.as_str()?.to_string(),
                array,
                model,
            });
        }
        if let Some(c) = o.get("call") {
            return Some(Shape::Call {
                chain: c.as_str()?.to_string(),
                array,
                model,
            });
        }
        if let Some(c) = o.get("chain") {
            return Some(Shape::Chain {
                chain: c.as_str()?.to_string(),
                array,
                model,
            });
        }
        if o.get("open")?.as_bool()? {
            return Some(Shape::Open {
                raw: req_str(o, "raw")?,
            });
        }
        None
    }

    fn write(&self, out: &mut String) {
        let mut o = Obj::new(out);
        match self {
            Shape::Named { ty, array, model } => {
                if *array {
                    o.bool("array", true);
                }
                if let Some(m) = model {
                    o.str("model", m);
                }
                o.str("type", ty);
            }
            Shape::Call { chain, array, model } => {
                if *array {
                    o.bool("array", true);
                }
                o.str("call", chain);
                if let Some(m) = model {
                    o.str("model", m);
                }
            }
            Shape::Chain { chain, array, model } => {
                if *array {
                    o.bool("array", true);
                }
                o.str("chain", chain);
                if let Some(m) = model {
                    o.str("model", m);
                }
            }
            Shape::Object { fields, open } => {
                o.key("fields");
                write_array(o.buf(), fields, |b, f| f.write(b));
                if *open {
                    o.bool("open", true);
                }
            }
            Shape::Union(members) => {
                o.key("union");
                write_array(o.buf(), members, |b, s| s.write(b));
            }
            Shape::Open { raw } => {
                o.bool("open", true);
                o.str("raw", &clip_raw(raw));
            }
        }
        o.end();
    }
}

// ---- server site (MESSAGE_TYPE) ---------------------------------------------

/// A server-side wire site: what a handler writes or binds, what a GraphQL
/// root field returns or an operation selects. A MESSAGE_TYPE cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerWire {
    /// A12.1's `type`: the [`Shape::simple_name`] of a [`Shape::Named`]
    /// value, or the resolved model's simple name; empty otherwise.
    pub ty: String,
    /// The value's text as written, clipped to [`RAW_MAX`] chars (`raw`).
    pub raw: String,
    /// What the site does with the value (`form`).
    pub form: WireForm,
    /// The site's 1-based line (`line`).
    pub line: u32,
    /// The response status when known (`status`): written, or the implicit
    /// 200 of a form with no status argument. `None` is unknown (a variable,
    /// a call): such a site is never judged.
    pub status: Option<u16>,
    /// The GraphQL document identifier a client call names (`document`).
    pub document: Option<String>,
    /// The value (`shape`).
    pub shape: Shape,
}

impl ServerWire {
    /// A site of `form` at 1-based `line` whose value is `shape`, written as
    /// `raw`; `ty` is the shape's [`Shape::simple_name`], `status` and
    /// `document` unset.
    pub fn new(form: WireForm, raw: &str, line: u32, shape: Shape) -> ServerWire {
        ServerWire {
            ty: shape.simple_name().to_string(),
            raw: clip_raw(raw),
            form,
            line,
            status: None,
            document: None,
            shape,
        }
    }

    /// The status is known and 2xx: the only sites a comparison reads.
    pub fn known_2xx(&self) -> bool {
        self.status.is_some_and(|s| (200..300).contains(&s))
    }

    /// The MESSAGE_TYPE payload, keys in this fixed order: `type`, `raw`,
    /// `form`, `window` (A12.1's leading keys), `line`, `status`?,
    /// `document`?, `shape` (sorted inside).
    pub fn to_payload(&self) -> String {
        let mut out = String::new();
        let mut o = Obj::new(&mut out);
        o.str("type", &self.ty);
        o.str("raw", &clip_raw(&self.raw));
        o.str("form", self.form.as_str());
        o.str("window", SERVER_WINDOW);
        o.num("line", u64::from(self.line));
        if let Some(s) = self.status {
            o.num("status", u64::from(s));
        }
        if let Some(d) = &self.document {
            o.str("document", d);
        }
        o.key("shape");
        self.shape.write(o.buf());
        o.end();
        out
    }

    /// The site a MESSAGE_TYPE payload holds; `None` for an A12.1 queue cell
    /// (its `window` is `near` / `file`), an unknown `form`, or a missing or
    /// mistyped key.
    pub fn parse(payload: &str) -> Option<ServerWire> {
        let v: Value = serde_json::from_str(payload).ok()?;
        let o = v.as_object()?;
        if o.get("window")?.as_str()? != SERVER_WINDOW {
            return None;
        }
        let status = match o.get("status") {
            None => None,
            Some(s) => Some(u16::try_from(s.as_u64()?).ok()?),
        };
        Some(ServerWire {
            ty: req_str(o, "type")?,
            raw: req_str(o, "raw")?,
            form: WireForm::parse(o.get("form")?.as_str()?)?,
            line: u32::try_from(o.get("line")?.as_u64()?).ok()?,
            status,
            document: opt_str(o, "document")?,
            shape: Shape::from_value(o.get("shape")?)?,
        })
    }

    /// The site a MESSAGE_TYPE cell holds ([`ServerWire::parse`]); `None` for
    /// any other cell.
    pub fn from_cell(cell: &Cell) -> Option<ServerWire> {
        (cell.kind == cell_type::MESSAGE_TYPE)
            .then(|| json_payload(cell))
            .flatten()
            .and_then(ServerWire::parse)
    }

    /// The MESSAGE_TYPE cell.
    pub fn cell(&self) -> Cell {
        Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json(self.to_payload()),
        }
    }
}

// ---- ENDPOINT_HIT shapes ----------------------------------------------------

/// `payload` (an ENDPOINT_HIT object) with `,"response":S` and then
/// `,"request":S` spliced in before its last `}`. The old bytes up to that
/// `}` stay a prefix; with neither shape (or no `}`) the payload comes back
/// unchanged. Called once per hit, by the parser that wrote it.
pub fn append_hit_fields(payload: &str, response: Option<&Shape>, request: Option<&Shape>) -> String {
    let Some(end) = payload.rfind('}') else {
        return payload.to_string();
    };
    if response.is_none() && request.is_none() {
        return payload.to_string();
    }
    let mut out = String::with_capacity(payload.len() + 64);
    out.push_str(&payload[..end]);
    let mut empty = payload[..end].trim_end().ends_with('{');
    for (key, shape) in [("response", response), ("request", request)] {
        if let Some(s) = shape {
            if !empty {
                out.push(',');
            }
            empty = false;
            out.push('"');
            out.push_str(key);
            out.push_str("\":");
            s.write(&mut out);
        }
    }
    out.push_str(&payload[end..]);
    out
}

/// The `response` and `request` shapes of an ENDPOINT_HIT payload; `None`
/// for a key that is absent or does not hold a [`Shape`].
pub fn hit_shapes(payload: &str) -> (Option<Shape>, Option<Shape>) {
    let Ok(v) = serde_json::from_str::<Value>(payload) else {
        return (None, None);
    };
    let get = |k: &str| v.get(k).and_then(Shape::from_value);
    (get("response"), get("request"))
}

// ---- JSON kinds -------------------------------------------------------------

/// The JSON kind a declared type's value takes on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsonKind {
    String,
    Number,
    Boolean,
    Array,
    Object,
    /// Any JSON value (`any`, `interface{}`, `dynamic`, a mixed union):
    /// never a mismatch.
    Any,
    Null,
    /// A named type the format's table does not know, as written with
    /// nullability peeled: the consumer decides (a resolved model is an
    /// object; anything else is never judged).
    Unknown(String),
}

impl JsonKind {
    /// The kind's name: `string`, `number`, `boolean`, `array`, `object`,
    /// `any`, `null`, or `unknown`.
    pub fn as_str(&self) -> &'static str {
        match self {
            JsonKind::String => "string",
            JsonKind::Number => "number",
            JsonKind::Boolean => "boolean",
            JsonKind::Array => "array",
            JsonKind::Object => "object",
            JsonKind::Any => "any",
            JsonKind::Null => "null",
            JsonKind::Unknown(_) => "unknown",
        }
    }
}

/// The JSON kind of a value declared with type text `ty` in `format`.
///
/// - `ts`: `string`, string-literal and template-literal types -> string;
///   `number` / `bigint` and numeric literals -> number; `boolean`, `true` /
///   `false` -> boolean; `X[]`, `Array<X>`, `ReadonlyArray<X>`, tuples ->
///   array; an inline object or `Record<..>` -> object; `any` / `unknown` /
///   `object` / `Date` -> any; `null` / `undefined` -> null. A union drops its
///   `null` / `undefined` members and is the one kind its members share, else
///   any; an intersection is any.
/// - `go`: `string`, `time.Time`, `uuid.UUID`, `[]byte` -> string; `int*`,
///   `uint*`, `float*`, `byte`, `rune`, `json.Number` -> number; `bool` ->
///   boolean; `[]T` / `[N]T` -> array; `map[..]..` and an inline struct ->
///   object; `any`, `interface{..}`, `json.RawMessage` -> any; `*T` is `T`.
/// - `dart`: `String` -> string; `int` / `double` / `num` -> number; `bool`
///   -> boolean; `List` / `Iterable` -> array; `Map` -> object; `dynamic` /
///   `Object` / `DateTime` -> any; `Null` -> null; `T?` is `T`.
/// - `graphql`: `ID` / `String` -> string; `Int` / `Float` -> number;
///   `Boolean` -> boolean; `[X]` -> array; `X!` is `X`.
///
/// Every other type is [`JsonKind::Unknown`].
pub fn json_kind(format: WireFormat, ty: &str) -> JsonKind {
    match format {
        WireFormat::Ts => ts_kind(ty, 0),
        WireFormat::Go => go_kind(ty),
        WireFormat::Dart => dart_kind(ty),
        WireFormat::Graphql => graphql_kind(ty),
    }
}

/// The JSON kind of an OpenAPI / AsyncAPI / JSON-Schema field type as
/// `contracts.rs` renders it (`integer(int32)`, `string|null`,
/// `oneOf<A|B>`): `string`, `integer` / `number`, `boolean`, `array`,
/// `object`, `null` by name, with the `(format)` suffix and `null` members
/// dropped; `any`, `oneOf<..>`, `anyOf<..>` and a mixed type list -> any;
/// anything else -> [`JsonKind::Unknown`].
pub fn json_schema_kind(ty: &str) -> JsonKind {
    let t = ty.trim();
    if t.starts_with("oneOf<") || t.starts_with("anyOf<") {
        return JsonKind::Any;
    }
    let kinds: Vec<JsonKind> = t
        .split('|')
        .map(|m| {
            let m = m.trim();
            let m = m.split_once('(').map_or(m, |(base, _)| base.trim());
            match m {
                "string" => JsonKind::String,
                "integer" | "number" => JsonKind::Number,
                "boolean" => JsonKind::Boolean,
                "array" => JsonKind::Array,
                "object" => JsonKind::Object,
                "null" => JsonKind::Null,
                "any" => JsonKind::Any,
                _ => JsonKind::Unknown(m.to_string()),
            }
        })
        .collect();
    one_kind(kinds)
}

/// The kind a union's members share once its null members are dropped:
/// null when none is left, any when they disagree.
fn one_kind(kinds: Vec<JsonKind>) -> JsonKind {
    let mut left = kinds.into_iter().filter(|k| *k != JsonKind::Null);
    let Some(first) = left.next() else {
        return JsonKind::Null;
    };
    if left.all(|k| k == first) { first } else { JsonKind::Any }
}

fn ts_kind(ty: &str, depth: usize) -> JsonKind {
    let t = strip_readonly(unparen(ty.trim()));
    if depth > 8 {
        return JsonKind::Any;
    }
    let union: Vec<&str> = split_top(t, '|').into_iter().filter(|m| !m.is_empty()).collect();
    if union.len() > 1 {
        return one_kind(union.into_iter().map(|m| ts_kind(m, depth + 1)).collect());
    }
    let t = union.first().copied().unwrap_or(t);
    let t = strip_readonly(unparen(t));
    if split_top(t, '&').len() > 1 {
        return JsonKind::Any;
    }
    match t {
        "string" => return JsonKind::String,
        "number" | "bigint" => return JsonKind::Number,
        "boolean" | "true" | "false" => return JsonKind::Boolean,
        "any" | "unknown" | "object" | "Date" => return JsonKind::Any,
        "null" | "undefined" => return JsonKind::Null,
        _ => {}
    }
    if t.starts_with(['\'', '"', '`']) {
        return JsonKind::String;
    }
    // A numeric literal type (`1 | 2`); the leading-char gate keeps `inf` /
    // `NaN`-named types, which `f64::from_str` accepts, out.
    if t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') && t.parse::<f64>().is_ok() {
        return JsonKind::Number;
    }
    if t.starts_with('{') || generic_arg(t, "Record").is_some() {
        return JsonKind::Object;
    }
    if t.starts_with('[') || array_layer(t).is_some() {
        return JsonKind::Array;
    }
    JsonKind::Unknown(t.to_string())
}

fn go_kind(ty: &str) -> JsonKind {
    let mut t = ty.trim();
    while let Some(rest) = t.strip_prefix('*') {
        t = rest.trim_start();
    }
    match t {
        "string" | "time.Time" | "uuid.UUID" | "[]byte" | "[]uint8" => JsonKind::String,
        "bool" => JsonKind::Boolean,
        "byte" | "rune" | "json.Number" => JsonKind::Number,
        "any" | "json.RawMessage" => JsonKind::Any,
        _ if is_go_number(t) => JsonKind::Number,
        _ if t.starts_with('[') => JsonKind::Array,
        _ if t.starts_with("map[") => JsonKind::Object,
        _ if keyword_block(t, "struct") => JsonKind::Object,
        _ if keyword_block(t, "interface") => JsonKind::Any,
        _ => JsonKind::Unknown(t.to_string()),
    }
}

/// `int`, `int8`..`int64`, `uint`, `uint8`..`uint64`, `uintptr`, `float32`,
/// `float64`.
fn is_go_number(t: &str) -> bool {
    let bits = |rest: &str| matches!(rest, "" | "8" | "16" | "32" | "64");
    if let Some(r) = t.strip_prefix("uint") {
        return bits(r) || r == "ptr";
    }
    if let Some(r) = t.strip_prefix("int") {
        return bits(r);
    }
    matches!(t, "float32" | "float64")
}

/// `t` is `<kw>{..}` or `<kw> {..}` (a Go inline struct / interface type).
fn keyword_block(t: &str, kw: &str) -> bool {
    t.strip_prefix(kw).is_some_and(|r| r.trim_start().starts_with('{'))
}

fn dart_kind(ty: &str) -> JsonKind {
    let mut t = ty.trim();
    while let Some(rest) = t.strip_suffix('?') {
        t = rest.trim_end();
    }
    let base = t.split_once('<').map_or(t, |(b, _)| b.trim_end());
    match base {
        "String" => JsonKind::String,
        "int" | "double" | "num" => JsonKind::Number,
        "bool" => JsonKind::Boolean,
        "List" | "Iterable" => JsonKind::Array,
        "Map" => JsonKind::Object,
        "dynamic" | "Object" | "DateTime" => JsonKind::Any,
        "Null" => JsonKind::Null,
        _ => JsonKind::Unknown(t.to_string()),
    }
}

fn graphql_kind(ty: &str) -> JsonKind {
    let mut t = ty.trim();
    while let Some(rest) = t.strip_suffix('!') {
        t = rest.trim_end();
    }
    match t {
        "ID" | "String" => JsonKind::String,
        "Int" | "Float" => JsonKind::Number,
        "Boolean" => JsonKind::Boolean,
        _ if t.starts_with('[') => JsonKind::Array,
        _ => JsonKind::Unknown(t.to_string()),
    }
}

// ---- element types ----------------------------------------------------------

/// The element type of a declared type text and whether one array layer was
/// peeled off it: `*`, `?`, `!`, wrapping parentheses, a TS `readonly` and
/// `| null` / `| undefined` union members are stripped repeatedly, plus one
/// array layer (`[]X`, `[N]X`, `X[]`, `Array<X>`, `ReadonlyArray<X>`,
/// `List<X>`, `Iterable<X>`, `[X]`). Qualifiers are kept (`models.Order`).
/// Go `[]byte` is not an array (a base64 string), and a TS tuple has no one
/// element type, so both come back whole with `false`.
///
/// `[]*models.Order` -> (`models.Order`, true); `Ticket | null` ->
/// (`Ticket`, false); `[Item!]!` -> (`Item`, true).
pub fn element_type(ty: &str) -> (String, bool) {
    let mut t = ty.trim();
    let mut array = false;
    loop {
        let before = t;
        t = strip_nullish(t);
        if !array && let Some(inner) = array_layer(t) {
            t = inner.trim();
            array = true;
            continue;
        }
        if t == before {
            break;
        }
    }
    (t.to_string(), array)
}

/// One round of the non-array stripping [`element_type`] repeats.
fn strip_nullish(t: &str) -> &str {
    let mut t = strip_readonly(unparen(t.trim()));
    if let Some(r) = t.strip_prefix('*') {
        t = r.trim_start();
    }
    if let Some(r) = t.strip_suffix('?').or_else(|| t.strip_suffix('!')) {
        t = r.trim_end();
    }
    let members: Vec<&str> = split_top(t, '|').into_iter().filter(|m| !m.is_empty()).collect();
    if members.len() > 1 {
        let kept: Vec<&str> = members
            .into_iter()
            .filter(|m| !matches!(*m, "null" | "undefined"))
            .collect();
        if let [one] = kept.as_slice() {
            t = one;
        }
    }
    t
}

/// The element text inside one array layer of `t`, or `None`. Go `[]byte`
/// / `[]uint8` is no layer: encoding/json writes it as a base64 string.
fn array_layer(t: &str) -> Option<&str> {
    if matches!(t, "[]byte" | "[]uint8") {
        return None;
    }
    if t.starts_with('[') {
        let close = matching(t, 0)?;
        let inner = &t[1..close];
        if close + 1 == t.len() {
            // GraphQL `[X]` (a TS tuple `[A, B]` has no single element).
            return (split_top(inner, ',').len() == 1 && !inner.trim().is_empty()).then_some(inner);
        }
        // Go `[]X` / `[N]X` / `[...]X`.
        return Some(&t[close + 1..]);
    }
    if let Some(inner) = t.strip_suffix("[]") {
        return (!inner.trim().is_empty()).then_some(inner);
    }
    ["Array", "ReadonlyArray", "List", "Iterable"]
        .into_iter()
        .find_map(|g| generic_arg(t, g))
}

/// The argument text of `t` when it is exactly `<name><..>` with one
/// argument list closing at the end.
fn generic_arg<'a>(t: &'a str, name: &str) -> Option<&'a str> {
    let rest = t.strip_prefix(name)?;
    if !rest.starts_with('<') {
        return None;
    }
    let open = name.len();
    (matching(t, open)? + 1 == t.len()).then(|| &t[open + 1..t.len() - 1])
}

/// `t` without one pair of parentheses wrapping all of it.
fn unparen(t: &str) -> &str {
    let mut t = t;
    while t.starts_with('(') && matching(t, 0).is_some_and(|c| c + 1 == t.len()) {
        t = t[1..t.len() - 1].trim();
    }
    t
}

fn strip_readonly(t: &str) -> &str {
    t.strip_prefix("readonly ").map_or(t, str::trim_start)
}

/// The byte index of the bracket closing the one opening at `open`, skipping
/// quoted text and a TS `=>`.
fn matching(t: &str, open: usize) -> Option<usize> {
    let b = t.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < b.len() {
        let c = b[i];
        if let Some(qc) = quote {
            if c == b'\\' {
                i += 1;
            } else if c == qc {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'=' if b.get(i + 1) == Some(&b'>') => i += 1,
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// `t` split on `sep` outside brackets and quotes, each part trimmed.
fn split_top(t: &str, sep: char) -> Vec<&str> {
    let b = t.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if let Some(qc) = quote {
            if c == b'\\' {
                i += 1;
            } else if c == qc {
                quote = None;
            }
        } else {
            match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'=' if b.get(i + 1) == Some(&b'>') => i += 1,
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
                _ if depth == 0 && c == sep as u8 => {
                    parts.push(t[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    parts.push(t[start..].trim());
    parts
}

/// The name after the last `.` / `::` of a type text, its type arguments
/// (`<..>`, Go `[..]`) dropped: `models.Page[T]` -> `Page`.
fn simple_type_name(ty: &str) -> &str {
    let t = ty.split(['<', '[']).next().unwrap_or(ty).trim_end();
    let t = t.rsplit("::").next().unwrap_or(t);
    t.rsplit('.').next().unwrap_or(t)
}

/// `s` cut to its first [`RAW_MAX`] chars (on a char boundary).
pub fn clip_raw(s: &str) -> String {
    match s.char_indices().nth(RAW_MAX) {
        Some((cut, _)) => s[..cut].to_string(),
        None => s.to_string(),
    }
}

// ---- JSON plumbing ----------------------------------------------------------

/// A JSON string literal for `s`, escaped by serde_json.
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""))
}

/// A JSON object written key by key, in the order the caller writes them.
struct Obj<'a> {
    out: &'a mut String,
    first: bool,
}

impl<'a> Obj<'a> {
    fn new(out: &'a mut String) -> Obj<'a> {
        out.push('{');
        Obj { out, first: true }
    }

    fn key(&mut self, k: &str) {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        self.out.push_str(&q(k));
        self.out.push(':');
    }

    fn str(&mut self, k: &str, v: &str) {
        self.key(k);
        self.out.push_str(&q(v));
    }

    fn bool(&mut self, k: &str, v: bool) {
        self.key(k);
        self.out.push_str(if v { "true" } else { "false" });
    }

    fn num(&mut self, k: &str, v: u64) {
        self.key(k);
        self.out.push_str(&v.to_string());
    }

    fn buf(&mut self) -> &mut String {
        self.out
    }

    fn end(self) {
        self.out.push('}');
    }
}

fn write_array<T>(out: &mut String, items: &[T], mut each: impl FnMut(&mut String, &T)) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        each(out, item);
    }
    out.push(']');
}

fn json_payload(cell: &Cell) -> Option<&str> {
    match &cell.payload {
        CellPayload::Json(s) => Some(s),
        _ => None,
    }
}

/// A required string key.
fn req_str(o: &Map<String, Value>, k: &str) -> Option<String> {
    o.get(k)?.as_str().map(str::to_string)
}

/// An optional string key: `Some(None)` when absent, `None` when mistyped.
fn opt_str(o: &Map<String, Value>, k: &str) -> Option<Option<String>> {
    match o.get(k) {
        None => Some(None),
        Some(v) => v.as_str().map(|s| Some(s.to_string())),
    }
}

/// An optional `true` flag: `Some(false)` when absent, `None` when mistyped.
fn opt_flag(o: &Map<String, Value>, k: &str) -> Option<bool> {
    match o.get(k) {
        None => Some(false),
        Some(v) => v.as_bool(),
    }
}

/// `"bind":"required"`: `Some(false)` when absent, `None` for any other value.
fn bind_flag(o: &Map<String, Value>) -> Option<bool> {
    match o.get("bind") {
        None => Some(false),
        Some(v) => (v.as_str()? == "required").then_some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    use std::fmt;

    // ---- an order-preserving JSON tree, independent of serde_json features --

    /// JSON parsed with every object's keys in the order written, so the
    /// sorted-key check does not depend on serde_json's `preserve_order`.
    enum Tree {
        Obj(Vec<(String, Tree)>),
        Arr(Vec<Tree>),
        Leaf,
    }

    impl<'de> Deserialize<'de> for Tree {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Tree, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Tree;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("JSON")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Tree, A::Error> {
                    let mut out = Vec::new();
                    while let Some(e) = m.next_entry::<String, Tree>()? {
                        out.push(e);
                    }
                    Ok(Tree::Obj(out))
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Tree, A::Error> {
                    let mut out = Vec::new();
                    while let Some(e) = s.next_element::<Tree>()? {
                        out.push(e);
                    }
                    Ok(Tree::Arr(out))
                }
                fn visit_str<E>(self, _: &str) -> Result<Tree, E> {
                    Ok(Tree::Leaf)
                }
                fn visit_bool<E>(self, _: bool) -> Result<Tree, E> {
                    Ok(Tree::Leaf)
                }
                fn visit_u64<E>(self, _: u64) -> Result<Tree, E> {
                    Ok(Tree::Leaf)
                }
                fn visit_i64<E>(self, _: i64) -> Result<Tree, E> {
                    Ok(Tree::Leaf)
                }
                fn visit_f64<E>(self, _: f64) -> Result<Tree, E> {
                    Ok(Tree::Leaf)
                }
                fn visit_unit<E>(self) -> Result<Tree, E> {
                    Ok(Tree::Leaf)
                }
            }
            d.deserialize_any(V)
        }
    }

    /// Every object at or below `t` (the top one too unless `skip_top`) has
    /// its keys in strictly ascending byte order.
    fn sorted(t: &Tree, skip_top: bool) -> bool {
        match t {
            Tree::Obj(kv) => {
                (skip_top || kv.windows(2).all(|w| w[0].0 < w[1].0)) && kv.iter().all(|(_, v)| sorted(v, false))
            }
            Tree::Arr(xs) => xs.iter().all(|x| sorted(x, false)),
            Tree::Leaf => true,
        }
    }

    /// The `serde_json::Value` round trip of a sorted writer's output.
    fn value_round_trip(p: &str) -> String {
        serde_json::to_string(&serde_json::from_str::<Value>(p).unwrap()).unwrap()
    }

    /// The endpoint fold's round trip (`engine/src/endpoint_fold.rs`
    /// `Fields`): the top level keeps its order, nested values go through
    /// `serde_json::Value`.
    fn fold_round_trip(p: &str) -> String {
        struct Fields(Vec<(String, Value)>);
        impl<'de> Deserialize<'de> for Fields {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Fields, D::Error> {
                struct V;
                impl<'de> Visitor<'de> for V {
                    type Value = Fields;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("a JSON object")
                    }
                    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Fields, A::Error> {
                        let mut out = Vec::new();
                        while let Some(e) = m.next_entry::<String, Value>()? {
                            out.push(e);
                        }
                        Ok(Fields(out))
                    }
                }
                d.deserialize_map(V)
            }
        }
        impl serde::Serialize for Fields {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
            }
        }
        let f: Fields = serde_json::from_str(p).unwrap();
        serde_json::to_string(&f).unwrap()
    }

    /// A sorted writer's output: sorted at every level and byte-equal through
    /// a Value round trip.
    fn stable_sorted(p: &str) -> bool {
        let t: Tree = serde_json::from_str(p).unwrap();
        sorted(&t, false) && value_round_trip(p) == p
    }

    /// A fixed-top-level writer's output (ServerWire, a spliced hit): sorted
    /// below the top and byte-equal through the fold's round trip.
    fn stable_fixed_top(p: &str) -> bool {
        let t: Tree = serde_json::from_str(p).unwrap();
        sorted(&t, true) && fold_round_trip(p) == p
    }

    // ---- fixtures ------------------------------------------------------------

    fn named(ty: &str, array: bool, model: Option<&str>) -> Shape {
        Shape::Named {
            ty: ty.into(),
            array,
            model: model.map(Into::into),
        }
    }

    fn full_field() -> ModelField {
        ModelField {
            name: "items".into(),
            field: Some("Items".into()),
            ty: "[]*models.Item".into(),
            required: true,
            array: true,
            bind_required: true,
            model: Some("models::Item".into()),
        }
    }

    fn full_schema() -> ModelSchema {
        ModelSchema {
            format: WireFormat::Go,
            fields: vec![full_field(), ModelField::new("note", "*string", false)],
            extends: vec!["Base".into(), "audit.Stamp".into()],
            open: true,
        }
    }

    /// One of every shape form, nested.
    fn every_shape() -> Vec<Shape> {
        let object = Shape::Object {
            fields: vec![
                ShapeField::new("offers", true, named("Offer", true, Some("src::models::Offer"))),
                ShapeField {
                    name: "total".into(),
                    field: Some("count".into()),
                    required: false,
                    bind_required: true,
                    shape: Shape::Chain {
                        chain: "self.cfg.Total".into(),
                        array: false,
                        model: None,
                    },
                },
            ],
            open: true,
        };
        vec![
            named("Order", false, None),
            named("models.Order", true, Some("models::Order")),
            Shape::Call {
                chain: "self.store.Orders()".into(),
                array: true,
                model: Some("models::Order".into()),
            },
            Shape::Call {
                chain: "h.svc.Get(id)".into(),
                array: false,
                model: None,
            },
            Shape::Chain {
                chain: "self.cfg.X".into(),
                array: true,
                model: Some("cfg::X".into()),
            },
            object.clone(),
            Shape::Object {
                fields: vec![],
                open: false,
            },
            Shape::Union(vec![named("Ticket", false, None), Shape::open("any"), object]),
            Shape::open("someCall(a, b)"),
        ]
    }

    fn every_server_wire() -> Vec<ServerWire> {
        let mut out = Vec::new();
        for (i, shape) in every_shape().into_iter().enumerate() {
            let form = WireForm::ALL[i % WireForm::ALL.len()];
            let mut w = ServerWire::new(form, "gin.H{\"a\": x}", 10 + i as u32, shape);
            if i % 2 == 0 {
                w.status = Some(200 + i as u16);
            }
            if i % 3 == 0 {
                w.document = Some("ListItems".into());
            }
            out.push(w);
        }
        out
    }

    const HIT: &str = r#"{"method":"GET","path":"/api/orders","file":"src/a.ts","line":3,"col":4,"confidence":"strong","raw":"api/orders"}"#;

    // ---- tests ---------------------------------------------------------------

    #[test]
    fn model_schema_round_trip() {
        let s = full_schema();
        let p = s.to_payload();
        assert_eq!(ModelSchema::parse(&p), Some(s.clone()));
        let minimal = ModelSchema {
            format: WireFormat::Ts,
            fields: vec![ModelField::new("id", "string", true)],
            extends: vec![],
            open: false,
        };
        assert_eq!(ModelSchema::parse(&minimal.to_payload()), Some(minimal));
        assert_eq!(
            ModelSchema::parse(&ModelSchema::new(WireFormat::Dart).to_payload()),
            Some(ModelSchema::new(WireFormat::Dart))
        );
    }

    #[test]
    fn model_schema_bytes_are_pinned() {
        assert_eq!(
            full_schema().to_payload(),
            concat!(
                r#"{"extends":["Base","audit.Stamp"],"fields":["#,
                r#"{"array":true,"bind":"required","field":"Items","model":"models::Item","name":"items","required":true,"type":"[]*models.Item"},"#,
                r#"{"name":"note","required":false,"type":"*string"}],"format":"go","open":true}"#
            )
        );
        let ts = ModelSchema {
            format: WireFormat::Ts,
            fields: vec![ModelField::new("lines", "OrderLine[]", true)],
            extends: vec![],
            open: false,
        };
        assert_eq!(
            ts.to_payload(),
            r#"{"fields":[{"array":true,"name":"lines","required":true,"type":"OrderLine[]"}],"format":"ts"}"#
        );
    }

    #[test]
    fn model_field_member_equal_to_name_is_not_written() {
        let mut f = ModelField::new("id", "string", true);
        f.field = Some("id".into());
        let mut s = ModelSchema::new(WireFormat::Go);
        s.fields.push(f);
        let p = s.to_payload();
        assert!(!p.contains("\"field\""), "{p}");
        let back = ModelSchema::parse(&p).unwrap();
        assert_eq!(back.fields[0].field, None);
        assert_eq!(back.fields[0].member(), "id");
        assert_eq!(full_field().member(), "Items");
        assert_eq!(full_schema().field("note").map(|f| f.ty.as_str()), Some("*string"));
    }

    #[test]
    fn model_schema_reader_rejects_foreign_and_mistyped_payloads() {
        // A proto / OpenAPI SCHEMA_FIELDS payload is not a model schema.
        assert_eq!(
            ModelSchema::parse(r#"{"format":"proto","fields":[{"name":"id","type":"string","number":1}]}"#),
            None
        );
        assert_eq!(
            ModelSchema::parse(r#"{"format":"openapi","request":[{"name":"id","type":"string"}]}"#),
            None
        );
        // A mistyped known key rejects the payload; an unknown key is ignored.
        assert_eq!(
            ModelSchema::parse(r#"{"fields":[{"name":"a","required":"yes","type":"x"}],"format":"ts"}"#),
            None
        );
        assert_eq!(
            ModelSchema::parse(
                r#"{"fields":[{"bind":"optional","name":"a","required":true,"type":"x"}],"format":"go"}"#
            ),
            None
        );
        assert_eq!(ModelSchema::parse(r#"{"fields":[],"format":"ts","open":"yes"}"#), None);
        assert_eq!(ModelSchema::parse("not json"), None);
        let extra = r#"{"fields":[{"name":"a","required":true,"type":"x","zz":1}],"format":"ts","zz":2}"#;
        assert_eq!(ModelSchema::parse(extra).unwrap().fields[0].name, "a");
        // contract_fields' field reader (name / type / required) reads every field.
        let v: Value = serde_json::from_str(&full_schema().to_payload()).unwrap();
        for f in v["fields"].as_array().unwrap() {
            assert!(
                f["name"].is_string() && f["type"].is_string() && f["required"].is_boolean(),
                "{f}"
            );
        }
    }

    #[test]
    fn cells_carry_their_cell_types() {
        let s = full_schema();
        let c = s.cell();
        assert_eq!(c.kind, cell_type::SCHEMA_FIELDS);
        assert_eq!(ModelSchema::from_cell(&c), Some(s));
        let w = every_server_wire().remove(0);
        let wc = w.cell();
        assert_eq!(wc.kind, cell_type::MESSAGE_TYPE);
        assert_eq!(ServerWire::from_cell(&wc), Some(w));
        // Wrong kind, or a non-JSON payload, reads as nothing.
        assert_eq!(ModelSchema::from_cell(&wc), None);
        assert_eq!(ServerWire::from_cell(&c), None);
        let text = Cell {
            kind: cell_type::SCHEMA_FIELDS,
            payload: CellPayload::Text(full_schema().to_payload()),
        };
        assert_eq!(ModelSchema::from_cell(&text), None);
    }

    #[test]
    fn shape_round_trip_every_form() {
        for s in every_shape() {
            let j = s.to_json();
            assert_eq!(Shape::parse(&j), Some(s.clone()), "{j}");
            assert_eq!(Shape::from_value(&serde_json::from_str(&j).unwrap()), Some(s));
        }
        assert_eq!(Shape::parse(r#"{"open":false,"raw":"x"}"#), None);
        assert_eq!(Shape::parse(r#"{"array":"yes","type":"X"}"#), None);
        assert_eq!(Shape::parse(r#"{"nothing":1}"#), None);
    }

    #[test]
    fn shape_bytes_are_pinned() {
        let s = every_shape();
        assert_eq!(s[0].to_json(), r#"{"type":"Order"}"#);
        assert_eq!(
            s[1].to_json(),
            r#"{"array":true,"model":"models::Order","type":"models.Order"}"#
        );
        assert_eq!(
            s[2].to_json(),
            r#"{"array":true,"call":"self.store.Orders()","model":"models::Order"}"#
        );
        assert_eq!(
            s[4].to_json(),
            r#"{"array":true,"chain":"self.cfg.X","model":"cfg::X"}"#
        );
        assert_eq!(
            s[5].to_json(),
            concat!(
                r#"{"fields":[{"name":"offers","required":true,"shape":{"array":true,"model":"src::models::Offer","type":"Offer"}},"#,
                r#"{"bind":"required","field":"count","name":"total","required":false,"shape":{"chain":"self.cfg.Total"}}],"open":true}"#
            )
        );
        assert_eq!(s[6].to_json(), r#"{"fields":[]}"#);
        assert_eq!(s[8].to_json(), r#"{"open":true,"raw":"someCall(a, b)"}"#);
        assert!(
            s[7].to_json()
                .starts_with(r#"{"union":[{"type":"Ticket"},{"open":true,"raw":"any"},"#)
        );
    }

    #[test]
    fn shape_field_alias_round_trip() {
        // A GraphQL `top: products { .. }`: the response key is the alias.
        let alias = ShapeField {
            name: "top".into(),
            field: Some("products".into()),
            required: false,
            bind_required: false,
            shape: Shape::Object {
                fields: vec![ShapeField::new("sku", false, named("String", false, None))],
                open: false,
            },
        };
        let s = Shape::Object {
            fields: vec![alias.clone()],
            open: false,
        };
        let j = s.to_json();
        assert_eq!(
            j,
            r#"{"fields":[{"field":"products","name":"top","required":false,"shape":{"fields":[{"name":"sku","required":false,"shape":{"type":"String"}}]}}]}"#
        );
        let Some(Shape::Object { fields, .. }) = Shape::parse(&j) else {
            panic!("{j}")
        };
        assert_eq!(fields, vec![alias]);
        // An alias equal to its field is not written.
        let same = Shape::Object {
            fields: vec![ShapeField {
                field: Some("sku".into()),
                ..ShapeField::new("sku", true, Shape::named("String!"))
            }],
            open: false,
        };
        assert!(!same.to_json().contains("\"field\""));
    }

    #[test]
    fn shape_helpers() {
        let mut s = Shape::named("[]*models.Order");
        assert_eq!(s, named("models.Order", true, None));
        assert!(s.is_array());
        assert_eq!(s.simple_name(), "Order");
        assert!(s.set_model("models::Order"));
        assert_eq!(s.model(), Some("models::Order"));
        let mut obj = Shape::Object {
            fields: vec![],
            open: false,
        };
        assert!(!obj.set_model("x"));
        assert!(!obj.is_open());
        assert!(Shape::Union(vec![named("A", false, None), Shape::open("any")]).is_open());
        assert!(!Shape::Union(vec![named("A", false, None)]).is_open());
        assert_eq!(named("src::m::Order", false, None).simple_name(), "Order");
        assert_eq!(named("models.Page[T]", false, None).simple_name(), "Page");
        assert_eq!(named("Api.Envelope<Order>", false, None).simple_name(), "Envelope");
        assert_eq!(
            Shape::Call {
                chain: "f()".into(),
                array: false,
                model: None
            }
            .simple_name(),
            ""
        );
    }

    #[test]
    fn raw_is_clipped_to_120_chars_on_a_char_boundary() {
        let long: String = "é".repeat(200);
        let Shape::Open { raw } = Shape::open(&long) else {
            panic!()
        };
        assert_eq!(raw.chars().count(), RAW_MAX);
        assert_eq!(clip_raw("short"), "short");
        let w = ServerWire::new(WireForm::JsonResponse, &long, 1, Shape::open(&long));
        assert_eq!(w.raw.chars().count(), RAW_MAX);
        // The writers clip too, so a hand-built value cannot exceed it.
        let hand = Shape::Open { raw: long.clone() };
        let Some(Shape::Open { raw }) = Shape::parse(&hand.to_json()) else {
            panic!()
        };
        assert_eq!(raw.chars().count(), RAW_MAX);
    }

    #[test]
    fn server_wire_round_trip_and_leading_keys() {
        for w in every_server_wire() {
            let p = w.to_payload();
            assert_eq!(ServerWire::parse(&p), Some(w.clone()), "{p}");
        }
        let mut w = ServerWire::new(WireForm::JsonResponse, "orders", 42, Shape::named("[]models.Order"));
        w.status = Some(200);
        assert_eq!(w.ty, "Order");
        assert_eq!(
            w.to_payload(),
            r#"{"type":"Order","raw":"orders","form":"json_response","window":"site","line":42,"status":200,"shape":{"array":true,"type":"models.Order"}}"#
        );
        assert!(w.known_2xx());
        w.status = Some(404);
        assert!(!w.known_2xx());
        // Status absent (unknown) and a document.
        let mut g = ServerWire::new(
            WireForm::GraphqlResult,
            "client.query<ItemsResult>",
            7,
            Shape::named("ItemsResult"),
        );
        g.document = Some("LIST_ITEMS".into());
        let p = g.to_payload();
        assert_eq!(
            p,
            r#"{"type":"ItemsResult","raw":"client.query<ItemsResult>","form":"graphql_result","window":"site","line":7,"document":"LIST_ITEMS","shape":{"type":"ItemsResult"}}"#
        );
        let back = ServerWire::parse(&p).unwrap();
        assert_eq!(back.status, None);
        assert!(!back.known_2xx());
        assert_eq!(back.document.as_deref(), Some("LIST_ITEMS"));
    }

    #[test]
    fn server_wire_reader_rejects_queue_cells() {
        // A12.1's queue MESSAGE_TYPE cell (queues.rs message_type_cell).
        assert_eq!(
            ServerWire::parse(r#"{"type":"OrderCreated","raw":"pb.OrderCreated","form":"generic","window":"near"}"#),
            None
        );
        assert_eq!(
            ServerWire::parse(r#"{"type":"X","raw":"X{}","form":"struct_literal","window":"file"}"#),
            None
        );
        // A site window with an unknown form, a status out of range, or no shape.
        let ok = r#"{"type":"","raw":"v","form":"json_bind","window":"site","line":1,"shape":{"open":true,"raw":"v"}}"#;
        assert!(ServerWire::parse(ok).is_some());
        assert_eq!(ServerWire::parse(&ok.replace("json_bind", "generic")), None);
        assert_eq!(
            ServerWire::parse(&ok.replace("\"line\":1", "\"line\":1,\"status\":70000")),
            None
        );
        assert_eq!(
            ServerWire::parse(r#"{"type":"","raw":"v","form":"json_bind","window":"site","line":1}"#),
            None
        );
    }

    #[test]
    fn escapes_go_through_serde_json() {
        let nasty = "a\"b\\c\nd\te\u{1}f\u{7f}é😀</script>";
        let mut s = ModelSchema::new(WireFormat::Ts);
        s.fields.push(ModelField {
            field: Some(format!("{nasty}_src")),
            ..ModelField::new(nasty, nasty, true)
        });
        s.extends.push(nasty.into());
        let p = s.to_payload();
        assert!(p.contains(r#"a\"b\\c\nd\te\u0001f"#), "{p}");
        assert_eq!(ModelSchema::parse(&p), Some(s));
        let w = ServerWire::new(WireForm::JsonResponse, nasty, 1, Shape::open(nasty));
        assert_eq!(ServerWire::parse(&w.to_payload()), Some(w.clone()));
        assert!(stable_sorted(&Shape::open(nasty).to_json()));
        assert!(stable_fixed_top(&w.to_payload()));
    }

    #[test]
    fn append_hit_fields_keeps_the_old_bytes_as_a_prefix() {
        let resp = named("Order", true, None);
        let req = Shape::Object {
            fields: vec![ShapeField::new("status", true, Shape::named("'cancelled'"))],
            open: false,
        };
        // No shape: unchanged.
        assert_eq!(append_hit_fields(HIT, None, None), HIT);
        let stem = &HIT[..HIT.len() - 1];
        for (r, q) in [(Some(&resp), None), (None, Some(&req)), (Some(&resp), Some(&req))] {
            let out = append_hit_fields(HIT, r, q);
            assert!(out.starts_with(stem) && out.ends_with('}'), "{out}");
            assert_eq!(hit_shapes(&out), (r.cloned(), q.cloned()));
            assert!(stable_fixed_top(&out), "{out}");
        }
        assert_eq!(
            append_hit_fields(HIT, Some(&resp), Some(&req)),
            format!(r#"{stem},"response":{},"request":{}}}"#, resp.to_json(), req.to_json())
        );
        // An empty object takes no leading comma; a non-object is untouched.
        assert_eq!(
            append_hit_fields("{}", Some(&resp), None),
            r#"{"response":{"array":true,"type":"Order"}}"#
        );
        assert_eq!(append_hit_fields("[1]", Some(&resp), None), "[1]");
        assert_eq!(hit_shapes(HIT), (None, None));
        assert_eq!(hit_shapes("not json"), (None, None));
    }

    #[test]
    fn json_kind_ts() {
        let k = |t| json_kind(WireFormat::Ts, t);
        assert_eq!(k("string"), JsonKind::String);
        assert_eq!(k("'open' | 'closed'"), JsonKind::String);
        assert_eq!(k("`order-${string}`"), JsonKind::String);
        assert_eq!(k("number"), JsonKind::Number);
        assert_eq!(k("bigint"), JsonKind::Number);
        assert_eq!(k("1 | 2 | 3"), JsonKind::Number);
        assert_eq!(k("-1.5"), JsonKind::Number);
        assert_eq!(k("Infinity"), JsonKind::Unknown("Infinity".into()));
        assert_eq!(k("NaN"), JsonKind::Unknown("NaN".into()));
        assert_eq!(k("boolean"), JsonKind::Boolean);
        assert_eq!(k("true"), JsonKind::Boolean);
        assert_eq!(k("Order[]"), JsonKind::Array);
        assert_eq!(k("Array<Order>"), JsonKind::Array);
        assert_eq!(k("ReadonlyArray<string>"), JsonKind::Array);
        assert_eq!(k("readonly string[]"), JsonKind::Array);
        assert_eq!(k("[string, number]"), JsonKind::Array);
        assert_eq!(k("(Order | null)[]"), JsonKind::Array);
        assert_eq!(k("{ id: string; n?: number }"), JsonKind::Object);
        assert_eq!(k("Record<string, number>"), JsonKind::Object);
        assert_eq!(k("any"), JsonKind::Any);
        assert_eq!(k("unknown"), JsonKind::Any);
        assert_eq!(k("object"), JsonKind::Any);
        assert_eq!(k("Date"), JsonKind::Any);
        assert_eq!(k("null"), JsonKind::Null);
        assert_eq!(k("undefined"), JsonKind::Null);
        assert_eq!(k("string | null"), JsonKind::String);
        assert_eq!(k("string | undefined"), JsonKind::String);
        assert_eq!(k("string | number"), JsonKind::Any);
        assert_eq!(k("A & B"), JsonKind::Any);
        assert_eq!(k("Order"), JsonKind::Unknown("Order".into()));
        assert_eq!(k("Ticket | null"), JsonKind::Unknown("Ticket".into()));
        assert_eq!(k("Models.Order"), JsonKind::Unknown("Models.Order".into()));
        assert_eq!(k("Partial<Order>"), JsonKind::Unknown("Partial<Order>".into()));
        assert_eq!(
            k("(a: string) => void"),
            JsonKind::Unknown("(a: string) => void".into())
        );
    }

    #[test]
    fn json_kind_go() {
        let k = |t| json_kind(WireFormat::Go, t);
        assert_eq!(k("string"), JsonKind::String);
        assert_eq!(k("time.Time"), JsonKind::String);
        assert_eq!(k("*time.Time"), JsonKind::String);
        assert_eq!(k("uuid.UUID"), JsonKind::String);
        assert_eq!(k("[]byte"), JsonKind::String);
        for n in [
            "int",
            "int8",
            "int64",
            "uint",
            "uint32",
            "uintptr",
            "float32",
            "float64",
            "byte",
            "rune",
            "json.Number",
        ] {
            assert_eq!(k(n), JsonKind::Number, "{n}");
        }
        assert_eq!(k("integer"), JsonKind::Unknown("integer".into()));
        assert_eq!(k("bool"), JsonKind::Boolean);
        assert_eq!(k("*bool"), JsonKind::Boolean);
        assert_eq!(k("[]Order"), JsonKind::Array);
        assert_eq!(k("[]*models.Order"), JsonKind::Array);
        assert_eq!(k("[3]int"), JsonKind::Array);
        assert_eq!(k("map[string]any"), JsonKind::Object);
        assert_eq!(k("struct{ A int `json:\"a\"` }"), JsonKind::Object);
        assert_eq!(k("any"), JsonKind::Any);
        assert_eq!(k("interface{}"), JsonKind::Any);
        assert_eq!(k("json.RawMessage"), JsonKind::Any);
        assert_eq!(k("*models.Order"), JsonKind::Unknown("models.Order".into()));
        assert_eq!(k("sql.NullString"), JsonKind::Unknown("sql.NullString".into()));
        assert_eq!(k("Page[T]"), JsonKind::Unknown("Page[T]".into()));
    }

    #[test]
    fn json_kind_dart() {
        let k = |t| json_kind(WireFormat::Dart, t);
        assert_eq!(k("String"), JsonKind::String);
        assert_eq!(k("String?"), JsonKind::String);
        assert_eq!(k("int"), JsonKind::Number);
        assert_eq!(k("double"), JsonKind::Number);
        assert_eq!(k("num?"), JsonKind::Number);
        assert_eq!(k("bool"), JsonKind::Boolean);
        assert_eq!(k("List<Member>"), JsonKind::Array);
        assert_eq!(k("List<Member>?"), JsonKind::Array);
        assert_eq!(k("Iterable<int>"), JsonKind::Array);
        assert_eq!(k("Map<String, dynamic>"), JsonKind::Object);
        assert_eq!(k("dynamic"), JsonKind::Any);
        assert_eq!(k("Object?"), JsonKind::Any);
        assert_eq!(k("DateTime"), JsonKind::Any);
        assert_eq!(k("Null"), JsonKind::Null);
        assert_eq!(k("Member?"), JsonKind::Unknown("Member".into()));
    }

    #[test]
    fn json_kind_graphql() {
        let k = |t| json_kind(WireFormat::Graphql, t);
        assert_eq!(k("ID!"), JsonKind::String);
        assert_eq!(k("String"), JsonKind::String);
        assert_eq!(k("Int"), JsonKind::Number);
        assert_eq!(k("Float!"), JsonKind::Number);
        assert_eq!(k("Boolean!"), JsonKind::Boolean);
        assert_eq!(k("[Item!]!"), JsonKind::Array);
        assert_eq!(k("[Int]"), JsonKind::Array);
        assert_eq!(k("Item!"), JsonKind::Unknown("Item".into()));
        assert_eq!(k("DateTime"), JsonKind::Unknown("DateTime".into()));
    }

    #[test]
    fn json_schema_kind_reads_contract_types() {
        assert_eq!(json_schema_kind("string"), JsonKind::String);
        assert_eq!(json_schema_kind("string(uuid)"), JsonKind::String);
        assert_eq!(json_schema_kind("integer(int32)"), JsonKind::Number);
        assert_eq!(json_schema_kind("number|null"), JsonKind::Number);
        assert_eq!(json_schema_kind("boolean"), JsonKind::Boolean);
        assert_eq!(json_schema_kind("array"), JsonKind::Array);
        assert_eq!(json_schema_kind("object"), JsonKind::Object);
        assert_eq!(json_schema_kind("null"), JsonKind::Null);
        assert_eq!(json_schema_kind("any"), JsonKind::Any);
        assert_eq!(json_schema_kind("oneOf<Card|string>"), JsonKind::Any);
        assert_eq!(json_schema_kind("string|integer"), JsonKind::Any);
        assert_eq!(json_schema_kind("Order"), JsonKind::Unknown("Order".into()));
        assert_eq!(JsonKind::Number.as_str(), "number");
        assert_eq!(JsonKind::Unknown("X".into()).as_str(), "unknown");
    }

    #[test]
    fn element_type_cases() {
        let e = |t: &str| element_type(t);
        let s = |b: &str, a: bool| (b.to_string(), a);
        assert_eq!(e("[]*models.Order"), s("models.Order", true));
        assert_eq!(e("Order[]"), s("Order", true));
        assert_eq!(e("Array<Order>"), s("Order", true));
        assert_eq!(e("ReadonlyArray<Order>"), s("Order", true));
        assert_eq!(e("readonly Order[]"), s("Order", true));
        assert_eq!(e("List<Member>?"), s("Member", true));
        assert_eq!(e("Iterable<Member>"), s("Member", true));
        assert_eq!(e("[Item!]!"), s("Item", true));
        assert_eq!(e("Ticket | null"), s("Ticket", false));
        assert_eq!(e("undefined | Ticket"), s("Ticket", false));
        assert_eq!(e("(Order | null)[]"), s("Order", true));
        assert_eq!(e("*models.Order"), s("models.Order", false));
        assert_eq!(e("[5]Item"), s("Item", true));
        assert_eq!(e("*[]Item"), s("Item", true));
        assert_eq!(e("Member?"), s("Member", false));
        assert_eq!(e("Order"), s("Order", false));
        // One array layer only; a tuple, a mixed union and a map stay whole.
        assert_eq!(e("Order[][]"), s("Order[]", true));
        assert_eq!(e("[[Item]]"), s("[Item]", true));
        assert_eq!(e("[string, number]"), s("[string, number]", false));
        assert_eq!(e("Order | Ticket"), s("Order | Ticket", false));
        assert_eq!(e("map[string]Item"), s("map[string]Item", false));
        assert_eq!(e("[]byte"), s("[]byte", false));
        assert_eq!(e("*[]uint8"), s("[]uint8", false));
        assert_eq!(e("[][]byte"), s("[]byte", true));
        assert_eq!(e("Array<Map<string, X>>"), s("Map<string, X>", true));
        assert_eq!(e("  "), s("", false));
    }

    /// `array` and the kind agree: a type [`element_type`] peels an array
    /// layer off is an array in its format's [`json_kind`] table.
    #[test]
    fn array_layers_are_array_kinds() {
        let cases: &[(WireFormat, &[&str])] = &[
            (
                WireFormat::Ts,
                &[
                    "Order[]",
                    "Array<Order>",
                    "ReadonlyArray<X>",
                    "readonly X[]",
                    "(A | null)[]",
                    "Order[] | null",
                    "[string, number]",
                    "string",
                    "'a' | 'b'",
                ],
            ),
            (
                WireFormat::Go,
                &[
                    "[]Order",
                    "[]*models.Order",
                    "[4]int",
                    "*[]Item",
                    "[]byte",
                    "map[string][]X",
                    "string",
                ],
            ),
            (
                WireFormat::Dart,
                &[
                    "List<Member>",
                    "List<Member>?",
                    "Iterable<int>",
                    "Map<String, dynamic>",
                    "String?",
                ],
            ),
            (WireFormat::Graphql, &["[Item!]!", "[Int]", "Item!", "ID"]),
        ];
        for (format, types) in cases {
            for t in *types {
                if element_type(t).1 {
                    assert_eq!(json_kind(*format, t), JsonKind::Array, "{} {t}", format.as_str());
                }
            }
        }
        assert!(!ModelField::new("raw", "[]byte", true).array);
        assert!(ModelField::new("items", "[]Item", true).array);
    }

    #[test]
    fn slice_result_fact_round_trips() {
        // DT.1e records it, DT.1f reads it; appended after GoPackageName.
        let f = crate::NavFact::SliceResult {
            elem: "models.Order".into(),
        };
        let j = serde_json::to_string(&f).unwrap();
        assert_eq!(j, r#"{"SliceResult":{"elem":"models.Order"}}"#);
        assert_eq!(serde_json::from_str::<crate::NavFact>(&j).unwrap(), f);
    }

    #[test]
    fn formats_and_forms_name_themselves() {
        for f in WireFormat::ALL {
            assert_eq!(WireFormat::parse(f.as_str()), Some(f));
        }
        for f in WireForm::ALL {
            assert_eq!(WireForm::parse(f.as_str()), Some(f));
        }
        assert_eq!(WireFormat::parse("proto"), None);
        assert_eq!(WireForm::parse("generic"), None);
    }

    /// The sorted-key property over every writer output. Prints
    /// `[wire] payloads=<n> stable=<n>` (the fired_on marker): `stable`
    /// counts the outputs whose round trip is byte-equal - a Value round trip
    /// for the sorted writers, the endpoint fold's top-level-order round
    /// trip for ServerWire and spliced hits.
    #[test]
    fn every_writer_output_is_round_trip_stable() {
        let mut sorted_out: Vec<String> = Vec::new();
        let mut fixed_top: Vec<String> = Vec::new();
        sorted_out.push(full_schema().to_payload());
        sorted_out.push(ModelSchema::new(WireFormat::Graphql).to_payload());
        sorted_out.extend(every_shape().iter().map(Shape::to_json));
        fixed_top.extend(every_server_wire().iter().map(ServerWire::to_payload));
        for s in every_shape() {
            fixed_top.push(append_hit_fields(HIT, Some(&s), Some(&s)));
        }
        let mut stable = 0;
        for p in &sorted_out {
            if stable_sorted(p) {
                stable += 1;
            } else {
                eprintln!("[wire] unstable sorted payload {p}");
            }
        }
        for p in &fixed_top {
            if stable_fixed_top(p) {
                stable += 1;
            } else {
                eprintln!("[wire] unstable fixed-top payload {p}");
            }
        }
        let payloads = sorted_out.len() + fixed_top.len();
        eprintln!("[wire] payloads={payloads} stable={stable}");
        assert_eq!(stable, payloads);
        // The check bites: an unsorted object is caught whatever serde_json's
        // features are.
        assert!(!stable_sorted(r#"{"type":"X","array":true}"#));
    }
}
