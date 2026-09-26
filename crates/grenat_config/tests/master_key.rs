//! `GRENAT_MASTER_KEY` gives the key without a file (alone in this binary:
//! the environment is shared by the tests of a process).

use grenat_config::credentials::{KEY_VARIABLE, Location};

#[test]
fn the_key_can_come_from_the_environment() {
    let root = std::env::temp_dir().join(format!("grenat-config-{}-master-key", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let location = Location::of(&root, None);
    location.create().unwrap();
    location.write("a: 1\n").unwrap();
    let key = std::fs::read_to_string(&location.key).unwrap();
    std::fs::remove_file(&location.key).unwrap();
    // SAFETY: the only test of this binary
    unsafe { std::env::set_var(KEY_VARIABLE, key.trim()) };
    assert_eq!(location.read().unwrap(), "a: 1\n");
    unsafe { std::env::set_var(KEY_VARIABLE, "f".repeat(64)) };
    assert!(location.read().is_err());
    unsafe { std::env::remove_var(KEY_VARIABLE) };
}
