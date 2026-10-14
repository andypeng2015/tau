//! Consumer-local media inference; never canonical artifact metadata.

use tau_proto::ArtifactSignature;

#[cfg(test)]
mod tests;

/// Byte-preserving media declaration for one clipboard export.
pub struct ArtifactMedia {
    /// Selected inferred or explicitly declared media type.
    pub mime: String,
    /// Plain alias is allowed only for default complete-buffer UTF-8 inference.
    pub plain_alias: bool,
}

impl ArtifactMedia {
    /// Selects a media declaration after the caller verifies the complete
    /// original.
    ///
    /// Overrides support ASCII type/subtype and one optional unquoted charset
    /// parameter. They declare intent, not content validity.
    pub fn select(bytes: &[u8], declared: Option<&str>) -> Result<Self, &'static str> {
        if let Some(mime) = declared {
            validate_mime(mime)?;
            if mime.split_once(';').is_some_and(|(kind, charset)| {
                kind.to_ascii_lowercase().starts_with("text/")
                    && charset.eq_ignore_ascii_case("charset=utf-8")
            }) && std::str::from_utf8(bytes).is_err()
            {
                return Err("declared UTF-8 text is not valid UTF-8");
            }
            return Ok(Self {
                mime: mime.to_owned(),
                plain_alias: false,
            });
        }
        if let Some(signature) = artifact_signature(bytes) {
            return Ok(Self {
                mime: signature.mime().to_owned(),
                plain_alias: false,
            });
        }
        let text = std::str::from_utf8(bytes).is_ok_and(|text| {
            text.chars()
                .all(|ch| !ch.is_control() || matches!(ch, '\t' | '\r' | '\n'))
        });
        Ok(Self {
            mime: if text {
                "text/plain;charset=utf-8"
            } else {
                "application/octet-stream"
            }
            .to_owned(),
            plain_alias: text,
        })
    }
}

/// Infers only fixed signatures from at most the first sixteen bytes.
///
/// This does not validate the body or classify UTF-8 from a prefix.
#[must_use]
pub fn artifact_signature(bytes: &[u8]) -> Option<ArtifactSignature> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ArtifactSignature::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ArtifactSignature::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ArtifactSignature::Gif)
    } else if bytes.starts_with(b"RIFF")
        && bytes.get(8..12) == Some(b"WEBP")
        && matches!(bytes.get(12..16), Some(b"VP8 " | b"VP8L" | b"VP8X"))
    {
        Some(ArtifactSignature::Webp)
    } else if bytes.starts_with(b"%PDF-") {
        Some(ArtifactSignature::Pdf)
    } else {
        None
    }
}

fn validate_mime(mime: &str) -> Result<(), &'static str> {
    let error = "MIME must be ASCII type/subtype, optionally ;charset=TOKEN (maximum 256 bytes)";
    if mime.len() > 256 || !mime.is_ascii() {
        return Err(error);
    }
    let mut pieces = mime.split(';');
    let kind = pieces.next().ok_or(error)?;
    let (major, minor) = kind.split_once('/').ok_or(error)?;
    if !token(major) || !token(minor) || major.contains('*') || minor.contains('*') {
        return Err(error);
    }
    if let Some(parameter) = pieces.next() {
        let (name, value) = parameter.split_once('=').ok_or(error)?;
        if !name.eq_ignore_ascii_case("charset") || !token(value) {
            return Err(error);
        }
    }
    if pieces.next().is_some() {
        return Err(error);
    }
    Ok(())
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
