//! Credentials on disk: created, read, written, per environment.

use grenat_config::credentials::Location;
use serde_json::json;

fn app(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("grenat-config-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn created_with_a_private_key_then_edited() {
    let root = app("create");
    let location = Location::of(&root, None);
    assert!(!location.exists());
    location.create().unwrap();
    assert!(location.create().unwrap_err().contains("already exists"));
    assert!(location.read().unwrap().contains("Credentials.fetch"));
    assert_eq!(location.load().unwrap(), json!({}), "the template is only comments");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&location.key).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let encrypted = std::fs::read_to_string(&location.file).unwrap();
    assert!(!encrypted.contains("Credentials"), "encrypted on disk");
    location.write("# kept\ngithub:\n  token: ghp_123\n").unwrap();
    assert_eq!(location.load().unwrap(), json!({"github": {"token": "ghp_123"}}));
    assert!(location.read().unwrap().starts_with("# kept"), "comments are kept");
    assert!(location.write("github: [").unwrap_err().contains("invalid YAML"));
    assert_eq!(location.load().unwrap(), json!({"github": {"token": "ghp_123"}}), "unchanged");
    std::fs::write(&location.key, "0".repeat(64)).unwrap();
    assert!(location.read().unwrap_err().contains("wrong key"));
    std::fs::remove_file(&location.key).unwrap();
    assert!(location.read().unwrap_err().contains("GRENAT_MASTER_KEY"));
}

#[test]
fn an_environment_uses_its_own_credentials_when_it_has_them() {
    let root = app("envs");
    let shared = Location::of(&root, None);
    shared.create().unwrap();
    assert_eq!(Location::for_env(&root, "production"), shared);
    let production = Location::of(&root, Some("production"));
    assert_eq!(production.file, root.join("config/credentials/production.yml.enc"));
    assert_eq!(production.key, root.join("config/credentials/production.key"));
    production.create().unwrap();
    production.write("db:\n  password: prod\n").unwrap();
    assert_eq!(Location::for_env(&root, "production"), production);
    assert_eq!(Location::for_env(&root, "production").load().unwrap(), json!({"db": {"password": "prod"}}));
    assert_eq!(Location::for_env(&root, "staging"), shared);
    // each has its own key
    assert_ne!(std::fs::read_to_string(&shared.key).unwrap(), std::fs::read_to_string(&production.key).unwrap());
}

#[test]
fn keys_are_kept_out_of_git() {
    let root = app("gitignore");
    assert!(grenat_config::credentials::ignore_keys(&root).unwrap());
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        "config/master.key\nconfig/credentials/*.key\n"
    );
    assert!(!grenat_config::credentials::ignore_keys(&root).unwrap(), "once");
    std::fs::write(root.join(".gitignore"), ".grenat/\nconfig/master.key").unwrap();
    assert!(grenat_config::credentials::ignore_keys(&root).unwrap());
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        ".grenat/\nconfig/master.key\nconfig/credentials/*.key\n"
    );
}
