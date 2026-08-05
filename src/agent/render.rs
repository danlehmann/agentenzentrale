//! Server-side rendering of agent output to safe, pretty HTML.

use comrak::plugins::syntect::SyntectAdapter;
use comrak::{markdown_to_html_with_plugins, Options, Plugins};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::agent::SessionMessage;

/// Hard cap on rendered tool output per call (prevents multi-MB pages).
const TOOL_OUTPUT_CAP: usize = 8000;
const TOOL_DESC_CAP: usize = 140;

/// A shared, long-lived syntax highlighter. Building one is expensive (it loads
/// the default syntax + theme sets), so it is created exactly once per process
/// instead of per message.
fn adapter() -> &'static SyntectAdapter {
    static ADAPTER: std::sync::OnceLock<SyntectAdapter> = std::sync::OnceLock::new();
    ADAPTER.get_or_init(|| SyntectAdapter::new(Some("base16-ocean.dark")))
}

/// Render a markdown string to sanitized HTML with syntax-highlighted code.
///
/// Security: output is run through `ammonia` to strip scripts, event handlers,
/// and dangerous URL schemes before it ever reaches a browser. The `style`
/// attribute is the only relaxation (needed for the syntect theme); it cannot
/// execute code.
pub fn render_markdown(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.tagfilter = true;
    options.extension.superscript = true;
    options.extension.description_lists = true;
    options.extension.footnotes = true;
    options.render.unsafe_ = false;

    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(adapter());

    let html = markdown_to_html_with_plugins(markdown, &options, &plugins);

    let mut builder = ammonia::Builder::default();
    builder.add_generic_attributes(&["style"]);
    builder.clean(&html).to_string()
}

/// Render a full message to HTML: markdown for `text` parts, collapsible
/// blocks for `tool` parts, and skips noise parts (reasoning, step markers).
/// Returns an empty string if the message has nothing user-visible (so
/// callers can drop tool-only/empty turns).
pub fn render_message(msg: &SessionMessage) -> String {
    let mut out = String::new();
    for part in &msg.parts {
        let t = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match t {
            "text" => {
                if let Some(txt) = part.get("text").and_then(|v| v.as_str()) {
                    if !txt.trim().is_empty() {
                        out.push_str(&render_markdown(txt));
                    }
                }
            }
            "tool" => out.push_str(&render_tool(part)),
            _ => {} // reasoning, step-start, step-finish, compaction: hidden
        }
    }
    out
}

/// A short, human-readable descriptor for a tool call (its arguments).
fn tool_desc(name: &str, input: Option<&Value>) -> String {
    let pick = |k: &str| input.and_then(|v| v.get(k)).and_then(|v| v.as_str());
    let candidate = match name {
        "bash" => pick("command").map(|c| format!("$ {c}")),
        "read" | "write" | "edit" => pick("filePath").map(ToOwned::to_owned),
        "glob" | "grep" => pick("pattern").or_else(|| pick("query")).map(ToOwned::to_owned),
        "webfetch" => pick("url").map(ToOwned::to_owned),
        "task" | "todowrite" => pick("description").map(ToOwned::to_owned),
        "skill" => pick("name").map(ToOwned::to_owned),
        "question" => pick("question").map(ToOwned::to_owned),
        _ => None,
    };
    match candidate {
        Some(d) => truncate(&d, TOOL_DESC_CAP),
        None => name.to_string(),
    }
}

/// Render one `tool` part as a collapsible `<details>` block.
fn render_tool(part: &Value) -> String {
    let name = part
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("tool")
        .to_string();
    let state = part.get("state").cloned().unwrap_or_else(|| Value::Null);
    let status = state
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let input = state.get("input");
    let output = state
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let desc = tool_desc(&name, input);
    let err = status == "error";
    let status_cls = if err { "err" } else { "ok" };
    let shown = truncate(&output, TOOL_OUTPUT_CAP);

    let mut summary = format!(
        "<span class=\"tool-name\">{}</span> <code class=\"tool-desc\">{}</code>",
        html_escape(&name),
        html_escape(&desc)
    );
    if !status.is_empty() {
        summary.push_str(&format!(
            " <span class=\"tool-status {status_cls}\">{}</span>",
            html_escape(&status)
        ));
    }

    // File edits render an inline unified diff (old vs new) instead of the
    // terse "Edit applied successfully" output.
    let content = if name == "edit" {
        let old = input.and_then(|v| v.get("oldString")).and_then(|v| v.as_str());
        let new = input.and_then(|v| v.get("newString")).and_then(|v| v.as_str());
        match (old, new) {
            (Some(o), Some(n)) if o != n => Some(render_diff(o, n)),
            _ => tool_content(&shown),
        }
    } else {
        tool_content(&shown)
    };

    let body = match content {
        Some(c) => format!("<details class=\"tool\"><summary>{summary}</summary>{c}</details>"),
        None => format!("<details class=\"tool\"><summary>{summary}</summary></details>"),
    };
    body
}

fn tool_content(shown: &str) -> Option<String> {
    if shown.is_empty() {
        None
    } else {
        Some(format!(
            "<pre class=\"tool-out\">{}</pre>",
            html_escape(shown)
        ))
    }
}

/// Render a unified diff between two strings as +/- colored lines.
fn render_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::from("<pre class=\"diff\">");
    for change in diff.iter_all_changes() {
        let (sign, cls) = match change.tag() {
            ChangeTag::Delete => ('-', "d-del"),
            ChangeTag::Insert => ('+', "d-add"),
            ChangeTag::Equal => (' ', "d"),
        };
        out.push_str(&format!(
            "<span class=\"{cls}\">{sign}{}</span>",
            html_escape(change.value())
        ));
    }
    out.push_str("</pre>");
    out
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(cap).collect();
        t.push_str("\n… (truncated)");
        t
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::render_markdown;
    use crate::agent::SessionMessage;
    use serde_json::json;
    use crate::agent::render::render_message;

    fn msg(parts: Vec<serde_json::Value>) -> SessionMessage {
        SessionMessage { info: json!({"role": "assistant"}), parts }
    }

    #[test]
    fn strips_script_tags() {
        let out = render_markdown("hello <script>alert(1)</script>");
        assert!(!out.contains("<script"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn strips_on_handlers() {
        let out = render_markdown("<img src=x onerror=alert(1)>");
        assert!(!out.contains("onerror"));
    }

    #[test]
    fn strips_javascript_urls() {
        let out = render_markdown("[click](javascript:alert(1))");
        assert!(!out.contains("javascript:"));
    }

    #[test]
    fn renders_basic_markdown() {
        let out = render_markdown("**bold** and `code`");
        assert!(out.contains("<strong>bold</strong>"));
        assert!(out.contains("<code>code</code>"));
    }

    #[test]
    fn highlights_code_blocks() {
        let out = render_markdown("```rust\nfn main() {}\n```");
        assert!(out.contains("<pre"));
        assert!(out.contains("main"));
    }

    #[test]
    fn tool_part_renders_collapsible() {
        let m = msg(vec![json!({
            "type": "tool", "tool": "bash",
            "state": {"status": "completed", "input": {"command": "cargo test"}, "output": "ok"}
        })]);
        let out = render_message(&m);
        assert!(out.contains("<details class=\"tool\">"));
        assert!(out.contains("tool-name\">bash"));
        assert!(out.contains("$ cargo test"));
        assert!(out.contains("tool-status ok"));
        assert!(out.contains("ok"));
    }

    #[test]
    fn tool_error_status() {
        let m = msg(vec![json!({
            "type": "tool", "tool": "read",
            "state": {"status": "error", "input": {"filePath": "a/b.rs"}, "output": ""}
        })]);
        let out = render_message(&m);
        assert!(out.contains("tool-status err"));
        assert!(out.contains("a/b.rs"));
    }

    #[test]
    fn tool_output_is_escaped() {
        let m = msg(vec![json!({
            "type": "tool", "tool": "bash",
            "state": {"status": "completed", "input": {"command": "ls"}, "output": "<script>alert(1)</script>"}
        })]);
        let out = render_message(&m);
        assert!(!out.contains("<script>"));
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn reasoning_only_message_is_empty() {
        let m = msg(vec![json!({"type": "reasoning", "text": "thinking"})]);
        assert_eq!(render_message(&m), "");
    }

    #[test]
    fn text_and_tool_both_render() {
        let m = msg(vec![
            json!({"type": "text", "text": "**result**"}),
            json!({"type": "tool", "tool": "bash", "state": {"status": "completed", "input": {"command": "x"}, "output": "y"}}),
        ]);
        let out = render_message(&m);
        assert!(out.contains("<strong>result</strong>"));
        assert!(out.contains("<details class=\"tool\">"));
    }

    #[test]
    fn edit_tool_renders_diff() {
        let m = msg(vec![json!({
            "type": "tool", "tool": "edit",
            "state": {"status": "completed",
                      "input": {"filePath": "a/x.rs", "oldString": "foo\nbar\n", "newString": "foo\nbaz\n"}}
        })]);
        let out = render_message(&m);
        assert!(out.contains("class=\"diff\""));
        assert!(out.contains("d-del"));
        assert!(out.contains("d-add"));
        assert!(!out.contains("tool-out"));
    }

    #[test]
    fn edit_without_change_uses_plain_output() {
        let m = msg(vec![json!({
            "type": "tool", "tool": "edit",
            "state": {"status": "completed", "input": {"filePath": "a/x.rs"}, "output": "Edit applied successfully."}
        })]);
        let out = render_message(&m);
        assert!(out.contains("class=\"tool-out\""));
        assert!(!out.contains("class=\"diff\""));
    }
}
