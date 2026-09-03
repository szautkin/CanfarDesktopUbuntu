//! Tool abstractions: the boundary between the MCP server (transport/dispatch)
//! and the concrete tool catalog. Ported from `Mcp/Tools/*` + `Mcp/McpToolRouter.cs`.
//!
//! The server holds an `Arc<dyn ToolRouter>` and only knows this interface, so the
//! wire layer is testable with a `NullRouter` while the real catalog binds to
//! `AppServices` separately.

use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

pub mod ai_compute;
pub mod aiguide_ext;
pub mod aliases;
pub mod apps;
pub mod caom2_vizier;
pub mod catalog;
pub mod cube;
pub mod fits;
pub mod imagediscovery;
pub mod notebook;
pub mod proposals;
pub mod read;
pub mod registry;
pub mod research;
pub mod router;
pub mod search_ui;
pub mod sessions;
pub mod viewstate;
pub mod vospace;
pub mod workflows;
pub mod write;

// ─────────────────────────────────────────────────────────────────────────────
// Shared argument accessors
//
// Tool schemas declare arguments in camelCase, matching the reference (whose
// serializer is configured `PropertyNamingPolicy = CamelCase`). The reference
// also deserializes case-insensitively, so an agent that sends `publisher_id`
// where the schema says `publisherId` still works there — and must here too.
// These helpers therefore try the key as given, then its snake_case spelling.
// ─────────────────────────────────────────────────────────────────────────────

/// `some_key` → `someKey`. Leaves an already-camelCase key untouched.
pub(crate) fn camel_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper_next = false;
    for c in key.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `someKey` → `some_key`. Leaves an already-snake_case key untouched.
pub(crate) fn snake_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Read the style arguments a mark tool accepts, over a mark's current style.
///
/// One reader for all four tools — `annotate_fits`, `annotate_cube`,
/// `update_annotation` and anything that follows — because four copies of
/// "parse a hex colour, clamp a font size" is four chances for them to disagree
/// about what `#gg0000` means.
///
/// Returns `Ok(None)` when nothing about style was asked for, so a caller can
/// tell "leave it alone" from "set it to the defaults" — an
/// `update_annotation` that reset a mark's colour because the call happened not
/// to mention it would be worse than one that refused.
pub fn mark_style_args(
    args: &Value,
    current: crate::models::annotation::MarkStyle,
) -> Result<Option<crate::models::annotation::MarkStyle>, String> {
    let mut style = current;
    let mut touched = false;

    if let Some(v) = arg(args, "colour").or_else(|| arg(args, "color")) {
        let text = v
            .as_str()
            .ok_or_else(|| "colour must be a string like \"#ff8800\"".to_string())?;
        style.colour = crate::models::annotation::MarkStyle::colour_from_hex(text)
            .ok_or_else(|| format!("'{text}' is not a colour — use #rrggbb"))?;
        touched = true;
    }
    if let Some(v) = arg(args, "fontSize").and_then(|v| v.as_f64()) {
        style.font_size = v;
        touched = true;
    }
    if let Some(v) = arg(args, "bold").and_then(|v| v.as_bool()) {
        style.bold = v;
        touched = true;
    }
    if let Some(v) = arg(args, "stroke").and_then(|v| v.as_f64()) {
        style.stroke = v;
        touched = true;
    }
    // Clamped here rather than at draw time as well: what comes back from
    // `list_*_annotations` should be what will actually be drawn, so an agent
    // that asks for a 500px label can see it was given 72.
    Ok(touched.then(|| style.sane()))
}

/// The style fields every mark tool accepts, for its input schema.
///
/// Declared once so the four tools cannot advertise different spellings or
/// different ranges from the ones `mark_style_args` enforces.
pub fn mark_style_schema() -> Vec<(&'static str, Value)> {
    vec![
        (
            "colour",
            serde_json::json!({
                "type": "string",
                "description": "Ink, as #rrggbb. Also accepted as `color`. A mark keeps this in the file and in an exported figure; picking it out on screen still highlights it, which is session state and never reaches an export."
            }),
        ),
        (
            "fontSize",
            serde_json::json!({
                "type": "number", "minimum": 6, "maximum": 72,
                "description": "Label size in device pixels, not scaled by zoom (an export scales it). 11 by default."
            }),
        ),
        (
            "bold",
            serde_json::json!({ "type": "boolean", "description": "Draw the label bold." }),
        ),
        (
            "stroke",
            serde_json::json!({
                "type": "number", "minimum": 0.5, "maximum": 20,
                "description": "Outline width in device pixels, not scaled by zoom. 1 by default."
            }),
        ),
    ]
}

/// Look up `key` in an arguments object, accepting either naming style.
///
/// Tools ask for the canonical (camelCase) name; a caller that sent snake_case
/// still resolves. Returns `None` when neither spelling is present.
pub fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(v) = args.get(key) {
        return Some(v);
    }
    let alt = if key.contains('_') {
        camel_case(key)
    } else {
        snake_case(key)
    };
    args.get(&alt)
}

/// A trimmed string argument, or `""` when absent or not a string.
pub fn str_arg(args: &Value, key: &str) -> String {
    arg(args, key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// A trimmed string argument, or `None` when absent, not a string, or blank.
pub fn opt_str_arg(args: &Value, key: &str) -> Option<String> {
    arg(args, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// An optional boolean argument (`None` when absent or not a boolean).
pub fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    arg(args, key).and_then(Value::as_bool)
}

/// A boolean argument defaulting to `false`.
pub fn bool_arg(args: &Value, key: &str) -> bool {
    opt_bool(args, key).unwrap_or(false)
}

/// An optional unsigned argument.
pub fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    arg(args, key).and_then(Value::as_u64)
}

/// An optional unsigned argument narrowed to `u32`.
pub fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    opt_u64(args, key).map(|n| n as u32)
}

/// A number however it is spelled: a JSON number, or a string holding one.
///
/// Agents routinely send `"42"` where a schema says `number`, and refusing that
/// is pedantry rather than safety. The one place that decides what counts as a
/// number, so the lenient readers and the refusing ones agree.
fn as_number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// A numeric argument, `None` when absent OR unreadable.
///
/// Prefer [`opt_number`] in anything that can refuse: this cannot tell a caller
/// that their value was thrown away.
pub fn num_arg(args: &Value, key: &str) -> Option<f64> {
    arg(args, key).and_then(as_number)
}

/// A numeric argument that REFUSES what it cannot read.
pub fn opt_number(args: &Value, key: &str) -> Result<Option<f64>, String> {
    let Some(v) = arg(args, key) else {
        return Ok(None);
    };
    as_number(v)
        .map(Some)
        .ok_or_else(|| format!("{key} takes a number, got {}", describe_json(v)))
}

/// A whole-number argument that REFUSES what it cannot read.
///
/// `opt_u64` answers `None` both for "absent" and for "present but not a
/// number", so a caller cannot tell them apart and treats a wrong type as not
/// supplied. `set_search_results_view {"rowsPerPage": "abc"}` therefore
/// reported success and changed nothing — the same "answered that it was
/// asked, not what happened" this codebase keeps having to fix.
///
/// `"42"` and `42.0` are both whole numbers: the first is how agents spell one,
/// the second is what a language without an integer type produces for `500 / 2`.
/// `12.5` is not — half a row is a request nobody meant.
pub fn opt_whole(args: &Value, key: &str) -> Result<Option<u64>, String> {
    let Some(v) = arg(args, key) else {
        return Ok(None);
    };
    // Ahead of `as_number`, because past 2^53 an f64 no longer holds every
    // whole number and this is the form that keeps them all.
    if let Some(n) = v.as_u64() {
        return Ok(Some(n));
    }
    as_number(v)
        .filter(|n| n.fract() == 0.0 && *n >= 0.0)
        .map(|n| Some(n as u64))
        .ok_or_else(|| format!("{key} takes a whole number, got {}", describe_json(v)))
}

/// A JSON value in the words of a refusal — its shape, not its contents.
fn describe_json(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => format!("the boolean {b}"),
        Value::Number(n) => format!("{n}"),
        Value::String(s) => format!("the text {s:?}"),
        Value::Array(_) => "an array".to_string(),
        Value::Object(_) => "an object".to_string(),
    }
}

#[cfg(test)]
mod whole_number_tests {
    use super::*;
    use serde_json::json;

    /// Absent is not the same as unreadable.
    ///
    /// `opt_u64` answered `None` for both, so a wrong type was treated as not
    /// supplied: `{"rowsPerPage": "abc"}` reported success and changed nothing.
    #[test]
    fn a_value_that_is_not_a_number_is_refused_rather_than_ignored() {
        let absent = json!({});
        assert_eq!(opt_whole(&absent, "n"), Ok(None));
        for bad in [
            json!("abc"),
            json!(12.5),
            json!(null),
            json!(true),
            json!([1]),
        ] {
            let args = json!({ "n": bad });
            let out = opt_whole(&args, "n");
            assert!(out.is_err(), "{bad} was accepted: {out:?}");
            assert!(
                out.unwrap_err().contains("whole number"),
                "the refusal should say what it wanted"
            );
        }
    }

    /// A whole number is a whole number however it arrives.
    ///
    /// `"42"` is how an agent spells one; `250.0` is what a language without an
    /// integer type produces for `500 / 2`. Refusing either is pedantry.
    #[test]
    fn a_whole_number_is_accepted_however_it_is_spelled() {
        for (spelled, expected) in [
            (json!(3), 3),
            (json!("42"), 42),
            (json!(" 7 "), 7),
            (json!(250.0), 250),
        ] {
            assert_eq!(
                opt_whole(&json!({ "n": spelled.clone() }), "n"),
                Ok(Some(expected)),
                "{spelled} should read as {expected}"
            );
        }
    }

    /// The lenient and the refusing readers agree on what a number is.
    ///
    /// They share `as_number` so that they cannot drift: a value one accepts
    /// and the other refuses would mean a tool validating with one and applying
    /// with the other silently dropped it.
    #[test]
    fn the_lenient_and_refusing_readers_agree() {
        for spelled in [
            json!(1),
            json!(-2.5),
            json!("42"),
            json!("abc"),
            json!(null),
        ] {
            let args = json!({ "n": spelled.clone() });
            assert_eq!(
                num_arg(&args, "n"),
                opt_number(&args, "n").ok().flatten(),
                "{spelled} read differently by the two readers"
            );
        }
    }
}

/// An optional array-of-strings argument.
///
/// Distinguishes "omitted" (`None`) from "explicitly empty" (`Some(vec![])`) —
/// `set_vospace_acl` relies on that difference to tell "leave this dimension
/// unchanged" from "revoke every group in it".
pub fn opt_str_array(args: &Value, key: &str) -> Option<Vec<String>> {
    arg(args, key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()
    })
}

/// Round a float to `decimals` places for the wire.
///
/// Derived quantities (GB, percentages) are computed in full precision and
/// rounded only on output, matching the reference's `Math.Round(x, n)` at each
/// tool boundary. Without it a quota reads `23.847263918273544` — noise that
/// costs tokens and invites an agent to quote a precision the number does not
/// have.
///
/// Non-finite inputs pass through untouched: `NaN`/`inf` cannot be rounded
/// meaningfully, and `json!` renders them as `null`, which is the honest answer.
pub fn round_dp(value: f64, decimals: u32) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let factor = 10_f64.powi(decimals as i32);
    (value * factor).round() / factor
}

/// Stamp the reference's tab-switch outcome onto a viewer's state payload.
///
/// The reference answers `switch_fits_tab` / `switch_cube_tab` with
/// `{switched, index, count, activeName, message}` and nothing else; our viewers
/// answer with the newly-active tab's full state, which saves the agent a
/// follow-up read. Both go out: the outcome keys are the contract, the state is
/// the useful part. None of the four collides with a viewer state key.
///
/// Shared by the FITS and cube hosts so the two cannot describe the same event
/// differently.
pub fn with_tab_switch_outcome(
    mut state: Value,
    index: usize,
    count: usize,
    active_name: &str,
) -> Value {
    state["switched"] = Value::Bool(true);
    state["index"] = json!(index);
    state["count"] = json!(count);
    state["activeName"] = json!(active_name);
    // The reference carries a nullable note for the refusal path. A refusal here
    // is a typed error rather than a payload, so it is always null on success —
    // present so the key set does not change shape between apps.
    state["message"] = Value::Null;
    state
}

/// Apply an approved proposal by dispatching to the family that owns its `kind`.
/// Each service-backed family exposes `apply(...) -> Option<Result<..>>`; the base
/// write tools handle the rest (and error on a truly unknown kind).
pub async fn apply_any(
    services: &crate::state::AppServices,
    proposal: &proposals::PendingProposal,
) -> Result<String, String> {
    if let Some(r) = vospace::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = research::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = sessions::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = workflows::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = aiguide_ext::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = registry::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = imagediscovery::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = caom2_vizier::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = ai_compute::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = search_ui::apply(services, proposal).await {
        return r;
    }
    write::apply(services, proposal).await
}

/// All service-backed family descriptors (chained into the router's manifest).
pub fn family_descriptors() -> Vec<ToolDescriptor> {
    let mut v = Vec::new();
    v.extend(apps::descriptors());
    v.extend(vospace::descriptors());
    v.extend(research::descriptors());
    v.extend(sessions::descriptors());
    v.extend(workflows::descriptors());
    v.extend(aiguide_ext::descriptors());
    v.extend(viewstate::descriptors());
    v.extend(cube::descriptors());
    v.extend(notebook::descriptors());
    v.extend(fits::descriptors());
    v.extend(imagediscovery::descriptors());
    v.extend(registry::descriptors());
    v.extend(caom2_vizier::descriptors());
    v.extend(search_ui::descriptors());
    v.extend(ai_compute::descriptors());
    v
}

/// Try each family's dispatch; `None` if no family owns `name`.
pub async fn family_dispatch(
    name: &str,
    services: &crate::state::AppServices,
    args: &serde_json::Value,
    proposals: &std::sync::Arc<proposals::InMemoryProposalStore>,
) -> Option<ToolResult> {
    if let Some(r) = vospace::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = research::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = sessions::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = workflows::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = aiguide_ext::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = viewstate::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = cube::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = notebook::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = fits::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = registry::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = imagediscovery::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = caom2_vizier::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = search_ui::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = ai_compute::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    None
}

/// Read tools are agent-safe (no side effects); write tools return proposals and
/// must NEVER auto-apply destructive changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbClass {
    Read,
    Write,
}

/// A tool's public descriptor as advertised in `tools/list`.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's arguments.
    pub input_schema: Value,
    pub verb: VerbClass,
    /// Whether an external (agent) caller may invoke this tool directly.
    pub agent_safe: bool,
}

/// Where a tool call originates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOrigin {
    /// An external MCP client (an AI agent). Subject to the agent-safe gate.
    External(String),
    /// The app itself (user-initiated). May reach user-only tools.
    ///
    /// Never constructed today — the UI calls its services directly rather than
    /// dispatching through the router. It stays because `is_external` is a
    /// SECURITY check, and a single-variant enum would make it read as always
    /// true: the gate would look like a formality instead of a decision.
    #[allow(dead_code)]
    Internal,
}

/// Per-call context threaded to a tool.
pub struct ToolContext {
    pub origin: OperationOrigin,
    /// A unique id for this call (audit + proposal correlation).
    pub request_id: String,
}

impl ToolContext {
    pub fn for_external(client_id: String, request_id: String) -> Self {
        ToolContext {
            origin: OperationOrigin::External(client_id),
            request_id,
        }
    }

    pub fn is_external(&self) -> bool {
        matches!(self.origin, OperationOrigin::External(_))
    }

    pub fn client_label(&self) -> &str {
        match &self.origin {
            OperationOrigin::External(c) => c,
            OperationOrigin::Internal => "internal",
        }
    }
}

/// The outcome of a tool call.
pub enum ToolResult {
    /// Structured JSON payload (serialized as text content on the wire).
    Data(Value),
    /// Plain text content.
    Text(String),
    /// A queued write proposal awaiting user approval.
    Proposed(proposals::PendingProposal),
    /// A base64 image with an optional caption.
    Image {
        data_base64: String,
        mime: String,
        caption: Option<String>,
        /// What the picture is OF, as data: the view it was captured from and
        /// the transform between it and the raster.
        ///
        /// An agent that can see a viewer will be asked to draw on it — to ring
        /// a source, to point at an artefact. It can only express "here" in a
        /// frame the app shares, so a capture that arrives as pixels alone
        /// makes that impossible without capturing it again. The picture is for
        /// the eye; this is for the arithmetic.
        payload: Option<serde_json::Value>,
    },
    /// The call failed; `reason` is a human-readable message (maps to isError).
    Failed(String),
}

/// A dyn-compatible async tool router.
pub trait ToolRouter: Send + Sync {
    /// The agent-safe descriptors exposed to external clients via `tools/list`.
    fn external_manifest(&self) -> Vec<ToolDescriptor>;

    /// Dispatch a `tools/call`. Boxed future so the trait stays object-safe.
    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        args: Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;

    /// Fires when [`external_manifest`](Self::external_manifest) would return
    /// something different, so the connection can tell its client to re-list.
    ///
    /// Defaulted to `None`: a router whose manifest is fixed for the life of
    /// the process — every test router, and any future static one — says
    /// nothing rather than being made to carry a channel it would never use.
    fn subscribe_manifest_changed(&self) -> Option<tokio::sync::broadcast::Receiver<()>> {
        None
    }
}

#[cfg(test)]
/// A router with no tools — used by transport/server unit tests.
pub struct NullRouter;

#[cfg(test)]
impl ToolRouter for NullRouter {
    fn external_manifest(&self) -> Vec<ToolDescriptor> {
        Vec::new()
    }

    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        _args: Value,
        _ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        let name = name.to_string();
        Box::pin(async move { ToolResult::Failed(format!("no such tool: {}", name)) })
    }
}

#[cfg(test)]
mod schema_shape_tests {
    /// Every advertised tool takes an OBJECT.
    ///
    /// MCP tool arguments are a named map, so `inputSchema` must be an object
    /// schema. `check_notebook_dependencies` shipped
    /// `{"type": "string", "description": "…"}` — the schema for the `notebook`
    /// PROPERTY, passed where the whole schema goes, which every other tool in
    /// that file wraps correctly.
    ///
    /// Nothing caught it. It dispatched fine, its own tests passed, and the
    /// tool worked when called: the damage was to `tools/list`, where a client
    /// that validates the catalogue rejects it — and some reject the entire
    /// list over one bad entry, which reads as "the server has no tools"
    /// rather than "one tool is malformed". Claude Desktop connected, listed,
    /// and reported the server with zero tools for a week.
    #[test]
    fn every_tool_advertises_an_object_schema() {
        let mut bad = Vec::new();
        for d in super::family_descriptors() {
            let schema = &d.input_schema;
            let Some(obj) = schema.as_object() else {
                bad.push(format!("{}: inputSchema is not an object", d.name));
                continue;
            };
            match obj.get("type").and_then(|t| t.as_str()) {
                Some("object") => {}
                other => bad.push(format!("{}: type = {other:?}, must be \"object\"", d.name)),
            }
            // `properties`, when present, is a map; and anything `required`
            // must be one of them, or a caller cannot satisfy the tool.
            if let Some(props) = obj.get("properties") {
                if !props.is_object() {
                    bad.push(format!("{}: properties is not an object", d.name));
                }
                if let Some(req) = obj.get("required").and_then(|r| r.as_array()) {
                    for r in req.iter().filter_map(|v| v.as_str()) {
                        if props.get(r).is_none() {
                            bad.push(format!(
                                "{}: requires {r:?}, which it does not declare",
                                d.name
                            ));
                        }
                    }
                }
            }
        }

        assert!(
            bad.is_empty(),
            "tool(s) advertising a schema no client can call them by; a strict \
             client may drop the WHOLE catalogue over one of these: {bad:#?}"
        );
    }
}

#[cfg(test)]
mod arg_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn case_conversions_are_idempotent_and_inverse() {
        assert_eq!(camel_case("publisher_id"), "publisherId");
        assert_eq!(camel_case("max_bytes"), "maxBytes");
        assert_eq!(camel_case("a_b_c"), "aBC");
        // Already camelCase: unchanged.
        assert_eq!(camel_case("publisherId"), "publisherId");

        assert_eq!(snake_case("publisherId"), "publisher_id");
        assert_eq!(snake_case("maxBytes"), "max_bytes");
        // Already snake_case: unchanged.
        assert_eq!(snake_case("publisher_id"), "publisher_id");
    }

    /// The reference deserializes arguments case-insensitively, so an agent may
    /// send either spelling regardless of what the schema declares. Both must
    /// resolve, or a prompt written against one app breaks on the other.
    #[test]
    fn either_naming_style_resolves() {
        let camel = json!({ "publisherId": "ivo://x?1", "maxBytes": 10 });
        // Built key-by-key so a bulk casing pass over this file can never
        // silently turn this fixture into a second camelCase case.
        let mut snake = serde_json::Map::new();
        snake.insert("publisher".to_string() + "_id", json!("ivo://x?1"));
        snake.insert("max".to_string() + "_bytes", json!(10));
        let snake = Value::Object(snake);

        for args in [&camel, &snake] {
            assert_eq!(str_arg(args, "publisherId"), "ivo://x?1");
            assert_eq!(str_arg(args, "publisher_id"), "ivo://x?1");
            assert_eq!(arg(args, "maxBytes").and_then(|v| v.as_u64()), Some(10));
            assert_eq!(arg(args, "max_bytes").and_then(|v| v.as_u64()), Some(10));
        }
    }

    #[test]
    fn absent_and_wrong_type_arguments_are_empty_not_panics() {
        let args = json!({ "count": 3 });
        assert_eq!(str_arg(&args, "missing"), "");
        // Present but not a string.
        assert_eq!(str_arg(&args, "count"), "");
        assert!(arg(&args, "missing").is_none());
    }

    /// Viewer hosts must read tool arguments through [`arg`], never `args.get`.
    ///
    /// This is a source-level check because the failure it catches is invisible
    /// at runtime AND untestable at the unit level: the hosts need a live GTK
    /// widget tree. When tool schemas moved to camelCase, three hosts kept
    /// reading `min_cut` / `north_up` / `center_x` directly, so every one of
    /// those parameters was silently ignored — the call succeeded and did
    /// nothing. `arg` accepts either spelling, so routing through it makes the
    /// mismatch impossible rather than merely fixed.
    #[test]
    fn viewer_hosts_read_arguments_through_the_shared_accessor() {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut offenders: Vec<String> = Vec::new();
        let mut checked = 0usize;

        let mut stack = vec![ui];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Only files that actually serve bridge commands.
                if !text.contains("handle_viewer_command") {
                    continue;
                }
                checked += 1;
                // Whitespace-insensitive: `args.get(` and the rustfmt-wrapped
                // `args\n    .get(` are the same bug, and the naive substring
                // check missed the second form entirely.
                let squashed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                if squashed.contains("args.get(") {
                    offenders.push(path.display().to_string());
                }
            }
        }

        assert!(
            checked > 0,
            "found no viewer hosts to check — did src/ui move?"
        );
        assert!(
            offenders.is_empty(),
            "viewer host(s) read tool arguments with `args.get(...)`, which only \
             matches one spelling; use `crate::mcp::tools::arg(args, ..)`: {offenders:?}"
        );
    }

    /// Viewer hosts must emit camelCase JSON keys, like every other tool payload.
    ///
    /// The hosts BUILD tool results, so a snake_case key there reaches the wire
    /// just as surely as one in `mcp::tools` — and is just as invisible, since no
    /// test can see a GTK host's output. Scanned for the same reason as above.
    #[test]
    fn viewer_hosts_emit_camel_case_payload_keys() {
        /// Quoted strings immediately followed by `:` — i.e. JSON keys — that are
        /// written snake_case.
        fn snake_keys(line: &str) -> Vec<String> {
            let chars: Vec<char> = line.chars().collect();
            let mut found = Vec::new();
            let mut i = 0;
            while i < chars.len() {
                if chars[i] != '"' {
                    i += 1;
                    continue;
                }
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && chars[j] != '"' {
                    j += 1;
                }
                if j >= chars.len() {
                    break;
                }
                let key: String = chars[start..j].iter().collect();
                let is_key = chars.get(j + 1) == Some(&':');
                let snake = key.contains('_')
                    && key
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit());
                if is_key && snake {
                    found.push(key);
                }
                i = j + 1;
            }
            found
        }

        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut offenders: Vec<String> = Vec::new();
        let mut stack = vec![ui];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if !text.contains("handle_viewer_command") {
                    continue;
                }
                for key in text.lines().flat_map(snake_keys) {
                    offenders.push(format!("{}: {key}", path.display()));
                }
                // `payload["some_key"] = ..` never matches the `"key":` shape but
                // reaches the wire just the same.
                for line in text.lines() {
                    for (i, _) in line.match_indices("\"] =") {
                        let head = &line[..i];
                        if let Some(open) = head.rfind('"') {
                            let key = &head[open + 1..];
                            let snake = key.contains('_')
                                && key.chars().all(|c| {
                                    c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()
                                });
                            if snake {
                                offenders.push(format!("{}: {key}", path.display()));
                            }
                        }
                    }
                }
            }
        }
        offenders.sort();
        offenders.dedup();
        assert!(
            offenders.is_empty(),
            "viewer host(s) emit snake_case JSON keys; the reference emits camelCase: {offenders:?}"
        );
    }

    #[test]
    fn string_arguments_are_trimmed() {
        let args = json!({ "path": "  data/run1  " });
        assert_eq!(str_arg(&args, "path"), "data/run1");
    }

    #[test]
    fn rounding_matches_the_precision_each_tool_promises() {
        // The reference rounds quota figures to 2dp and the percentage to 1dp.
        assert_eq!(round_dp(23.847263918273544, 2), 23.85);
        assert_eq!(round_dp(66.66666666666667, 1), 66.7);
        assert_eq!(round_dp(5.0, 2), 5.0);
    }

    #[test]
    fn rounding_leaves_a_non_finite_value_alone() {
        // A zero quota yields 0/0. Multiplying NaN by a factor and rounding
        // would still be NaN, but going through the branch makes the intent
        // explicit: serde renders it as null, which is the honest answer.
        assert!(round_dp(f64::NAN, 2).is_nan());
        assert_eq!(round_dp(f64::INFINITY, 2), f64::INFINITY);
    }

    #[test]
    fn a_tab_switch_reports_the_reference_outcome_alongside_the_state() {
        // The viewer's own state is the useful part; the outcome keys are what a
        // reference-written agent checks. Both have to survive.
        let state = json!({ "fileName": "m31.fits", "zoomPercent": 100 });
        let out = with_tab_switch_outcome(state, 1, 3, "m31.fits");

        assert_eq!(out["switched"], true);
        assert_eq!(out["index"], 1);
        assert_eq!(out["count"], 3);
        assert_eq!(out["activeName"], "m31.fits");
        assert!(out["message"].is_null());
        assert_eq!(out["zoomPercent"], 100, "the viewer state must survive");
    }

    #[test]
    fn the_outcome_wins_over_a_colliding_state_key() {
        // Neither viewer emits a top-level `index` or `count` today. If one ever
        // does, this pins the resolution: the switch outcome is authoritative,
        // because it describes the operation the caller just performed. The
        // viewer would need to rename its field, and this test says so.
        let state = json!({ "index": 99, "count": 99 });
        let out = with_tab_switch_outcome(state, 1, 3, "x");
        assert_eq!(out["index"], 1);
        assert_eq!(out["count"], 3);
    }
}
