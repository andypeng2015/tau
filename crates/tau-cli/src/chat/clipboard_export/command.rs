//! Explicit clipboard intent, validated before reserving terminal authority.

use tau_cli_term::ClipboardFileName;
use tau_proto::ArtifactKey;

const USAGE: &str = "usage: :export-to-clipboard KEY [--mime TYPE/SUBTYPE[;charset=TOKEN] | --file [--name BASENAME]]";

/// One validated invocation; raw MIME and file intent are mutually exclusive.
pub(super) struct Command {
    /// Full original-byte content address.
    pub(super) key: ArtifactKey,
    /// Existing raw-write override.
    pub(super) mime: Option<String>,
    /// Validated local basename selects the dedicated copied-file protocol.
    pub(super) file_name: Option<ClipboardFileName>,
}

impl Command {
    /// Parses the fixed grammar, including a single- or double-quoted basename.
    pub(super) fn parse(args: &str) -> Result<Self, &'static str> {
        let (key, rest) = word(args);
        let key = tau_proto::parse_artifact_reference(key)
            .map_err(|_| "Expected a full artifact key or canonical artifact reference")?;
        let (option, rest) = word(rest);
        let mut command = Self {
            key,
            mime: None,
            file_name: None,
        };
        match option {
            "" => {}
            "--mime" => {
                let (mime, tail) = word(rest);
                if mime.is_empty() || !tail.is_empty() {
                    return Err(USAGE);
                }
                tau_client::ArtifactMedia::select(&[], Some(mime))?;
                command.mime = Some(mime.to_owned());
            }
            "--file" => {
                let (option, rest) = word(rest);
                let name = match option {
                    "" => format!(
                        "artifact-{}.bin",
                        command
                            .key
                            .as_str()
                            .strip_prefix("blake3:")
                            .expect("validated BLAKE3 key")
                    ),
                    "--name" => basename(rest)?.to_owned(),
                    _ => return Err(USAGE),
                };
                command.file_name = Some(ClipboardFileName::new(name)?);
            }
            _ => return Err(USAGE),
        }
        Ok(command)
    }
}

/// Splits one unquoted syntax token; only the final basename supports quotes.
fn word(input: &str) -> (&str, &str) {
    let input = input.trim();
    input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(head, tail)| (head, tail.trim()))
}

/// Quotes group the entire basename literally; there is no shell expansion or
/// escape syntax, and trailing tokens must not become part of an argument.
fn basename(input: &str) -> Result<&str, &'static str> {
    let input = input.trim();
    if let Some(quote @ ('\'' | '"')) = input.chars().next() {
        let name = input
            .strip_prefix(quote)
            .and_then(|s| s.strip_suffix(quote))
            .ok_or(USAGE)?;
        if name.contains(quote) {
            return Err(USAGE);
        }
        Ok(name)
    } else if input.is_empty() || input.chars().any(char::is_whitespace) {
        Err(USAGE)
    } else {
        Ok(input)
    }
}

#[cfg(test)]
mod tests;
