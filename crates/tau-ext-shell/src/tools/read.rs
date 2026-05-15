//! `read` tool: read a file (optionally a line slice).

use std::fs;
use std::path::{Path, PathBuf};

use tau_proto::CborValue;

use crate::argument::{argument_text, optional_argument_int};
use crate::display::{ToolFailure, ToolOutput, ok_display, text_stats};
use crate::truncate::{MAX_OUTPUT_BYTES, MAX_OUTPUT_LINES};

pub(crate) fn read_file(arguments: &CborValue) -> Result<ToolOutput, ToolFailure> {
    let path = argument_text(arguments, "path").map_err(ToolFailure::from)?;
    let start_line_arg = optional_argument_int(arguments, "start_line");
    let line_count_arg = optional_argument_int(arguments, "line_count");
    let start_line = parse_read_start_line(start_line_arg)?;
    let line_count = parse_read_line_count(line_count_arg)?;
    let path_buf = PathBuf::from(&path);
    let display_path = path_buf.display().to_string();
    let range = format_read_range(start_line_arg.map(|_| start_line), line_count);
    let display_args = format!("{display_path} {range}");

    let file_bytes = fs::metadata(&path_buf)
        .map_err(|error| ToolFailure::from(error.to_string()).with_args(display_args.clone()))?
        .len() as usize;
    let sliced = stream_slice_lines(&path_buf, start_line, line_count)
        .map_err(|error| ToolFailure::from(error.to_string()).with_args(display_args.clone()))?;
    if sliced.total_lines != 0 && start_line > sliced.total_lines {
        return Err(ToolFailure::new(format!(
            "start_line {start_line} is past end of file (total_lines: {})",
            sliced.total_lines
        ))
        .with_args(display_args));
    }
    let total_lines = sliced.total_lines;
    let truncated = truncate_read_content(&sliced.content, start_line, total_lines, file_bytes);
    let content_value = CborValue::Text(truncated.content.clone());
    let returned_line_count = truncated.line_count;
    debug_assert!(returned_line_count <= sliced.line_count);
    let mut entries = vec![
        (
            CborValue::Text("path".to_owned()),
            CborValue::Text(display_path),
        ),
        (
            CborValue::Text("line-numbered content".to_owned()),
            content_value,
        ),
        (
            CborValue::Text("start_line".to_owned()),
            CborValue::Integer((sliced.start_line as i64).into()),
        ),
        (
            CborValue::Text("line_count".to_owned()),
            CborValue::Integer((returned_line_count as i64).into()),
        ),
        (
            CborValue::Text("total_lines".to_owned()),
            CborValue::Integer((total_lines as i64).into()),
        ),
        (
            CborValue::Text("ends_with_newline".to_owned()),
            CborValue::Bool(sliced.ends_with_newline),
        ),
        (
            CborValue::Text("line_ending".to_owned()),
            CborValue::Text(sliced.line_ending.clone()),
        ),
        (
            CborValue::Text("valid_utf8".to_owned()),
            CborValue::Bool(sliced.valid_utf8),
        ),
        (
            CborValue::Text("total_bytes".to_owned()),
            CborValue::Integer((file_bytes as i64).into()),
        ),
    ];
    if truncated.was_truncated {
        entries.push((
            CborValue::Text("truncated".to_owned()),
            CborValue::Bool(true),
        ));
    }
    let mut display = ok_display(display_args);
    display.stats = text_stats(&truncated.content);
    Ok(ToolOutput {
        result: CborValue::Map(entries),
        display,
    })
}

pub(crate) struct ReadSlice {
    pub(crate) content: String,
    pub(crate) start_line: usize,
    pub(crate) line_count: usize,
    pub(crate) ends_with_newline: bool,
    pub(crate) line_ending: String,
    pub(crate) valid_utf8: bool,
    /// Total lines in the source. For [`stream_slice_lines`] this is
    /// computed by scanning the rest of the file after the slice ends.
    pub(crate) total_lines: usize,
}

struct TruncatedRead {
    content: String,
    was_truncated: bool,
    line_count: usize,
}

fn truncate_read_content(
    content: &str,
    start_line: usize,
    total_lines: usize,
    file_bytes: usize,
) -> TruncatedRead {
    let total_rendered_lines = content.lines().count();
    let total_rendered_bytes = content.len();
    let mut was_truncated = false;
    let mut rendered = String::new();
    let mut rendered_bytes = 0usize;
    let mut line_count = 0usize;

    if total_rendered_lines <= MAX_OUTPUT_LINES && total_rendered_bytes <= MAX_OUTPUT_BYTES {
        rendered.push_str(content);
        line_count = total_rendered_lines;
    } else {
        was_truncated = true;
        for (line_index, line) in content.lines().enumerate() {
            if line_count >= MAX_OUTPUT_LINES {
                break;
            }
            let separator_bytes = usize::from(line_index != 0);
            if rendered_bytes + separator_bytes >= MAX_OUTPUT_BYTES {
                break;
            }
            let remaining = MAX_OUTPUT_BYTES - rendered_bytes - separator_bytes;
            if line.len() + separator_bytes <= remaining {
                if line_index != 0 {
                    rendered.push('\n');
                    rendered_bytes += 1;
                }
                rendered.push_str(line);
                rendered_bytes += line.len();
                line_count += 1;
            } else {
                let prefix = utf8_prefix(line, remaining);
                if !prefix.is_empty() {
                    if line_index != 0 {
                        rendered.push('\n');
                    }
                    rendered.push_str(&mark_line_truncated(prefix));
                    line_count += 1;
                }
                break;
            }
        }
    }

    if was_truncated {
        let end_line = if line_count == 0 {
            start_line.saturating_sub(1)
        } else {
            start_line.saturating_add(line_count).saturating_sub(1)
        };
        let continuation = if rendered.contains("(truncated)") {
            "Line was truncated by byte cap; line-based continuation cannot resume within a line."
        } else {
            "Use start_line and line_count to continue reading."
        };
        rendered.push_str(&format!(
            "\n\n[Showing lines {start_line}-{end_line} of {total_lines} ({file_bytes} bytes total). \
             {continuation}]"
        ));
    }

    TruncatedRead {
        content: rendered,
        was_truncated,
        line_count,
    }
}

fn utf8_prefix(input: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

fn mark_line_truncated(line: &str) -> String {
    if let Some((line_number, rest)) = line.split_once(' ') {
        format!("{line_number}(truncated) {rest}...")
    } else {
        format!("{line}(truncated) ...")
    }
}

/// Stream `[start_line, start_line+count)` from `path` without
/// slurping the whole file. Continues reading past the slice end only
/// to count remaining lines (so the caller can report `total_lines`
/// for a "showing N of M" hint).
fn stream_slice_lines(
    path: &Path,
    start_line: usize,
    line_count: Option<usize>,
) -> std::io::Result<ReadSlice> {
    let bytes = fs::read(path)?;
    let take = line_count.unwrap_or(usize::MAX);
    let mut state = SliceState::new(start_line, take);

    let mut line_start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                let is_crlf = index + 1 < bytes.len() && bytes[index + 1] == b'\n';
                let ending = if is_crlf {
                    LineEndingKind::Crlf
                } else {
                    LineEndingKind::Cr
                };
                state.push_line(&bytes[line_start..index], Some(ending));
                index += if is_crlf { 2 } else { 1 };
                line_start = index;
            }
            b'\n' => {
                state.push_line(&bytes[line_start..index], Some(LineEndingKind::Lf));
                index += 1;
                line_start = index;
            }
            _ => index += 1,
        }
    }

    if line_start < bytes.len() {
        state.push_line(&bytes[line_start..], None);
    }

    Ok(state.finish())
}

struct SliceState {
    content: String,
    start_line: usize,
    take: usize,
    kept: usize,
    total_lines: usize,
    saw_lf: bool,
    saw_crlf: bool,
    saw_cr: bool,
    ends_with_newline: bool,
    valid_utf8: bool,
}

#[derive(Clone, Copy)]
enum LineEndingKind {
    Lf,
    Crlf,
    Cr,
}

impl SliceState {
    fn new(start_line: usize, take: usize) -> Self {
        Self {
            content: String::new(),
            start_line,
            take,
            kept: 0,
            total_lines: 0,
            saw_lf: false,
            saw_crlf: false,
            saw_cr: false,
            ends_with_newline: false,
            valid_utf8: true,
        }
    }

    fn push_line(&mut self, line: &[u8], ending: Option<LineEndingKind>) {
        self.total_lines += 1;
        self.ends_with_newline = ending.is_some();
        match ending {
            Some(LineEndingKind::Lf) => self.saw_lf = true,
            Some(LineEndingKind::Crlf) => self.saw_crlf = true,
            Some(LineEndingKind::Cr) => self.saw_cr = true,
            None => {}
        }

        let valid_line = std::str::from_utf8(line).ok();
        if valid_line.is_none() {
            self.valid_utf8 = false;
        }
        if self.total_lines >= self.start_line && self.kept < self.take {
            if self.kept > 0 {
                self.content.push('\n');
            }
            let line_number = self.total_lines;
            if let Some(line) = valid_line {
                self.content.push_str(&format!("{line_number} {line}"));
            } else {
                self.content.push_str(&format!("{line_number}(non-utf-8)"));
            }
            self.kept += 1;
        }
    }

    fn finish(self) -> ReadSlice {
        ReadSlice {
            content: self.content,
            start_line: self.start_line,
            line_count: self.kept,
            ends_with_newline: self.ends_with_newline,
            line_ending: line_ending_label(self.saw_lf, self.saw_crlf, self.saw_cr).to_owned(),
            valid_utf8: self.valid_utf8,
            total_lines: self.total_lines,
        }
    }
}

fn line_ending_label(saw_lf: bool, saw_crlf: bool, saw_cr: bool) -> &'static str {
    let kinds = usize::from(saw_lf) + usize::from(saw_crlf) + usize::from(saw_cr);
    if kinds == 0 {
        "none"
    } else if kinds != 1 {
        "mixed"
    } else if saw_lf {
        "lf"
    } else if saw_crlf {
        "crlf"
    } else {
        "cr"
    }
}

fn parse_read_start_line(value: Option<i64>) -> Result<usize, ToolFailure> {
    match value {
        None => Ok(1),
        Some(value) if value < 1 => Err(ToolFailure::new("start_line must be >= 1")),
        Some(value) => Ok(value as usize),
    }
}

fn parse_read_line_count(value: Option<i64>) -> Result<Option<usize>, ToolFailure> {
    match value {
        None => Ok(None),
        Some(value) if value < 1 => Err(ToolFailure::new("line_count must be >= 1")),
        Some(value) => Ok(Some(value as usize)),
    }
}

pub(crate) fn format_read_range(start_line: Option<usize>, line_count: Option<usize>) -> String {
    match (start_line, line_count) {
        (None, None) => "all lines".to_owned(),
        (Some(start), None) => format!("from line {start}"),
        (None, Some(count)) => format!("first {count} lines"),
        (Some(start), Some(1)) => format!("line {start}"),
        (Some(start), Some(count)) => {
            let end = start.saturating_add(count).saturating_sub(1);
            format!("lines {start}-{end}")
        }
    }
}

/// In-memory equivalent of [`stream_slice_lines`], retained for tests
/// that exercise the slicing logic on a string rather than a file.
#[cfg(test)]
pub(crate) fn slice_lines(input: &str, start_line: usize, line_count: Option<usize>) -> ReadSlice {
    let all_lines: Vec<&str> = input.lines().collect();
    let total_lines = all_lines.len();
    let start_idx = start_line.saturating_sub(1).min(total_lines);
    let end_idx = match line_count {
        Some(count) => start_idx.saturating_add(count).min(total_lines),
        None => total_lines,
    };
    ReadSlice {
        content: all_lines[start_idx..end_idx]
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{} {line}", start_idx + index + 1))
            .collect::<Vec<_>>()
            .join("\n"),
        start_line,
        line_count: end_idx.saturating_sub(start_idx),
        ends_with_newline: input.ends_with('\n'),
        line_ending: line_ending_label(
            input.contains('\n') && !input.contains("\r\n"),
            input.contains("\r\n"),
            input.contains('\r') && !input.contains("\r\n"),
        )
        .to_owned(),
        valid_utf8: true,
        total_lines,
    }
}
