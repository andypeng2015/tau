//! Git-backed file enumeration and fuzzy ranking for prompt completion.
//!
//! `./...` completion uses these helpers to search all tracked and unignored
//! files in the current repository without teaching the terminal input layer
//! about git or fuzzy-matching details.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_CANDIDATES: usize = 100;

#[derive(Clone)]
struct GitFileCache {
    cwd: PathBuf,
    repo_root: PathBuf,
    files: Vec<String>,
}

static CACHE: Mutex<Option<GitFileCache>> = Mutex::new(None);

/// Returns the git repository root and tracked/unignored files for `cwd`.
///
/// The file list is cached per current working directory so each keystroke in a
/// completion session can re-rank the same repository snapshot cheaply.
/// Starting completion from another directory refreshes the cache.
pub(crate) fn git_repo_files(cwd: &Path) -> Option<(PathBuf, Vec<String>)> {
    if let Ok(cache) = CACHE.lock()
        && let Some(cached) = cache.as_ref()
        && cached.cwd == cwd
    {
        return Some((cached.repo_root.clone(), cached.files.clone()));
    }

    let result = enumerate_git_files(cwd);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = result.as_ref().map(|(repo_root, files)| GitFileCache {
            cwd: cwd.to_path_buf(),
            repo_root: repo_root.clone(),
            files: files.clone(),
        });
    }
    result
}

fn enumerate_git_files(cwd: &Path) -> Option<(PathBuf, Vec<String>)> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let repo_root = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let files = stdout.lines().map(str::to_owned).collect();
    Some((repo_root, files))
}

/// Fuzzy-matches `query` against repo-relative git file paths.
pub(crate) fn fuzzy_match_git_files<'a>(query: &str, files: &'a [String]) -> Vec<&'a str> {
    if query.is_empty() {
        return files
            .iter()
            .take(MAX_CANDIDATES)
            .map(String::as_str)
            .collect();
    }

    use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
    use nucleo_matcher::{Config, Matcher};

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let atom = Atom::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );

    atom.match_list(files.iter().map(String::as_str), &mut matcher)
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(path, _score)| path)
        .collect()
}

/// Converts a repo-relative path into the prompt replacement path for `./`.
///
/// Files below `cwd` keep an explicit `./` prefix. Files elsewhere in the repo
/// are still offered, but use a normal relative path such as `../Cargo.toml`.
pub(crate) fn dotslash_display_path(repo_relative: &str, repo_root: &Path, cwd: &Path) -> String {
    let abs = repo_root.join(repo_relative);
    let cwd_relative = relative_path(cwd, &abs)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo_relative.to_owned());
    if cwd_relative.starts_with("../") || cwd_relative == ".." {
        cwd_relative
    } else {
        format!("./{cwd_relative}")
    }
}

fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common_len = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(left, right)| left == right)
        .count();

    let mut result = PathBuf::new();
    for _ in common_len..base_components.len() {
        result.push("..");
    }
    for component in &target_components[common_len..] {
        result.push(component);
    }
    (!result.as_os_str().is_empty()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_git_files_ranks_path_matches() {
        let files = vec![
            "crates/tau-cli-term/src/completion.rs".to_owned(),
            "README.md".to_owned(),
            "crates/tau/src/main.rs".to_owned(),
        ];

        let matches = fuzzy_match_git_files("completion", &files);

        assert_eq!(
            matches.first(),
            Some(&"crates/tau-cli-term/src/completion.rs")
        );
    }

    #[test]
    fn dotslash_display_path_keeps_local_prefix_and_allows_parent_paths() {
        let root = PathBuf::from("/repo");
        let cwd = root.join("crates/tau-cli-term");

        assert_eq!(
            dotslash_display_path("crates/tau-cli-term/src/lib.rs", &root, &cwd),
            "./src/lib.rs"
        );
        assert_eq!(
            dotslash_display_path("Cargo.toml", &root, &cwd),
            "../../Cargo.toml"
        );
    }
}
