//! `read` tool: read a file (optionally a line slice).

use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use tau_proto::CborValue;

use crate::argument::{argument_text, optional_argument_int};
use crate::display::{ToolFailure, ToolOutput, ok_display, text_stats};
use crate::truncate::truncate_head_plain;

pub(crate) fn read_file(arguments: &CborValue) -> Result<ToolOutput, ToolFailure> {
    let path = argument_text(arguments, "path").map_err(ToolFailure::from)?;
    let start_line_arg = optional_argument_int(arguments, "start_line");
    let line_count_arg = optional_argument_int(arguments, "line_count");
    let start_line = parse_read_start_line(start_line_arg)?;
    let line_count = parse_read_line_count(line_count_arg)?;
    let path_buf = PathBuf::from(&path);
    let range = format_read_range(start_line_arg.map(|_| start_line), line_count);
    let display_args = format!("{} {range}", path_buf.display());

    let sliced = stream_slice_lines(&path_buf, start_line, line_count)
        .map_err(|error| ToolFailure::from(error.to_string()).with_args(display_args.clone()))?;
    let total_lines = sliced.total_lines;
    let truncated = truncate_read_content(&sliced.content, start_line, total_lines);
    let returned_line_count = truncated.line_count;
    debug_assert!(returned_line_count <= sliced.line_count);
    let mut entries = vec![
        (
            CborValue::Text("path".to_owned()),
            CborValue::Text(display_args.clone()),
        ),
        (
            CborValue::Text("line-numbered content".to_owned()),
            CborValue::Text(truncated.content.clone()),
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
    ];
    if truncated.was_truncated {
        entries.push((
            CborValue::Text("truncated".to_owned()),
            CborValue::Bool(true),
        ));
        entries.push((
            CborValue::Text("total_bytes".to_owned()),
            CborValue::Integer((truncated.total_bytes as i64).into()),
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
    /// Total lines in the source. For [`stream_slice_lines`] this is
    /// computed by scanning the rest of the file after the slice ends.
    pub(crate) total_lines: usize,
}

struct TruncatedRead {
    content: String,
    was_truncated: bool,
    total_bytes: usize,
    line_count: usize,
}

fn truncate_read_content(content: &str, start_line: usize, total_lines: usize) -> TruncatedRead {
    let mut truncated = truncate_head_plain(content);
    let line_count = truncated.content.lines().count();
    if truncated.was_truncated {
        let end_line = start_line.saturating_add(line_count).saturating_sub(1);
        truncated.content.push_str(&format!(
            "\n\n[Showing lines {start_line}-{end_line} of {total_lines} ({} bytes total). \
             Use start_line and line_count to continue reading.]",
            truncated.total_bytes
        ));
    }

    TruncatedRead {
        content: truncated.content,
        was_truncated: truncated.was_truncated,
        total_bytes: truncated.total_bytes,
        line_count,
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
    use std::io::BufRead as _;

    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut content = String::new();
    let mut kept = 0usize;
    let mut total_lines = 0usize;
    let mut buf = String::new();
    let take = line_count.unwrap_or(usize::MAX);

    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut ends_with_newline = false;

    loop {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        total_lines += 1;
        ends_with_newline = buf.ends_with('\n');
        if buf.ends_with("\r\n") {
            saw_crlf = true;
        } else if buf.ends_with('\n') {
            saw_lf = true;
        }
        // 1-based index of this line is `total_lines`. Inside slice
        // window if it's >= start_line and we haven't kept enough yet.
        if total_lines >= start_line && kept < take {
            // Strip a single trailing newline so the join shape
            // matches `slice_lines` (which used `lines().join("\n")`).
            let trimmed = buf.strip_suffix('\n').unwrap_or(&buf);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
            if kept > 0 {
                content.push('\n');
            }
            content.push_str(&format!("{total_lines} {trimmed}"));
            kept += 1;
        }
    }

    Ok(ReadSlice {
        content,
        start_line,
        line_count: kept,
        ends_with_newline,
        line_ending: line_ending_label(saw_lf, saw_crlf).to_owned(),
        total_lines,
    })
}

fn line_ending_label(saw_lf: bool, saw_crlf: bool) -> &'static str {
    match (saw_lf, saw_crlf) {
        (false, false) => "none",
        (true, false) => "lf",
        (false, true) => "crlf",
        (true, true) => "mixed",
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
        (None, None) => "..".to_owned(),
        (Some(start), None) => format!("{start}.."),
        (None, Some(count)) => format!("..{count}"),
        (Some(start), Some(count)) => {
            let end = start.saturating_add(count).saturating_sub(1);
            format!("{start}..{end}")
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
        line_ending: line_ending_label(input.contains('\n'), input.contains("\r\n")).to_owned(),
        total_lines,
    }
}
