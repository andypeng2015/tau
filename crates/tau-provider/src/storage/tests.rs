use super::*;

#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TestAuth {
    token: String,
}

#[test]
fn auth_file_loads_default_when_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = ProviderStore::open_in(temp.path())
        .auth_file::<TestAuth>("provider-test")
        .expect("auth file");

    assert_eq!(file.load().expect("load"), None);
}

#[test]
fn auth_file_saves_and_deletes_typed_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = ProviderStore::open_in(temp.path())
        .auth_file::<TestAuth>("provider-test")
        .expect("auth file");

    file.save(&TestAuth {
        token: "secret".to_owned(),
    })
    .expect("save");
    assert_eq!(
        file.load().expect("load"),
        Some(TestAuth {
            token: "secret".to_owned()
        })
    );
    assert!(file.delete().expect("delete"));
    assert!(!file.delete().expect("delete missing"));
    assert_eq!(file.load().expect("load missing"), None);
}

#[test]
fn auth_file_rejects_unsafe_names() {
    for name in ["", ".hidden", "-leading", "has/slash", "has space"] {
        assert!(
            ProviderStore::open_in("/tmp")
                .auth_file::<TestAuth>(name)
                .is_err(),
            "expected '{name}' to be rejected"
        );
    }
}
