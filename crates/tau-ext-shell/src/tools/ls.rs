//! `ls` tool: directory listing with truncation.

use std::fs;
use std::path::PathBuf;

use tau_proto::{CborValue, ToolUseStats};

use crate::argument::{optional_argument_int, optional_argument_text};
use crate::display::{ToolFailure, ToolOutput, ok_display, text_stats};
use crate::truncate::truncate_line_oriented;

pub(crate) const DEFAULT_LS_LIMIT: usize = 500;

pub(crate) fn run_ls(arguments: &CborValue) -> Result<ToolOutput, ToolFailure> {
    let path = optional_argument_text(arguments, "path").unwrap_or_else(|| ".".to_owned());
    let limit = optional_argument_int(arguments, "limit")
        .map(|v| v.max(1) as usize)
        .unwrap_or(DEFAULT_LS_LIMIT);
    let dir_path = PathBuf::from(&path);
    let display_args = dir_path.display().to_string();
    let with_args = |f: ToolFailure| f.with_args(display_args.clone());

    let metadata = fs::metadata(&dir_path).map_err(|e| {
        with_args(ToolFailure::from(format!(
            "failed to access {}: {e}",
            dir_path.display()
        )))
    })?;
    if !metadata.is_dir() {
        return Err(with_args(ToolFailure::from(format!(
            "not a directory: {}",
            dir_path.display()
        ))));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir_path).map_err(|e| {
        with_args(ToolFailure::from(format!(
            "failed to read {}: {e}",
            dir_path.display()
        )))
    })? {
        let entry = entry.map_err(|e| {
            with_args(ToolFailure::from(format!(
                "failed to read {}: {e}",
                dir_path.display()
            )))
        })?;
        let name = entry.file_name();
        let mut display = name.to_string_lossy().into_owned();
        if entry
            .file_type()
            .map_err(|e| {
                with_args(ToolFailure::from(format!(
                    "failed to read {}: {e}",
                    dir_path.display()
                )))
            })?
            .is_dir()
        {
            display.push('/');
        }
        entries.push(display);
    }
    entries.sort_by_key(|entry| entry.to_lowercase());

    if entries.is_empty() {
        let mut display = ok_display(display_args.clone());
        display.stats = ToolUseStats {
            matches: None,
            lines: Some(0),
            bytes: Some(0),
        };
        return Ok(ToolOutput {
            result: CborValue::Map(vec![
                (
                    CborValue::Text("entries".to_owned()),
                    CborValue::Integer(0.into()),
                ),
                (
                    CborValue::Text("output".to_owned()),
                    CborValue::Text("(empty directory)".to_owned()),
                ),
            ]),
            display,
        });
    }
    let total_entries = entries.len();
    let displayed: Vec<String> = entries.into_iter().take(limit).collect();
    let limit_reached = total_entries > displayed.len();
    let mut output_text = displayed.join("\n");
    let truncated = truncate_line_oriented(&output_text);
    if truncated.was_truncated {
        output_text = truncated.content;
    }

    let mut notices = Vec::new();
    if limit_reached {
        notices.push(format!(
            "{limit} entries limit reached. Use limit={} for more.",
            limit * 2
        ));
    }
    if truncated.was_truncated {
        notices.push("50KB/2000 line output limit reached.".to_owned());
    }
    if !notices.is_empty() {
        output_text.push_str("\n\n[");
        output_text.push_str(&notices.join(" "));
        output_text.push(']');
    }

    let mut display = ok_display(display_args.clone());
    display.stats = text_stats(&output_text);
    Ok(ToolOutput {
        result: CborValue::Map(vec![
            (
                CborValue::Text("entries".to_owned()),
                CborValue::Integer((total_entries as i64).into()),
            ),
            (
                CborValue::Text("output".to_owned()),
                CborValue::Text(output_text),
            ),
        ]),
        display,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ls_args(path: &std::path::Path) -> CborValue {
        CborValue::Map(vec![(
            CborValue::Text("path".to_owned()),
            CborValue::Text(path.display().to_string()),
        )])
    }

    #[test]
    fn empty_ls_display_uses_zero_line_and_byte_stats() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");

        let output = run_ls(&ls_args(tempdir.path())).expect("ls output");

        assert!(output.display.info_chips.is_empty());
        assert_eq!(output.display.stats.lines, Some(0));
        assert_eq!(output.display.stats.bytes, Some(0));
        assert!(matches!(
            &output.result,
            CborValue::Map(entries)
                if entries.iter().any(|(key, value)| matches!(
                    (key, value),
                    (CborValue::Text(key), CborValue::Text(value))
                        if key == "output" && value == "(empty directory)"
                ))
        ));
    }

    #[test]
    fn ls_display_uses_line_and_byte_stats_instead_of_entry_chip() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(tempdir.path().join("alpha"), "a").expect("write alpha");
        std::fs::write(tempdir.path().join("beta"), "b").expect("write beta");

        let output = run_ls(&ls_args(tempdir.path())).expect("ls output");

        assert!(output.display.info_chips.is_empty());
        assert_eq!(output.display.stats.lines, Some(2));
        assert_eq!(output.display.stats.bytes, Some("alpha\nbeta".len() as u64));
    }
}
