use super::*;

fn markdown_test_theme() -> tau_themes::Theme {
    tau_themes::Theme::parse(
        r##"{
            styles: {
                "shell.output": { },
                "user.prompt": { },
                "prompt.marker.submitted": { fg: "red" },
                "markdown.strong": { bold: true },
                "markdown.emphasis": { italic: true },
                "markdown.heading": { underline: true },
                "markdown.list.marker": { fg: "green" },
                "markdown.code": { bg: "#111111" },
                "markdown.escape": { bg: "#222222" },
                "progress.indicator": { fg: "cyan" },
            }
        }"##,
    )
    .expect("valid markdown test theme")
}

fn rendered_text(block: &tau_cli_term::StyledBlock) -> String {
    block
        .content
        .spans()
        .iter()
        .map(|span| span.text.as_str())
        .collect()
}

/// Ensures Markdown-lite is style-only and preserves source text exactly.
#[test]
fn final_render_preserves_source_text() {
    let theme = tau_themes::Theme::builtin();
    let block = markdown_block(
        &theme,
        names::USER_PROMPT,
        "# Title\n- *bold* and _italics_\n",
    );

    assert_eq!(rendered_text(&block), "# Title\n- *bold* and _italics_\n");
}

/// Ensures headings, list markers, strong, and emphasis map to semantic
/// theme attributes.
#[test]
fn final_render_applies_markdown_styles() {
    let theme = markdown_test_theme();
    let block = markdown_block(
        &theme,
        names::SHELL_OUTPUT,
        "# Title\n- *bold* and _italics_",
    );
    let spans = block.content.spans();

    let heading = spans.iter().find(|span| span.text == "# Title").unwrap();
    assert!(heading.style.underline);

    let marker = spans.iter().find(|span| span.text == "-").unwrap();
    assert_eq!(marker.style.fg, Some(tau_cli_term::Color::Green));

    let strong = spans.iter().find(|span| span.text == "*bold*").unwrap();
    assert!(strong.style.bold);

    let emphasis = spans.iter().find(|span| span.text == "_italics_").unwrap();
    assert!(!emphasis.style.bold);
    assert!(emphasis.style.italic);
}

/// Ensures nested ordered list markers are list markers instead of indented
/// code.
#[test]
fn nested_ordered_list_items_are_not_indented_code() {
    let theme = markdown_test_theme();
    let block = markdown_block(
        &theme,
        names::SHELL_OUTPUT,
        "1. Parent item\n   - Child bullet\n     1. Nested numbered item\n     2. Another nested numbered item\n2. Second parent item",
    );
    let spans = block.content.spans();

    for marker in ["1.", "-", "2."] {
        let span = spans
            .iter()
            .find(|span| span.text == marker)
            .unwrap_or_else(|| panic!("missing marker span {marker}"));
        assert_eq!(span.style.fg, Some(tau_cli_term::Color::Green));
        assert_eq!(span.style.bg, None);
    }

    let nested_body = spans
        .iter()
        .find(|span| span.text.contains("Nested numbered item"))
        .expect("nested ordered item body");
    assert_eq!(nested_body.style.bg, None);
}

/// Ensures unmatched, escaped, identifier, and code-like delimiters do not
/// style accidentally.
#[test]
fn inline_parser_avoids_common_false_positives() {
    let theme = markdown_test_theme();
    let block = markdown_block(
        &theme,
        names::SHELL_OUTPUT,
        "foo_bar_baz \\*literal\\* `*code*`\n```\n*code*\n```",
    );

    let spans = block.content.spans();
    for span in spans {
        assert!(!span.style.bold, "unexpected bold span: {span:?}");
        assert!(!span.style.italic, "unexpected italic span: {span:?}");
    }
    let escape = spans
        .iter()
        .find(|span| span.text == "\\*")
        .expect("escaped marker span");
    assert!(escape.style.bg.is_some());

    let inline_code = spans
        .iter()
        .find(|span| span.text == "`*code*`")
        .expect("inline code span");
    assert!(inline_code.style.bg.is_some());
}

/// Ensures live rendering leaves the unsealed suffix plain until a blank
/// line seals it.
#[test]
fn live_stream_formats_only_sealed_paragraphs() {
    let theme = markdown_test_theme();
    let mut cache = MarkdownStreamCache::default();

    let block = markdown_streaming_block(&theme, names::SHELL_OUTPUT, "*bold*", &mut cache);
    let bold = block
        .content
        .spans()
        .iter()
        .find(|span| span.text == "*bold*")
        .unwrap();
    assert!(!bold.style.bold);

    let block = markdown_streaming_block(&theme, names::SHELL_OUTPUT, "*bold*\n\nnext", &mut cache);
    let bold = block
        .content
        .spans()
        .iter()
        .find(|span| span.text == "*bold*")
        .unwrap();
    assert!(bold.style.bold);
    let next = block
        .content
        .spans()
        .iter()
        .find(|span| span.text == "next")
        .unwrap();
    assert!(!next.style.bold);
}

/// Ensures non-append provider replacements reset the streaming cache
/// safely.
#[test]
fn live_stream_cache_resets_on_replacement() {
    let theme = markdown_test_theme();
    let mut cache = MarkdownStreamCache::default();
    let _ = markdown_streaming_block(&theme, names::SHELL_OUTPUT, "*old*\n\n", &mut cache);
    let block = markdown_streaming_block(&theme, names::SHELL_OUTPUT, "_new_\n\n", &mut cache);

    assert_eq!(rendered_text(&block), "_new_\n\n…");
    let emphasis = block
        .content
        .spans()
        .iter()
        .find(|span| span.text == "_new_")
        .unwrap();
    assert!(!emphasis.style.bold);
    assert!(emphasis.style.italic);
}

/// Ensures submitted prompt prefixes keep prompt-marker semantics instead
/// of inheriting the Markdown list-marker style.
#[test]
fn prompt_marker_uses_submitted_marker_style() {
    let theme = markdown_test_theme();
    let block = markdown_prompt_block(&theme, names::USER_PROMPT, "> ".to_owned(), "- item");
    let spans = block.content.spans();

    let prompt_marker = spans.iter().find(|span| span.text == "> ").unwrap();
    assert_eq!(prompt_marker.style.fg, Some(tau_cli_term::Color::Red));

    let list_marker = spans.iter().find(|span| span.text == "-").unwrap();
    assert_eq!(list_marker.style.fg, Some(tau_cli_term::Color::Green));
}

/// Ensures the live cache carries fenced-code parser state across sealed
/// chunks split by blank lines inside the fence.
#[test]
fn live_stream_preserves_fence_state_across_blank_lines() {
    let theme = markdown_test_theme();
    let mut cache = MarkdownStreamCache::default();
    let _ = markdown_streaming_block(&theme, names::SHELL_OUTPUT, "```\n\n", &mut cache);
    let block = markdown_streaming_block(
        &theme,
        names::SHELL_OUTPUT,
        "```\n\n*not bold*\n\n",
        &mut cache,
    );
    let code = block
        .content
        .spans()
        .iter()
        .find(|span| span.text.contains("*not bold*"))
        .expect("code text span after second update");
    assert!(!code.style.bold);

    let block = markdown_streaming_block(
        &theme,
        names::SHELL_OUTPUT,
        "```\n\n*not bold*\n\n```\n\n*bold*\n\n",
        &mut cache,
    );

    let code = block
        .content
        .spans()
        .iter()
        .find(|span| span.text.contains("*not bold*"))
        .expect("code text span");
    assert!(!code.style.bold);

    let bold = block
        .content
        .spans()
        .iter()
        .find(|span| span.text == "*bold*")
        .expect("post-fence bold span");
    assert!(bold.style.bold);
}
