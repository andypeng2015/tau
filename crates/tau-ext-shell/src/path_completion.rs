//! Bounded enumeration in the extension host's filesystem namespace.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tau_shell_interaction::{COMPLETION_LIMIT, CompletionKind, PathCandidate, TOKEN_LIMIT};

/// Enumerate candidates beneath a captured workdir without invoking a shell.
/// Errors and elapsed deadlines return partial or empty results.
pub(crate) fn complete(
    cwd: &Path,
    token: &str,
    matching: CompletionKind,
    deadline: Instant,
) -> Vec<PathCandidate> {
    if token.len() > TOKEN_LIMIT || Instant::now() >= deadline {
        return Vec::new();
    }
    let mut candidates = match matching {
        CompletionKind::Path => prefix_candidates(cwd, token, deadline),
        CompletionKind::PathFuzzy => fuzzy_candidates(cwd, token, deadline),
    };
    candidates.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.value.cmp(&b.value)));
    candidates.dedup_by(|a, b| a.value == b.value);
    candidates.truncate(COMPLETION_LIMIT);
    candidates
}

fn expanded(cwd: &Path, token: &str) -> Option<PathBuf> {
    if token == "~" {
        dirs::home_dir()
    } else if let Some(rest) = token.strip_prefix("~/") {
        dirs::home_dir().map(|home| home.join(rest))
    } else {
        Some(cwd.join(token))
    }
}

fn prefix_candidates(cwd: &Path, token: &str, deadline: Instant) -> Vec<PathCandidate> {
    let (parent, prefix) = token
        .rsplit_once('/')
        .map_or(("", token), |(parent, prefix)| {
            (&token[..parent.len() + 1], prefix)
        });
    let (parent, prefix) = if token == "~" {
        ("~/", "")
    } else {
        (parent, prefix)
    };
    let Some(directory) = expanded(cwd, parent) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.take(4096) {
        if result.len() >= COMPLETION_LIMIT || Instant::now() >= deadline {
            break;
        }
        let Ok(entry) = entry else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
            continue;
        }
        let directory = entry.path().is_dir();
        let value = format!("{parent}{name}{}", if directory { "/" } else { "" });
        if value.len() <= TOKEN_LIMIT {
            result.push(PathCandidate { value, directory });
        }
    }
    result
}

fn fuzzy_candidates(cwd: &Path, token: &str, deadline: Instant) -> Vec<PathCandidate> {
    let (root, prefix, needle) = if let Some(needle) = token.strip_prefix("~/") {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        (home, "~/", needle)
    } else if let Some(needle) = token.strip_prefix('/') {
        (PathBuf::from("/"), "/", needle)
    } else {
        (
            cwd.to_owned(),
            if token.starts_with("./") { "./" } else { "" },
            token.strip_prefix("./").unwrap_or(token),
        )
    };
    let mut result = Vec::new();
    let walker = ignore::WalkBuilder::new(&root)
        .max_depth(Some(12))
        .follow_links(false)
        .build();
    for entry in walker.take(8192) {
        if result.len() >= COMPLETION_LIMIT || Instant::now() >= deadline {
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue;
        }
        let Ok(path) = entry.path().strip_prefix(&root) else {
            continue;
        };
        let Some(path) = path.to_str() else { continue };
        if !subsequence(needle, path) {
            continue;
        }
        let directory = entry.file_type().is_some_and(|kind| kind.is_dir());
        let value = format!("{prefix}{path}{}", if directory { "/" } else { "" });
        if value.len() <= TOKEN_LIMIT {
            result.push(PathCandidate { value, directory });
        }
    }
    result
}

fn subsequence(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack.chars();
    needle.chars().all(|needle| {
        haystack
            .by_ref()
            .any(|candidate| candidate.eq_ignore_ascii_case(&needle))
    })
}

#[cfg(test)]
mod tests;
