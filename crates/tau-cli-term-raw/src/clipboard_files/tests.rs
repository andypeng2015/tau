use super::*;

/// Encodes fixture originals without granting any filesystem authority.
fn bundle(entries: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let mut bytes = b"TAUFILES\0".to_vec();
    bytes.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (name, mime, body) in entries {
        bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&(mime.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&(body.len() as u64).to_be_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(mime.as_bytes());
        bytes.extend_from_slice(body);
    }
    bytes
}

/// Arbitrary media and duplicate names preserve exact bytes and original order;
/// copied files are not limited to PNG or a discretionary sixteen-file count.
#[test]
fn files_preserve_arbitrary_bodies_names_and_order_without_type_whitelist() {
    let mut entries = vec![("song.mp3", "audio/mpeg", &b"\0\xffMP3"[..]); 17];
    entries.push(("café.txt", "", &b""[..]));
    let files = ClipboardFiles::parse(bundle(&entries).into()).expect("valid files");
    assert_eq!(files.len(), 18);
    for index in 0..17 {
        let file = files.file(index).expect("original");
        assert_eq!(file.filename, Some("song.mp3"));
        assert_eq!(file.mime_type, Some("audio/mpeg"));
        assert_eq!(file.bytes, b"\0\xffMP3");
    }
    let empty = files.file(17).expect("empty original");
    assert_eq!(empty.filename, Some("café.txt"));
    assert!(empty.mime_type.is_none());
    assert!(empty.bytes.is_empty());
    assert!(files.file(18).is_none());
}

/// Every truncation, extra suffix, unknown version, impossible count and
/// overflowing body size fails before exposing even a valid preceding file.
#[test]
fn malformed_framing_never_admits_partial_files() {
    let valid = bundle(&[("a", "", b"x"), ("b", "", b"y")]);
    for end in 0..valid.len() {
        assert!(ClipboardFiles::parse(valid[..end].into()).is_err());
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(ClipboardFiles::parse(trailing.into()).is_err());
    let mut unknown = valid.clone();
    unknown[0] = b'X';
    assert!(ClipboardFiles::parse(unknown.into()).is_err());
    let mut count = valid.clone();
    count[9..13].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(ClipboardFiles::parse(count.into()).is_err());
    let mut body = valid;
    body[17..25].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(ClipboardFiles::parse(body.into()).is_err());
    assert!(ClipboardFiles::parse(bundle(&[]).into()).is_err());
}

/// Names stay basenames and media hints cannot inject prompt text; neither
/// metadata field can become a path, control sequence or content authority.
#[test]
fn metadata_rejects_paths_controls_and_invalid_media() {
    for name in ["", ".", "..", "/tmp/a", "a/b", "a\\b", "a\0", "a\n"] {
        assert!(ClipboardFiles::parse(bundle(&[(name, "", b"")]).into()).is_err());
    }
    for mime in ["audio", "/mpeg", "audio/", "a/b/c", "a/b\nx", "a/<tag>"] {
        assert!(ClipboardFiles::parse(bundle(&[("a", mime, b"")]).into()).is_err());
    }
    let mut utf8 = bundle(&[("a", "", b"")]);
    utf8[25] = 0xff;
    assert!(ClipboardFiles::parse(utf8.into()).is_err());
}

/// The existing aggregate cap includes framing; even an otherwise-valid
/// original cannot make a clipboard bundle exceed sixteen MiB.
#[test]
fn aggregate_cap_includes_framing() {
    let bytes = bundle(&[("a", "", &vec![0; MAX_BYTES])]);
    assert!(ClipboardFiles::parse(bytes.into()).is_err());
    let exact = bundle(&[("a", "", &vec![0; MAX_BYTES - 26])]);
    assert_eq!(exact.len(), MAX_BYTES);
    assert!(ClipboardFiles::parse(exact.into()).is_ok());
}
