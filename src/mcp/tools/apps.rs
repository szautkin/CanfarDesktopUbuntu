//! Telling an agent what this app can do, without telling it all 147 ways.
//!
//! `tools/list` is 147 schemas — 96 KB, about 24 000 tokens, measured against
//! the running app — and most of it is irrelevant to any one task. An agent
//! reading all of it spends context before it starts, and chooses worse for
//! having more to choose between.
//!
//! These three tools are the map:
//!
//!  * [`list_apps`] — the ~17 apps and what each is for. Small enough to keep.
//!  * `describe_app(app)` — one app's tools, with their schemas. This is the
//!    working set for a task. The tool is declared by `read`, which has always
//!    owned it; this module supplies the `app` branch, so there is one
//!    declaration rather than two that must agree.
//!  * `search_tools(query)` — for when the app is not obvious, which is the
//!    common case when a model knows what it wants but not where it lives.
//!
//! **They do not shrink `tools/list`.** A client reads that on connect and there
//! is no way for a server to say otherwise, so these help an agent CHOOSE, not
//! an agent LOAD. Making the payload itself smaller needs the server to
//! advertise less — the `listChanged` machinery for it already exists — and is
//! a separate, riskier change. Anyone expecting a token reduction from this
//! file alone will not find one; see `dev_info/19`.
//!
//! The taxonomy is [`crate::models::tool_category`], which the AI Guide window
//! reads too. One table: an app added here appears in both, or in neither.

use super::{ToolDescriptor, ToolResult, VerbClass};
use crate::models::tool_category as taxonomy;
use serde_json::{json, Value};

/// The tools this module answers to.
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "list_apps".into(),
            description: "START HERE when you do not know which tool you need. Lists the app's \
                          areas — FITS Viewer, Cube Viewer, Notebook, Storage, Search, Sessions \
                          and the rest — with what each is for and how many tools it has. Then \
                          call describe_app with the one you want to get just those tools and \
                          their arguments, instead of reading all of them."
                .into(),
            input_schema: json!({
                "type": "object", "properties": {}, "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "man".into(),
            description: "Everything about ONE tool: what it does in full, every argument with \
                          its type and meaning, which area it belongs to, and the other tools in \
                          that area. Pass `tool` with a tool name. Use it before calling anything \
                          whose arguments you are unsure of, and after an error you do not \
                          understand."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tool": {"type": "string", "description": "The tool name to look up."}
                },
                "required": ["tool"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "search_tools".into(),
            description: "Find tools by what they DO, across every area — use when you know what \
                          you want but not which app owns it (\"draw a region\", \"spectrum\", \
                          \"upload a file\"). Matches names and descriptions, best first, and \
                          says which app each belongs to so you can describe_app that one next."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "words to look for"},
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": 50,
                        "description": "how many hits (default 12)"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
    ]
}

/// The whole tool set as a grouped map: each app, what it is for, and the names
/// it owns.
///
/// This is what an agent needs BEFORE it has read anything — not 149 schemas,
/// but the shape of them. A client must call `tools/list` and receives whatever
/// is advertised; nothing in the protocol lets a server say "read this
/// instead". The one channel that reaches the model first is `instructions`,
/// and this is sized to fit there: names and one line per app, no schemas.
///
/// Names matter more than they look. A tool that is NOT advertised is still
/// callable — the router's gate refuses only tools that are known and not
/// agent-safe — so an agent that reads a name here can use it immediately, and
/// ask `describe_app` for its arguments when it needs them.
pub fn tool_map(manifest: Vec<ToolDescriptor>) -> String {
    let grouped = taxonomy::group_by_category(manifest, |d| d.name.clone());
    let mut out = String::new();
    for (cat, tools) in grouped {
        let names: Vec<&str> = tools.iter().map(|d| d.name.as_str()).collect();
        out.push_str(&format!(
            "\n{} ({}) — {}\n  {}\n",
            cat.id,
            cat.title,
            cat.summary,
            names.join(", ")
        ));
    }
    out
}

/// The apps, with a count of the tools in each.
///
/// `manifest` is the tool set as ADVERTISED — the router's, complete with the
/// user's description overrides, their own guide tools, and the `agent_safe`
/// filter. Rebuilding it here from the module tables looked equivalent and was
/// not: it missed nine live tools, and the test comparing the two passed
/// because it asked the same wrong function twice.
pub fn list_apps(manifest: Vec<ToolDescriptor>) -> Value {
    let grouped = taxonomy::group_by_category(manifest, |d| d.name.clone());
    let apps: Vec<Value> = grouped
        .iter()
        .map(|(cat, tools)| {
            json!({
                "id": cat.id,
                "title": cat.title,
                "description": cat.summary,
                "toolCount": tools.len(),
            })
        })
        .collect();
    json!({
        "apps": apps,
        "totalTools": grouped.iter().map(|(_, t)| t.len()).sum::<usize>(),
        "next": "describe_app with an app id for that area's tools, or search_tools \
                 if you are not sure which area owns what you need",
    })
}

/// One app's tools, with their schemas.
pub fn describe_one_app(app: &str, manifest: Vec<ToolDescriptor>) -> Result<Value, String> {
    let cat = taxonomy::by_id(app).ok_or_else(|| unknown_app_message(app))?;
    let grouped = taxonomy::group_by_category(manifest, |d| d.name.clone());
    let tools: Vec<Value> = grouped
        .into_iter()
        .find(|(c, _)| c.id == cat.id)
        .map(|(_, tools)| {
            tools
                .into_iter()
                .map(|d| {
                    json!({
                        "name": d.name,
                        "description": d.description,
                        "inputSchema": d.input_schema,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "id": cat.id,
        "title": cat.title,
        "description": cat.summary,
        "toolCount": tools.len(),
        "tools": tools,
    }))
}

/// Why that app id did not match, and what the ids are.
///
/// The list is included because a wrong id is nearly always a near miss, and an
/// agent that has to call `list_apps` again to recover has paid twice for one
/// mistake.
fn unknown_app_message(app: &str) -> String {
    let ids: Vec<&str> = taxonomy::all().map(|c| c.id).collect();
    format!(
        "no app called {app:?}. The app ids are: {}. Call list_apps for what each one does.",
        ids.join(", ")
    )
}

/// One tool in full: its own entry, its app, and its siblings.
///
/// The page an agent reads when a name is not enough — before a call it is
/// unsure of, or after an error it does not understand. Siblings are listed
/// because the tool someone needs is very often the one NEXT to the one they
/// looked up.
pub fn man(tool: &str, manifest: Vec<ToolDescriptor>) -> Result<Value, String> {
    let wanted = tool.trim();
    let found = manifest
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(wanted))
        .ok_or_else(|| unknown_tool_message(wanted, &manifest))?;

    let app = taxonomy::category_id_for_tool(&found.name);
    let category = taxonomy::by_id(app);
    let siblings: Vec<&str> = manifest
        .iter()
        .filter(|d| d.name != found.name && taxonomy::category_id_for_tool(&d.name) == app)
        .map(|d| d.name.as_str())
        .collect();

    Ok(json!({
        "name": found.name,
        "description": found.description,
        "inputSchema": found.input_schema,
        "example": example_call(&found.name, &found.input_schema),
        "app": app,
        "appTitle": category.map(|c| c.title),
        "appDescription": category.map(|c| c.summary),
        "alsoInThisApp": siblings,
    }))
}

/// A ready-to-send call for this tool, built from its own schema.
///
/// Three of the nine failures in a full QA pass were the caller guessing an
/// argument name — `type` for `kind`, `vospacePath` for `path`. The schema said
/// so all along, but a list of properties does not show the SHAPE of a call the
/// way one filled-in example does.
///
/// Generated rather than written per tool, so it cannot drift from the schema
/// it documents: 42 hand-written examples would be 42 things to forget.
/// How many arguments to show for a tool that requires none.
const MAX_EXAMPLE_ARGS: usize = 6;

fn example_call(name: &str, schema: &Value) -> Value {
    let props = schema.get("properties").and_then(|p| p.as_object());
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut args = serde_json::Map::new();
    if let Some(props) = props {
        if required.is_empty() {
            // Nothing is required, so the smallest call that works is an empty
            // one — and an empty example is useless in exactly the place
            // guessing is likeliest. `launch_session` requires none of its
            // arguments, which is how a caller came to invent `type` when the
            // schema says `kind`. Show the shape instead, capped so a
            // wide-surface tool does not answer with a wall.
            for (key, spec) in props.iter().take(MAX_EXAMPLE_ARGS) {
                args.insert(key.clone(), example_value(key, spec));
            }
        } else {
            // The required ones, in the schema's own order — that is the
            // smallest call that works, which is what an example is for.
            for key in &required {
                if let Some(spec) = props.get(*key) {
                    args.insert((*key).to_string(), example_value(key, spec));
                }
            }
        }
    }
    json!({ "name": name, "arguments": Value::Object(args) })
}

/// A plausible value for one argument, taken from the schema where it says.
fn example_value(key: &str, spec: &Value) -> Value {
    // An enum states its own answers; anything else would be a guess.
    if let Some(first) = spec
        .get("enum")
        .and_then(|e| e.as_array())
        .and_then(|e| e.first())
    {
        return first.clone();
    }
    match spec.get("type").and_then(|t| t.as_str()) {
        Some("integer") | Some("number") => spec
            .get("minimum")
            .cloned()
            .unwrap_or_else(|| Value::from(1)),
        Some("boolean") => Value::Bool(true),
        Some("array") => Value::Array(vec![]),
        // A description that already carries an "e.g." is the author's own
        // example, and better than anything invented here.
        _ => Value::String(
            spec.get("description")
                .and_then(|d| d.as_str())
                .and_then(example_from_description)
                .unwrap_or_else(|| format!("<{key}>")),
        ),
    }
}

/// Pull the sample out of a description that says "e.g. `something`".
fn example_from_description(description: &str) -> Option<String> {
    let at = description.find("e.g. ")? + "e.g. ".len();
    let rest = description[at..].trim_start_matches(['\'', '`', '"']);
    let end = rest.find([',', '\'', '`', '"', ')']).unwrap_or(rest.len());
    let sample = rest[..end].trim().trim_end_matches('.');
    (!sample.is_empty()).then(|| sample.to_string())
}

/// Why that tool name did not match, with the nearest ones that do.
///
/// A name an agent got slightly wrong is the common case, so the answer is the
/// candidates rather than a refusal — `search_tools` exists for the rest.
fn unknown_tool_message(tool: &str, manifest: &[ToolDescriptor]) -> String {
    // Rank by how many word-parts a name shares, not by one "best" part. The
    // first version took the LONGEST part, so `get_fits_picture` was looked up
    // by "picture" — which matches nothing — and the answer offered no
    // candidates at all, when `get_fits_image` was sitting right there.
    let lower = tool.to_lowercase();
    let parts: Vec<&str> = lower.split('_').filter(|p| p.len() > 2).collect();
    let mut scored: Vec<(usize, &str)> = manifest
        .iter()
        .filter_map(|d| {
            let name = d.name.to_lowercase();
            let shared = parts.iter().filter(|p| name.contains(**p)).count();
            (shared > 0).then_some((shared, d.name.as_str()))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let near: Vec<&str> = scored.into_iter().take(6).map(|(_, n)| n).collect();
    if near.is_empty() {
        format!(
            "no tool called {tool:?}. Call search_tools with what you are trying to do, \
             or list_apps for the areas."
        )
    } else {
        format!(
            "no tool called {tool:?}. Did you mean: {}? Otherwise call search_tools with \
             what you are trying to do.",
            near.join(", ")
        )
    }
}

/// Tools whose name or description matches `query`, best first.
pub fn search_tools(query: &str, limit: usize, manifest: Vec<ToolDescriptor>) -> Value {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|t| !t.is_empty() && !is_noise(t))
        .collect();

    // Every term, then any term. Requiring all of them keeps a two-word query
    // precise, but a phrase an agent actually types — "image region", "draw a
    // box" — has words no tool description contains, and strict matching
    // answers nothing. An empty result is the one answer that helps nobody: it
    // sends the agent back to reading all 147. So the strict pass runs first and
    // the loose pass only rescues it, with `relaxed` saying which happened.
    let strict: Vec<ToolDescriptor> = manifest
        .iter()
        .filter(|d| score_tool(d, &terms, true) > 0)
        .cloned()
        .collect();
    let relaxed = strict.is_empty();
    let candidates = if relaxed { manifest } else { strict };

    let mut hits: Vec<(u32, Value)> = candidates
        .into_iter()
        .filter_map(|d| {
            let score = score_tool(&d, &terms, !relaxed);
            (score > 0).then(|| {
                (
                    score,
                    json!({
                        "name": d.name,
                        "app": taxonomy::category_id_for_tool(&d.name),
                        "description": d.description,
                        "score": score,
                    }),
                )
            })
        })
        .collect();

    // Best first, then by name so equal scores do not shuffle between calls.
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            a.1["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b.1["name"].as_str().unwrap_or(""))
        })
    });
    let total = hits.len();
    let tools: Vec<Value> = hits.into_iter().take(limit).map(|(_, v)| v).collect();

    json!({
        "query": query,
        "matched": total,
        "returned": tools.len(),
        // True when no tool matched every word, so these match SOME of them.
        // Worth saying: a loose hit is a suggestion, not an answer.
        "relaxed": relaxed,
        "tools": tools,
        "next": "describe_app with a hit's `app` for the rest of that area's tools",
    })
}

/// Words that carry no signal in a tool search.
///
/// An agent asks in a sentence — "draw a box on the image" — and "a", "on" and
/// "the" appear in nearly every description, so measured against the live app
/// that phrase matched all 149 tools: a ranked list of everything, which is the
/// same as no list. Dropping them leaves "draw box image", which is the
/// question.
///
/// Single letters go too, for the same reason and because a stray initial is
/// never the word someone meant.
fn is_noise(term: &str) -> bool {
    const NOISE: &[&str] = &[
        "a", "an", "and", "any", "are", "as", "at", "be", "by", "can", "do", "for", "from", "get",
        "how", "i", "in", "is", "it", "me", "my", "of", "on", "or", "please", "that", "the",
        "then", "this", "to", "want", "was", "what", "which", "with", "you",
    ];
    term.len() < 2 || NOISE.contains(&term)
}

/// How well one tool answers `terms`.
///
/// A name match outranks a description match, because a model searching for
/// "spectrum" wants `probe_cube_spectrum` before every tool whose prose happens
/// to mention spectra. Every term must appear somewhere, so a two-word query
/// narrows rather than widens — the opposite would make `search_tools` return
/// most of the catalogue for any query with a common word in it.
fn score_tool(d: &ToolDescriptor, terms: &[String], require_all: bool) -> u32 {
    if terms.is_empty() {
        return 0;
    }
    let name = d.name.to_lowercase();
    let description = d.description.to_lowercase();
    let mut score = 0;
    for term in terms {
        let in_name = name.contains(term.as_str());
        let in_description = description.contains(term.as_str());
        if !in_name && !in_description {
            if require_all {
                return 0;
            }
            continue;
        }
        if in_name {
            score += if name == *term { 20 } else { 10 };
        }
        if in_description {
            score += 1;
        }
    }
    score
}

/// Dispatch: `None` if this module does not own `name`.
pub fn dispatch(name: &str, args: &Value, manifest: Vec<ToolDescriptor>) -> Option<ToolResult> {
    match name {
        "list_apps" => Some(ToolResult::Data(list_apps(manifest))),
        "man" => {
            let tool = super::arg(args, "tool")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if tool.is_empty() {
                return Some(ToolResult::Failed(
                    "tool is required — the name of the tool to look up".to_string(),
                ));
            }
            Some(match man(&tool, manifest) {
                Ok(v) => ToolResult::Data(v),
                Err(e) => ToolResult::Failed(e),
            })
        }
        "describe_app" => super::arg(args, "app")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|app| match describe_one_app(app, manifest) {
                Ok(v) => ToolResult::Data(v),
                Err(e) => ToolResult::Failed(e),
            }),
        "search_tools" => {
            let query = super::arg(args, "query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if query.is_empty() {
                return Some(ToolResult::Failed(
                    "query is required — words describing what you want to do, \
                     e.g. \"open a cube\" or \"upload\""
                        .to_string(),
                ));
            }
            let limit = super::arg(args, "limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(12)
                .clamp(1, 50) as usize;
            Some(ToolResult::Data(search_tools(&query, limit, manifest)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod example_tests {
    use super::*;

    #[test]
    fn the_example_uses_the_schemas_own_argument_names() {
        // The failure this prevents: a caller guessing `type` when the schema
        // says `kind`, or `vospacePath` when it says `path`. Three of nine
        // failures in a full QA pass were exactly that.
        let schema = json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["notebook", "desktop"] },
                "image": { "type": "string", "description": "Container image, e.g. `skaha/base:1.0`." },
                "cores": { "type": "integer", "minimum": 2 },
                "name": { "type": "string", "description": "Optional." }
            },
            "required": ["kind", "image", "cores"]
        });
        let example = example_call("launch_session", &schema);
        let args = &example["arguments"];

        assert_eq!(example["name"], "launch_session");
        // An enum states its own answer.
        assert_eq!(args["kind"], "notebook");
        // A description carrying "e.g." is the author's own sample.
        assert_eq!(args["image"], "skaha/base:1.0");
        // A number takes its own minimum rather than an invented 1.
        assert_eq!(args["cores"], 2);
        // Optional arguments stay out: the example is the smallest call that
        // works, not a catalogue.
        assert!(args.get("name").is_none(), "an optional argument crept in");
    }

    #[test]
    fn a_tool_that_requires_nothing_still_shows_its_argument_names() {
        // `launch_session` requires none of its arguments, so "the smallest
        // call that works" is `{}` — and an empty example is useless exactly
        // where guessing is likeliest. A caller invented `type` because nothing
        // showed them `kind`.
        let schema = json!({
            "properties": {
                "kind": { "type": "string", "enum": ["notebook"] },
                "image": { "type": "string" }
            }
        });
        let args = example_call("launch_session", &schema)["arguments"].clone();
        assert_eq!(args["kind"], "notebook");
        assert!(args.get("image").is_some());
        assert!(
            args.get("type").is_none(),
            "the example must show the schema's names, not a caller's guess"
        );
    }

    #[test]
    fn an_argument_with_nothing_to_go_on_is_named_not_guessed() {
        let schema = json!({
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        });
        let example = example_call("read_vospace_file", &schema);
        assert_eq!(example["arguments"]["path"], "<path>");
    }

    #[test]
    fn a_tool_that_takes_nothing_still_shows_the_shape() {
        let example = example_call("get_auth_state", &json!({ "type": "object" }));
        assert_eq!(example["name"], "get_auth_state");
        assert!(example["arguments"].as_object().unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in manifest, so these test the LOGIC and not the tool set.
    ///
    /// Deliberately not the real one. The first version of this module built
    /// its own "live tools" list and the tests compared `list_apps` against
    /// that same function — so both agreed, both were wrong, and nine
    /// advertised tools were missing from every app, including every guide tool
    /// the user had written. A manifest is now an argument, and the router is
    /// the only thing that decides what is in it.
    fn manifest() -> Vec<ToolDescriptor> {
        [
            "get_fits_image",
            "set_fits_view",
            "probe_cube_spectrum",
            "run_cell",
            "list_apps",
        ]
        .iter()
        .map(|name| ToolDescriptor {
            name: (*name).to_string(),
            description: format!("does {name} things"),
            input_schema: json!({"type": "object", "properties": {}}),
            verb: VerbClass::Read,
            agent_safe: true,
        })
        .collect()
    }

    #[test]
    fn every_app_with_tools_is_listed() {
        let value = list_apps(manifest());
        let apps = value["apps"].as_array().expect("apps");
        assert!(!apps.is_empty());
        for app in apps {
            assert!(!app["id"].as_str().unwrap_or("").is_empty(), "{app}");
            assert!(!app["title"].as_str().unwrap_or("").is_empty(), "{app}");
            assert!(
                !app["description"].as_str().unwrap_or("").is_empty(),
                "an app with no description tells an agent nothing: {app}"
            );
            assert!(app["toolCount"].as_u64().unwrap_or(0) > 0, "{app}");
        }
    }

    /// The apps account for every tool in the manifest, exactly once.
    ///
    /// The invariant, rather than a count: a test asserting "147 tools" has to
    /// be edited by whoever adds the 148th, and a test like that gets deleted
    /// instead of fixed.
    #[test]
    fn the_apps_partition_the_manifest_exactly_once() {
        let m = manifest();
        let total: u64 = list_apps(m.clone())["apps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["toolCount"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(
            total as usize,
            m.len(),
            "the apps lost or duplicated a tool"
        );
    }

    /// A tool with no category still reaches an agent, under "Other".
    ///
    /// This is what stops the catalog from hiding a tool: the fallback bucket
    /// is a real app, so a newly added tool is visible before anyone remembers
    /// to categorise it.
    #[test]
    fn an_uncategorised_tool_is_not_lost() {
        let mut m = manifest();
        m.push(ToolDescriptor {
            name: "brand_new_thing_nobody_sorted".to_string(),
            description: "unsorted".to_string(),
            input_schema: json!({"type": "object"}),
            verb: VerbClass::Read,
            agent_safe: true,
        });
        let value = list_apps(m.clone());
        let total: u64 = value["apps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["toolCount"].as_u64().unwrap())
            .sum();
        assert_eq!(total as usize, m.len(), "the uncategorised tool vanished");

        let other = describe_one_app("other", m).expect("other is an app");
        assert!(
            other["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "brand_new_thing_nobody_sorted"),
            "an uncategorised tool is not reachable through any app"
        );
    }

    #[test]
    fn describing_an_app_returns_its_tools_with_schemas() {
        let fits = describe_one_app("fits", manifest()).expect("fits is an app");
        assert_eq!(fits["id"], "fits");
        let tools = fits["tools"].as_array().expect("tools");
        assert!(!tools.is_empty());
        for t in tools {
            assert!(!t["name"].as_str().unwrap_or("").is_empty());
            assert!(!t["description"].as_str().unwrap_or("").is_empty());
            // The schema is the point: an agent needs the arguments, not just
            // the name, or it must go back to `tools/list` for them.
            assert!(t["inputSchema"].is_object(), "no schema for {}", t["name"]);
        }
        assert!(tools.iter().any(|t| t["name"] == "get_fits_image"));
        assert_eq!(fits["toolCount"].as_u64().unwrap() as usize, tools.len());
    }

    #[test]
    fn describing_an_app_matches_what_list_apps_promised() {
        for app in list_apps(manifest())["apps"].as_array().unwrap() {
            let id = app["id"].as_str().unwrap();
            let detail = describe_one_app(id, manifest()).expect(id);
            assert_eq!(
                detail["toolCount"], app["toolCount"],
                "list_apps and describe_app disagree about {id}"
            );
        }
    }

    /// The description an agent reads is the one the manifest carries.
    ///
    /// The AI Guide lets a user re-tune any tool's description, and the router
    /// applies those overrides when it builds the manifest. Rebuilding the list
    /// from the module tables would have quietly served the original text.
    #[test]
    fn a_retuned_description_reaches_the_agent() {
        let mut m = manifest();
        m[0].description = "RETUNED BY THE USER".to_string();
        let fits = describe_one_app("fits", m).expect("fits");
        let tool = fits["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "get_fits_image")
            .expect("get_fits_image");
        assert_eq!(tool["description"], "RETUNED BY THE USER");
    }

    #[test]
    fn an_unknown_app_lists_the_real_ones() {
        let err = describe_one_app("fits-viewer", manifest()).expect_err("not an id");
        assert!(err.contains("fits-viewer"), "{err}");
        assert!(err.contains("fits"), "the real ids are not offered: {err}");
        assert!(err.contains("list_apps"), "{err}");
    }

    #[test]
    fn an_app_id_is_matched_case_insensitively() {
        assert!(describe_one_app("FITS", manifest()).is_ok());
        assert!(describe_one_app("Notebook", manifest()).is_ok());
    }

    #[test]
    fn search_finds_a_tool_by_its_name() {
        let hits = search_tools("cube spectrum", 12, manifest());
        let names: Vec<&str> = hits["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n.contains("cube") && n.contains("spectrum")),
            "no cube spectrum tool in {names:?}"
        );
        assert_eq!(
            hits["tools"][0]["app"].as_str(),
            Some("cube"),
            "the best hit does not name its app"
        );
    }

    /// A name match beats a description match.
    #[test]
    fn a_name_match_outranks_a_passing_mention() {
        let mut m = manifest();
        m.push(ToolDescriptor {
            name: "unrelated_tool".to_string(),
            description: "mentions run_cell in passing".to_string(),
            input_schema: json!({"type": "object"}),
            verb: VerbClass::Read,
            agent_safe: true,
        });
        let hits = search_tools("run_cell", 10, m);
        assert_eq!(
            hits["tools"][0]["name"], "run_cell",
            "a passing mention outranked the tool named for it"
        );
    }

    /// Every term must match, so a second word narrows the result.
    #[test]
    fn more_words_narrow_the_search() {
        let broad = search_tools("fits", 50, manifest())["matched"]
            .as_u64()
            .unwrap();
        let narrow = search_tools("fits view", 50, manifest())["matched"]
            .as_u64()
            .unwrap();
        assert!(
            narrow < broad,
            "adding a word widened the search ({broad} -> {narrow}); an OR search \
             returns most of the catalogue for any common word"
        );
        assert!(narrow > 0, "the narrower search found nothing");
    }

    /// A phrase with a word no tool uses still returns something useful.
    ///
    /// Measured against the live app before this existed: `search_tools("image
    /// region")` answered with an empty list, because no tool description
    /// contains "region". An agent that asks a reasonable question and is told
    /// "nothing" goes back to reading all 147 tools, which is what this module
    /// exists to avoid.
    #[test]
    fn a_phrase_with_an_unknown_word_falls_back_to_the_words_it_knows() {
        let hits = search_tools("fits region", 12, manifest());
        assert!(
            hits["matched"].as_u64().unwrap() > 0,
            "an unmatched word emptied the result"
        );
        assert_eq!(
            hits["relaxed"], true,
            "a loosened search must say it was loosened"
        );
        let names: Vec<&str> = hits["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.iter().any(|n| n.contains("fits")), "{names:?}");
    }

    /// A search that DOES match every word is not loosened.
    #[test]
    fn a_strict_match_is_not_reported_as_relaxed() {
        let hits = search_tools("fits view", 12, manifest());
        assert!(hits["matched"].as_u64().unwrap() > 0);
        assert_eq!(hits["relaxed"], false);
    }

    /// A question phrased as a sentence searches for its content words.
    #[test]
    fn filler_words_do_not_match_everything() {
        let m = manifest();
        let sentence = search_tools("show me the run_cell for my notebook", 50, m.clone());
        let bare = search_tools("run_cell notebook", 50, m.clone());
        assert_eq!(
            sentence["matched"], bare["matched"],
            "the filler words changed the result"
        );
        assert_eq!(sentence["tools"][0]["name"], bare["tools"][0]["name"]);

        // A query of nothing BUT filler finds nothing, rather than everything.
        let noise = search_tools("what is the a of on", 50, m);
        assert_eq!(
            noise["matched"], 0,
            "a query with no content words matched something"
        );
    }

    #[test]
    fn a_search_that_matches_nothing_says_so_rather_than_guessing() {
        let hits = search_tools("xyzzy nothing here", 12, manifest());
        assert_eq!(hits["matched"], 0);
        assert!(hits["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_limit_is_respected_and_the_total_is_still_reported() {
        let hits = search_tools("e", 2, manifest());
        assert!(hits["tools"].as_array().unwrap().len() <= 2);
        assert!(
            hits["matched"].as_u64().unwrap() >= hits["returned"].as_u64().unwrap(),
            "a truncated result must still say how many matched"
        );
    }

    #[test]
    fn results_do_not_shuffle_between_identical_calls() {
        assert_eq!(
            search_tools("view", 20, manifest()),
            search_tools("view", 20, manifest()),
            "equal scores are not ordered deterministically"
        );
    }

    #[test]
    fn an_empty_query_is_refused_with_a_usable_message() {
        let result =
            dispatch("search_tools", &json!({"query": "   "}), manifest()).expect("dispatched");
        match result {
            ToolResult::Failed(m) => assert!(m.contains("query is required"), "{m}"),
            _ => panic!("an empty query should be refused"),
        }
    }

    /// `describe_app` with no `app` is not this module's to answer.
    #[test]
    fn describe_app_without_an_app_falls_through_to_the_overview() {
        assert!(dispatch("describe_app", &json!({}), manifest()).is_none());
        assert!(dispatch("describe_app", &json!({"app": "fits"}), manifest()).is_some());
    }

    #[test]
    fn man_returns_one_tool_in_full_with_its_neighbours() {
        let page = man("get_fits_image", manifest()).expect("get_fits_image");
        assert_eq!(page["name"], "get_fits_image");
        // The full description and the real schema — this is the page an agent
        // reads INSTEAD of carrying every tool's prose in context.
        assert!(!page["description"].as_str().unwrap_or("").is_empty());
        assert!(page["inputSchema"].is_object());
        assert_eq!(page["app"], "fits");
        assert!(!page["appTitle"].as_str().unwrap_or("").is_empty());
        // Siblings, because the tool someone needs is often the next one along.
        let siblings = page["alsoInThisApp"].as_array().unwrap();
        assert!(
            siblings.iter().any(|s| s == "set_fits_view"),
            "{siblings:?}"
        );
        assert!(
            !siblings.iter().any(|s| s == "get_fits_image"),
            "a tool is listed as its own neighbour"
        );
    }

    #[test]
    fn man_is_case_insensitive() {
        assert!(man("GET_FITS_IMAGE", manifest()).is_ok());
    }

    /// A near-miss gets the candidates, not a refusal.
    ///
    /// Measured against the live app: `get_fits_picture` first answered with no
    /// candidates at all, because the lookup used the LONGEST word-part —
    /// "picture", which matches nothing — while `get_fits_image` sat one word
    /// away. Ranking by shared parts finds it.
    #[test]
    fn an_unknown_tool_offers_the_nearest_names() {
        let err = man("get_fits_picture", manifest()).expect_err("no such tool");
        assert!(err.contains("get_fits_picture"), "{err}");
        assert!(
            err.contains("get_fits_image"),
            "the obvious candidate was not offered: {err}"
        );
    }

    /// The best candidate comes first.
    #[test]
    fn candidates_are_ranked_by_how_much_they_share() {
        let err = man("cube_spectrum_probe", manifest()).expect_err("no such tool");
        let at_probe = err.find("probe_cube_spectrum").unwrap_or(usize::MAX);
        let at_other = err.find("run_cell").unwrap_or(usize::MAX);
        assert!(
            at_probe < at_other,
            "the tool sharing two words did not come first: {err}"
        );
    }

    #[test]
    fn a_name_with_nothing_like_it_is_sent_to_search() {
        let err = man("zzzz", manifest()).expect_err("no such tool");
        assert!(err.contains("search_tools"), "{err}");
    }

    #[test]
    fn man_without_a_tool_name_says_what_it_needs() {
        match dispatch("man", &json!({"tool": "  "}), manifest()).expect("dispatched") {
            ToolResult::Failed(m) => assert!(m.contains("tool is required"), "{m}"),
            _ => panic!("an empty name should be refused"),
        }
    }

    #[test]
    fn this_module_only_answers_for_its_own_tools() {
        assert!(dispatch("get_fits_image", &json!({}), manifest()).is_none());
        assert!(dispatch("list_apps", &json!({}), manifest()).is_some());
    }
}
