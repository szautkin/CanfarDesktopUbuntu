//! The `.workflow.md` markdown-checklist dialect: pure parse / step-flip /
//! skeleton / validate, no I/O. Ported from `Services/Workflows/WorkflowFormat.cs`.
//!
//! Format: first `# ` line = title, first `> ` line = description, `Key: value`
//! lines before the first step = metadata, steps are `- [ ]` / `- [x]` items whose
//! optional `**bold lead**` is the title, with indented `Tool:` / `Tools:` /
//! `View:` / `Note:` attachment lines. Tolerant by design — anything unrecognized
//! becomes body text or a non-fatal warning, never a failure.

use crate::models::workflow::{WorkflowDoc, WorkflowStep};
use once_cell::sync::Lazy;
use regex::Regex;

pub const FILE_EXTENSION: &str = ".workflow.md";

/// The `View:` keys a step may deep-link to (mirrors `MainWindow.NavigateByKey`).
pub const KNOWN_VIEWS: &[&str] = &[
    "landing",
    "portal",
    "search",
    "research",
    "storage",
    "notebook",
    "fitsViewer",
    "aiGuide",
    "workflows",
];

static STEP_START: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*-\s*\[( |x|X)\]\s*(.*)$").unwrap());
static BOLD_LEAD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\*\*(.+?)\*\*\s*(?:[—–-]\s*)?(.*)$").unwrap());
static META_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Za-z][A-Za-z ]{0,30}):\s*(.+)$").unwrap());

fn normalize(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|s| s.to_string())
        .collect()
}

/// Try to read an indented `Key: value` attachment line (case-insensitive key).
fn try_attachment<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{}:", key);
    if trimmed.len() >= prefix.len()
        && trimmed[..prefix.len()].eq_ignore_ascii_case(&prefix)
    {
        Some(&trimmed[prefix.len()..])
    } else {
        None
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse a `.workflow.md` document. Never fails; unrecognized input becomes a warning.
pub fn parse(text: &str) -> WorkflowDoc {
    let lines = normalize(text);
    let mut warnings: Vec<String> = Vec::new();
    let mut title: Option<String> = None;
    let mut description = String::new();
    let mut metadata: Vec<(String, String)> = Vec::new();
    let mut steps: Vec<WorkflowStep> = Vec::new();

    // Mutable accumulator for the step currently being read.
    let mut step_title: Option<String> = None;
    let mut step_done = false;
    let mut body: Vec<String> = Vec::new();
    let mut tools: Vec<String> = Vec::new();
    let mut view: Option<String> = None;
    let mut note: Option<String> = None;
    let mut in_step = false;

    // Closure-like flush via a macro to avoid borrow gymnastics.
    macro_rules! flush_step {
        () => {
            if in_step {
                let idx = steps.len();
                let t = step_title.take().unwrap_or_default();
                let t = t.trim();
                let final_title = if !t.is_empty() {
                    t.to_string()
                } else {
                    format!("Step {}", idx + 1)
                };
                steps.push(WorkflowStep {
                    index: idx,
                    title: final_title,
                    body: body.join("\n").trim().to_string(),
                    tools: std::mem::take(&mut tools),
                    view: view.take(),
                    note: note.take(),
                    done: step_done,
                });
                step_done = false;
                body.clear();
                in_step = false;
            }
        };
    }

    for raw in &lines {
        let line = raw.trim_end();

        if let Some(caps) = STEP_START.captures(line) {
            flush_step!();
            in_step = true;
            let marker = &caps[1];
            step_done = marker == "x" || marker == "X";
            let content = caps[2].trim();
            if let Some(bold) = BOLD_LEAD.captures(content) {
                step_title = Some(bold[1].to_string());
                let rest = bold[2].trim();
                if !rest.is_empty() {
                    body.push(rest.to_string());
                }
            } else {
                step_title = Some(content.to_string());
            }
            continue;
        }

        if in_step {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with('#') {
                flush_step!();
                continue;
            }
            if let Some(v) = try_attachment(t, "Tool").or_else(|| try_attachment(t, "Tools")) {
                tools.extend(split_csv(v));
            } else if let Some(v) = try_attachment(t, "View") {
                view = Some(v.trim().to_string());
            } else if let Some(v) = try_attachment(t, "Note") {
                note = Some(v.trim().to_string());
            } else {
                body.push(t.to_string());
            }
            continue;
        }

        // Preamble (before the first step).
        if line.starts_with("# ") && !line.starts_with("##") {
            if title.is_none() {
                title = Some(line[2..].trim().to_string());
            }
            continue;
        }
        if line.starts_with("> ") {
            if description.is_empty() {
                description = line[2..].trim().to_string();
            }
            continue;
        }
        if line.starts_with('#') {
            continue; // section headings ("## Steps") are ignored
        }
        if let Some(caps) = META_LINE.captures(line.trim()) {
            let key = caps[1].trim().to_string();
            let val = caps[2].trim().to_string();
            // Last write wins on duplicate keys (case-insensitive).
            if let Some(entry) = metadata.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
                entry.1 = val;
            } else {
                metadata.push((key, val));
            }
        }
    }
    flush_step!();

    let title = match title {
        Some(t) => t,
        None => {
            warnings.push("No `# Title` line found — using \"Untitled workflow\".".to_string());
            "Untitled workflow".to_string()
        }
    };
    if steps.is_empty() {
        warnings
            .push("No steps found — add lines like `- [ ] **Step title** — what to do`.".to_string());
    }

    WorkflowDoc {
        title,
        description,
        metadata,
        steps,
        warnings,
    }
}

/// Flip the done-marker of the `step_index`-th step (0-based, in step order),
/// changing ONLY the `[ ]`/`[x]` characters — every other byte of the author's
/// text is preserved (the file is the state, so rewrites must not reformat).
/// Returns `Err` when the index doesn't exist.
pub fn with_step_done(text: &str, step_index: usize, done: bool) -> Result<String, String> {
    let mut lines: Vec<String> = normalize(text);
    let mut probe = 0usize;
    for line in lines.iter_mut() {
        if STEP_START.is_match(line.trim_end()) {
            if probe == step_index {
                if let Some(open) = line.find('[') {
                    // Replace exactly the single char at open+1.
                    let mut bytes: Vec<char> = line.chars().collect();
                    if open + 1 < bytes.len() {
                        bytes[open + 1] = if done { 'x' } else { ' ' };
                        *line = bytes.into_iter().collect();
                        return Ok(lines.join("\n"));
                    }
                }
            }
            probe += 1;
        }
    }
    Err(format!(
        "workflow has {} steps; step {} does not exist.",
        probe, step_index
    ))
}

/// A starter document for the "New workflow" action.
pub fn skeleton(title: &str) -> String {
    // Note: explicit "\n" + literal spaces (no `\` line-continuation, which would
    // strip the leading indentation of the attachment lines).
    let mut s = String::new();
    s.push_str(&format!("# {}\n", title));
    s.push_str("> One-line description of what this protocol achieves.\n");
    s.push_str("Tags: \n");
    s.push_str("Time: ~1 h\n");
    s.push('\n');
    s.push_str("## Steps\n");
    s.push('\n');
    s.push_str("- [ ] **First step** — What to do and why.\n");
    s.push_str("      Tool: search_observations\n");
    s.push_str("      View: search\n");
    s.push_str("- [ ] **Second step** — ...\n");
    s
}

/// Validate a parsed doc against the app's known view keys and agent tool names.
/// Returns human-readable problems (starting with the parse warnings).
pub fn validate(doc: &WorkflowDoc, known_views: &[&str], known_tools: &[String]) -> Vec<String> {
    let mut problems = doc.warnings.clone();
    for s in &doc.steps {
        if let Some(v) = &s.view {
            if !v.is_empty() && !known_views.contains(&v.as_str()) {
                problems.push(format!(
                    "Step {} (\"{}\"): unknown View \"{}\".",
                    s.index + 1,
                    s.title,
                    v
                ));
            }
        }
        for tool in &s.tools {
            if !known_tools.iter().any(|t| t == tool) {
                problems.push(format!(
                    "Step {} (\"{}\"): unknown Tool \"{}\".",
                    s.index + 1,
                    s.title,
                    tool
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Variable Star Photometry
> Measure a variable star's light curve.
Tags: photometry, time-series
Time: ~2 h

## Steps

- [x] **Find the field** — Locate the target.
      Tool: search_observations, resolve_target
      View: search
      Note: use the resolver
- [ ] **Download frames**
      Some body text.
";

    #[test]
    fn parses_title_description_metadata() {
        let doc = parse(SAMPLE);
        assert_eq!(doc.title, "Variable Star Photometry");
        assert_eq!(doc.description, "Measure a variable star's light curve.");
        assert_eq!(doc.tags(), vec!["photometry", "time-series"]);
        assert_eq!(doc.metadata_get("Time"), Some("~2 h"));
    }

    #[test]
    fn parses_steps_with_attachments() {
        let doc = parse(SAMPLE);
        assert_eq!(doc.steps.len(), 2);
        let s0 = &doc.steps[0];
        assert_eq!(s0.title, "Find the field");
        assert!(s0.done);
        assert_eq!(s0.tools, vec!["search_observations", "resolve_target"]);
        assert_eq!(s0.view.as_deref(), Some("search"));
        assert_eq!(s0.note.as_deref(), Some("use the resolver"));
        assert!(s0.body.contains("Locate the target."));
        let s1 = &doc.steps[1];
        assert_eq!(s1.title, "Download frames");
        assert!(!s1.done);
        assert_eq!(doc.done_count(), 1);
    }

    #[test]
    fn with_step_done_is_byte_preserving() {
        // Flip step 1 (0-based) to done; only its checkbox char changes.
        let flipped = with_step_done(SAMPLE, 1, true).unwrap();
        assert!(flipped.contains("- [x] **Download frames**"));
        // Step 0 untouched, and the author's exact spacing/text survives.
        assert!(flipped.contains("- [x] **Find the field** — Locate the target."));
        assert!(flipped.contains("      Tool: search_observations, resolve_target"));
        // Round-trip back to not-done.
        let back = with_step_done(&flipped, 0, false).unwrap();
        assert!(back.contains("- [ ] **Find the field**"));
    }

    #[test]
    fn with_step_done_out_of_range_errors() {
        assert!(with_step_done(SAMPLE, 9, true).is_err());
    }

    #[test]
    fn tolerant_parse_warns_but_does_not_fail() {
        let doc = parse("just some text with no structure");
        assert_eq!(doc.title, "Untitled workflow");
        assert!(doc.steps.is_empty());
        assert!(doc.warnings.len() >= 2);
    }

    #[test]
    fn skeleton_round_trips() {
        let doc = parse(&skeleton("My Protocol"));
        assert_eq!(doc.title, "My Protocol");
        assert_eq!(doc.steps.len(), 2);
        assert_eq!(doc.steps[0].view.as_deref(), Some("search"));
    }

    #[test]
    fn validate_flags_unknown_view_and_tool() {
        let text = "# T\n- [ ] **S**\n      View: nope\n      Tool: madeup\n";
        let doc = parse(text);
        let problems = validate(&doc, KNOWN_VIEWS, &["search_observations".to_string()]);
        assert!(problems.iter().any(|p| p.contains("unknown View")));
        assert!(problems.iter().any(|p| p.contains("unknown Tool")));
    }
}
