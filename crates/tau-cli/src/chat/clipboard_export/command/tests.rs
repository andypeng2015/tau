use super::*;

/// File intent must be explicit, preserve quoted Unicode names, and never
/// widen raw MIME writes or accept path authority.
#[test]
fn explicit_file_grammar_and_safe_names() {
    let key = format!("blake3:{}", "a".repeat(64));
    let default = Command::parse(&format!("{key} --file")).expect("default file command");
    assert_eq!(
        default.file_name.expect("file intent").as_str(),
        format!("artifact-{}.bin", "a".repeat(64))
    );
    for name in [
        "\"two words.pdf\"",
        "'résumé final.pdf'",
        "\"it's fine.txt\"",
    ] {
        let parsed = Command::parse(&format!("<tau-artifact:{key}> --file --name {name}"))
            .expect("quoted name");
        assert_eq!(
            parsed.file_name.expect("file intent").as_str(),
            &name[1..name.len() - 1]
        );
    }
    for args in [
        "--name a",
        "--file --mime text/plain",
        "--mime text/plain --file",
        "--file --name",
        "--file --name ''",
        "--file --name .",
        "--file --name ..",
        "--file --name /tmp/a",
        "--file --name a\\b",
        "--file --name \"a\nb\"",
        "--file --name a b",
        "--file --name 'unfinished",
        "--file --name 'a' trailing",
        "--file --name \"a\" \"b\"",
        "--file --file",
    ] {
        assert!(Command::parse(&format!("{key} {args}")).is_err(), "{args}");
    }
    for name in ["x".repeat(255), "é".repeat(127)] {
        assert!(Command::parse(&format!("{key} --file --name {name}")).is_ok());
    }
    for name in ["x".repeat(256), "é".repeat(128)] {
        assert!(Command::parse(&format!("{key} --file --name {name}")).is_err());
    }
    assert!(
        Command::parse(&key)
            .expect("raw default")
            .file_name
            .is_none()
    );
    let raw = Command::parse(&format!("{key} --mime application/vnd.tau.file-export-v1"))
        .expect("raw MIME override");
    assert!(raw.file_name.is_none());
    assert_eq!(
        raw.mime.as_deref(),
        Some("application/vnd.tau.file-export-v1")
    );
}
