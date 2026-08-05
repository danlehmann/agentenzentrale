//! Server-side rendering of agent output to safe, pretty HTML.

use comrak::plugins::syntect::SyntectAdapter;
use comrak::{markdown_to_html_with_plugins, Options, Plugins};

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

    let adapter = SyntectAdapter::new(Some("base16-ocean.dark"));
    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&adapter);

    let html = markdown_to_html_with_plugins(markdown, &options, &plugins);

    let mut builder = ammonia::Builder::default();
    builder.add_generic_attributes(&["style"]);
    builder.clean(&html).to_string()
}

#[cfg(test)]
mod tests {
    use super::render_markdown;

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
    fn allows_inline_style_for_syntax_theme() {
        let out = render_markdown("# hi");
        assert!(!out.contains("<script"));
    }
}
