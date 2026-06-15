//! UI-only Markdown-lite styling for transcript text.
//!
//! Tau stores and sends prompts, responses, and reasoning as plain text. This
//! renderer is deliberately a terminal presentation layer: it converts a small
//! Markdown-like subset into semantic theme spans and never changes protocol
//! data, durable event logs, model context, or transcript copies.
//!
//! Supported syntax is intentionally small: ATX headings (`# Heading`),
//! unordered list markers (`-`, `*`, `+`), `*strong*`, `_emphasis_`, and
//! unordered (`-`, `*`, `+`) and ordered (`1.`) list markers, `*strong*`,
//! `_emphasis_`, and
//! source characters, including delimiters and list/header markers, so live
//! text keeps stable byte lengths and wrapping. Inline backtick spans, fenced
//! code blocks, and indented code-like lines use code styling and suppress
//! nested Markdown-lite styling. Escaped marker sequences use a separate escape
//! style so opt-outs remain visible.
//!
//! Live rendering uses [`MarkdownStreamCache`]. Blank lines seal earlier text;
//! sealed chunks are parsed once and cached, while the current unsealed suffix
//! is left plain until a future blank line makes it stable. Final/static
//! rendering parses the complete string immediately.

use tau_themes::{SpanTree, StyleIdx, StyleName, ThemedText, names};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FenceKind {
    Backticks,
    Tildes,
}

impl FenceKind {
    fn marker(self) -> &'static str {
        match self {
            Self::Backticks => "```",
            Self::Tildes => "~~~",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkdownStyle {
    Base,
    Strong,
    Emphasis,
    Heading,
    ListMarker,
    PromptMarker,
    Code,
    Escape,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarkdownRun {
    text: String,
    style: MarkdownStyle,
}

/// Append-aware cache for Markdown-lite live response/thinking rendering.
///
/// `source` is the latest full provider snapshot. `finalized_until` is a UTF-8
/// byte boundary into `source`; everything before it has been sealed by a blank
/// line, parsed exactly once, and stored in `finalized_runs`. `in_fence` is the
/// parser context after those sealed runs, so a fenced code block remains plain
/// even when blank lines inside it cause multiple sealed chunks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct MarkdownStreamCache {
    source: String,
    finalized_until: usize,
    finalized_runs: Vec<MarkdownRun>,
    in_fence: Option<FenceKind>,
}

/// Render final/static transcript text with Markdown-lite semantic styles.
pub(crate) fn markdown_block(
    theme: &tau_themes::Theme,
    base_style_name: &str,
    text: &str,
) -> tau_cli_term::StyledBlock {
    let mut in_fence = None;
    styled_block_from_runs(
        theme,
        base_style_name,
        &[],
        &parse_markdown_with_state(text, &mut in_fence),
        false,
    )
}

/// Render a submitted prompt marker followed by Markdown-lite prompt text.
pub(crate) fn markdown_prompt_block(
    theme: &tau_themes::Theme,
    base_style_name: &str,
    marker_text: String,
    text: &str,
) -> tau_cli_term::StyledBlock {
    let prefix = [MarkdownRun {
        text: marker_text,
        style: MarkdownStyle::PromptMarker,
    }];
    let mut in_fence = None;
    styled_block_from_runs(
        theme,
        base_style_name,
        &prefix,
        &parse_markdown_with_state(text, &mut in_fence),
        false,
    )
}

/// Render live append-only text with sealed paragraphs formatted and the live
/// suffix left plain until a following empty line makes it stable.
pub(crate) fn markdown_streaming_block(
    theme: &tau_themes::Theme,
    base_style_name: &str,
    text: &str,
    cache: &mut MarkdownStreamCache,
) -> tau_cli_term::StyledBlock {
    if !text.starts_with(&cache.source) {
        cache.source.clear();
        cache.finalized_until = 0;
        cache.finalized_runs.clear();
        cache.in_fence = None;
    }

    let sealed_until = latest_sealed_boundary(text).unwrap_or(0);
    if cache.finalized_until < sealed_until {
        cache.finalized_runs.extend(parse_markdown_with_state(
            &text[cache.finalized_until..sealed_until],
            &mut cache.in_fence,
        ));
        cache.finalized_until = sealed_until;
    }
    cache.source = text.to_owned();

    let mut runs = cache.finalized_runs.clone();
    if cache.finalized_until < text.len() {
        runs.push(MarkdownRun {
            text: text[cache.finalized_until..].to_owned(),
            style: MarkdownStyle::Base,
        });
    }
    styled_block_from_runs(theme, base_style_name, &[], &runs, true)
}

fn styled_block_from_runs(
    theme: &tau_themes::Theme,
    base_style_name: &str,
    prefix: &[MarkdownRun],
    runs: &[MarkdownRun],
    progress: bool,
) -> tau_cli_term::StyledBlock {
    use tau_cli_term::resolve::{convert_color, themed_text};

    let mut themed = ThemedText::new();
    let base = themed.add_style(base_style_name);
    let strong = themed.add_style(names::MARKDOWN_STRONG);
    let emphasis = themed.add_style(names::MARKDOWN_EMPHASIS);
    let heading = themed.add_style(names::MARKDOWN_HEADING);
    let list_marker = themed.add_style(names::MARKDOWN_LIST_MARKER);
    let prompt_marker = themed.add_style(names::PROMPT_MARKER_SUBMITTED);
    let code = themed.add_style(names::MARKDOWN_CODE);
    let escape = themed.add_style(names::MARKDOWN_ESCAPE);
    let progress_style = themed.add_style(names::PROGRESS_INDICATOR);

    let mut body_children = Vec::new();
    let styles = MarkdownStyleIndexes {
        strong,
        emphasis,
        heading,
        list_marker,
        prompt_marker,
        code,
        escape,
    };
    push_runs(&mut body_children, prefix, styles);
    push_runs(&mut body_children, runs, styles);

    let needs_space = progress && body_children_text_ends_non_whitespace(prefix, runs);

    let mut root_children = Vec::new();
    if !body_children.is_empty() {
        root_children.push(SpanTree::span(base, body_children));
    }
    if progress {
        if needs_space {
            root_children.push(SpanTree::span(base, vec![SpanTree::text(" ")]));
        }
        root_children.push(SpanTree::span(
            progress_style,
            vec![SpanTree::text(tau_proto::PROGRESS_INDICATOR_TEXT)],
        ));
    }
    themed.push_tree(SpanTree::span(StyleIdx::DEFAULT, root_children));

    let body_ts = theme.resolve_style(&StyleName::new(base_style_name));
    let mut block = tau_cli_term::StyledBlock::new(themed_text(theme, &themed));
    if let Some(bg) = body_ts.bg {
        block = block.bg(convert_color(bg));
    }
    block
}

fn body_children_text_ends_non_whitespace(prefix: &[MarkdownRun], runs: &[MarkdownRun]) -> bool {
    runs_end_non_whitespace(runs) || runs_end_non_whitespace(prefix)
}

fn runs_end_non_whitespace(runs: &[MarkdownRun]) -> bool {
    runs.iter()
        .rev()
        .find(|run| !run.text.is_empty())
        .and_then(|run| run.text.chars().next_back())
        .is_some_and(|c| !c.is_whitespace())
}

#[derive(Clone, Copy)]
struct MarkdownStyleIndexes {
    strong: StyleIdx,
    emphasis: StyleIdx,
    heading: StyleIdx,
    list_marker: StyleIdx,
    prompt_marker: StyleIdx,
    code: StyleIdx,
    escape: StyleIdx,
}

fn push_runs(
    children: &mut Vec<SpanTree<StyleIdx>>,
    runs: &[MarkdownRun],
    styles: MarkdownStyleIndexes,
) {
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        match run.style {
            MarkdownStyle::Base => children.push(SpanTree::text(run.text.clone())),
            MarkdownStyle::Strong => {
                children.push(SpanTree::span(
                    styles.strong,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
            MarkdownStyle::Emphasis => {
                children.push(SpanTree::span(
                    styles.emphasis,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
            MarkdownStyle::Heading => {
                children.push(SpanTree::span(
                    styles.heading,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
            MarkdownStyle::ListMarker => {
                children.push(SpanTree::span(
                    styles.list_marker,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
            MarkdownStyle::PromptMarker => {
                children.push(SpanTree::span(
                    styles.prompt_marker,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
            MarkdownStyle::Code => {
                children.push(SpanTree::span(
                    styles.code,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
            MarkdownStyle::Escape => {
                children.push(SpanTree::span(
                    styles.escape,
                    vec![SpanTree::text(run.text.clone())],
                ));
            }
        }
    }
}

fn latest_sealed_boundary(text: &str) -> Option<usize> {
    let mut offset = 0;
    let mut latest = None;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if line.ends_with('\n') && line.trim().is_empty() {
            latest = Some(offset);
        }
    }
    latest
}

fn parse_markdown_with_state(text: &str, in_fence: &mut Option<FenceKind>) -> Vec<MarkdownRun> {
    let mut runs = Vec::new();
    for line in text.split_inclusive('\n') {
        let (body, newline) = line
            .strip_suffix('\n')
            .map_or((line, ""), |body| (body, "\n"));
        let trimmed = body.trim_start();
        if let Some(fence) = *in_fence {
            push_run(&mut runs, body, MarkdownStyle::Code);
            push_run(&mut runs, newline, MarkdownStyle::Base);
            if trimmed.starts_with(fence.marker()) {
                *in_fence = None;
            }
            continue;
        }
        if let Some(fence) = fence_marker(trimmed) {
            push_run(&mut runs, body, MarkdownStyle::Code);
            push_run(&mut runs, newline, MarkdownStyle::Base);
            *in_fence = Some(fence);
            continue;
        }
        if is_heading(body) {
            push_run(&mut runs, body, MarkdownStyle::Heading);
            push_run(&mut runs, newline, MarkdownStyle::Base);
            continue;
        }
        if let Some((indent_end, marker_end)) = list_marker_range(body) {
            push_run(&mut runs, &body[..indent_end], MarkdownStyle::Base);
            push_run(
                &mut runs,
                &body[indent_end..marker_end],
                MarkdownStyle::ListMarker,
            );
            parse_inline(&body[marker_end..], &mut runs);
            push_run(&mut runs, newline, MarkdownStyle::Base);
            continue;
        }
        if is_indented_code(body) {
            push_run(&mut runs, body, MarkdownStyle::Code);
            push_run(&mut runs, newline, MarkdownStyle::Base);
            continue;
        }
        parse_inline(body, &mut runs);
        push_run(&mut runs, newline, MarkdownStyle::Base);
    }
    runs
}

fn is_heading(line: &str) -> bool {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ')
}

fn list_marker_range(line: &str) -> Option<(usize, usize)> {
    let indent_end = line
        .char_indices()
        .find(|(_, c)| !matches!(c, ' ' | '\t'))
        .map_or(line.len(), |(idx, _)| idx);
    let rest = &line[indent_end..];
    let bytes = rest.as_bytes();
    let marker = bytes.first().copied()?;
    if matches!(marker, b'-' | b'*' | b'+') && bytes.get(1) == Some(&b' ') {
        return Some((indent_end, indent_end + 1));
    }

    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    if matches!(bytes.get(digits), Some(b'.' | b')')) && bytes.get(digits + 1) == Some(&b' ') {
        return Some((indent_end, indent_end + digits + 1));
    }
    None
}

fn is_indented_code(line: &str) -> bool {
    line.starts_with('\t') || line.starts_with("    ")
}

fn fence_marker(trimmed: &str) -> Option<FenceKind> {
    if trimmed.starts_with("```") {
        Some(FenceKind::Backticks)
    } else if trimmed.starts_with("~~~") {
        Some(FenceKind::Tildes)
    } else {
        None
    }
}

fn parse_inline(text: &str, runs: &mut Vec<MarkdownRun>) {
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if let Some(ch) = rest.chars().next() {
            if ch == '\\' && escaped_len(rest).is_some() {
                let len = escaped_len(rest).expect("checked escape");
                push_run(runs, &rest[..len], MarkdownStyle::Escape);
                index += len;
                continue;
            }
            if ch == '`'
                && let Some(end) = find_unescaped(&rest[1..], '`')
            {
                let len = 1 + end + 1;
                push_run(runs, &rest[..len], MarkdownStyle::Code);
                index += len;
                continue;
            }
            if matches!(ch, '*' | '_')
                && delimiter_allowed(text, index, ch)
                && let Some(end) = find_closing_delimiter(text, index, ch)
            {
                let style = if ch == '*' {
                    MarkdownStyle::Strong
                } else {
                    MarkdownStyle::Emphasis
                };
                push_run(runs, &text[index..end], style);
                index = end;
                continue;
            }
            let next = index + ch.len_utf8();
            push_run(runs, &text[index..next], MarkdownStyle::Base);
            index = next;
        }
    }
}

fn escaped_len(rest: &str) -> Option<usize> {
    let mut chars = rest.chars();
    (chars.next() == Some('\\'))
        .then_some(chars.next()?)
        .filter(|c| matches!(c, '*' | '_' | '#' | '-' | '\\' | '`'))
        .map(|c| 1 + c.len_utf8())
}

fn find_unescaped(text: &str, needle: char) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == needle {
            return Some(idx);
        }
    }
    None
}

fn find_closing_delimiter(text: &str, start: usize, delimiter: char) -> Option<usize> {
    let after_open = start + delimiter.len_utf8();
    let rest = &text[after_open..];
    for (relative, ch) in rest.char_indices() {
        if ch != delimiter {
            continue;
        }
        let close = after_open + relative;
        if delimiter_allowed(text, close, delimiter) && after_open < close {
            return Some(close + delimiter.len_utf8());
        }
    }
    None
}

fn delimiter_allowed(text: &str, index: usize, delimiter: char) -> bool {
    if delimiter != '_' {
        return true;
    }
    let previous = text[..index].chars().next_back();
    let next = text[index + delimiter.len_utf8()..].chars().next();
    !(previous.is_some_and(|c| c.is_alphanumeric()) && next.is_some_and(|c| c.is_alphanumeric()))
}

fn push_run(runs: &mut Vec<MarkdownRun>, text: &str, style: MarkdownStyle) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut()
        && last.style == style
    {
        last.text.push_str(text);
        return;
    }
    runs.push(MarkdownRun {
        text: text.to_owned(),
        style,
    });
}

#[cfg(test)]
mod tests;
