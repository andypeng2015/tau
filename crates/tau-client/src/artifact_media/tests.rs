use super::*;

/// Default classification must inspect the complete original and never rewrite
/// bytes.
#[test]
fn whole_buffer_text_and_binary_defaults_preserve_intent() {
    for bytes in [b"".as_slice(), b"\xef\xbb\xbftext\r\n\t", "λ".as_bytes()] {
        let media = ArtifactMedia::select(bytes, None).expect("default text inference");
        assert_eq!(media.mime, "text/plain;charset=utf-8");
        assert!(media.plain_alias);
    }
    for bytes in [b"text\0".as_slice(), b"text\xff", b"text\x1b", b"\xc2\x85"] {
        let media = ArtifactMedia::select(bytes, None).expect("default binary inference");
        assert_eq!(media.mime, "application/octet-stream");
        assert!(!media.plain_alias);
    }
    for bytes in [b"<html>active</html>".as_slice(), b"<svg/>"] {
        assert_eq!(
            ArtifactMedia::select(bytes, None)
                .expect("default safe inference")
                .mime,
            "text/plain;charset=utf-8"
        );
    }
}

/// Signature hints remain narrow and explicit declarations win without binary
/// aliases.
#[test]
fn signatures_and_overrides_do_not_claim_media_validity() {
    for (bytes, expected) in [
        (b"\x89PNG\r\n\x1a\n".as_slice(), "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF89a", "image/gif"),
        (b"RIFF0000WEBPVP8X", "image/webp"),
        (b"%PDF-", "application/pdf"),
    ] {
        assert_eq!(
            ArtifactMedia::select(bytes, None)
                .expect("fixed signature")
                .mime,
            expected
        );
        let media = ArtifactMedia::select(bytes, Some("application/x-custom+binary"))
            .expect("valid override");
        assert_eq!(media.mime, "application/x-custom+binary");
        assert!(!media.plain_alias);
    }
    assert!(artifact_signature(b"RIFF0000WAVE").is_none());
    assert!(ArtifactMedia::select(b"\xff", Some("text/plain;charset=utf-8")).is_err());
    assert!(ArtifactMedia::select(b"\xff", Some("text/plain;charset=iso-8859-1")).is_ok());
    for invalid in [
        "text/*",
        "text/plain;foo=bar",
        "text/plain;charset=\"utf-8\"",
        "text/plain\n",
        "text/plain;charset=x;charset=y",
        "text/plain; charset=utf-8",
    ] {
        assert!(
            ArtifactMedia::select(b"", Some(invalid)).is_err(),
            "{invalid}"
        );
    }
}
