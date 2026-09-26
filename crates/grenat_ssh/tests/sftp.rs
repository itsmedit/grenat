//! SFTP against the in-process server, whose files live in a scratch
//! directory the test also sees.

use std::time::Duration;

use grenat_ssh::fake::{TempDir, TestServer, USER, key_text, new_key};
use grenat_ssh::{Auth, ErrorKind, KnownHosts, Options, Session, Sftp};

/// A server, and an SFTP session on it.
fn sftp() -> (TestServer, Session, Sftp) {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let session = Session::connect(Options {
        port: server.port,
        known_hosts: KnownHosts::Fingerprint(server.fingerprint()),
        timeout: Some(Duration::from_secs(10)),
        ..Options::new(USER, "127.0.0.1", Auth::Key { text: key_text(&key), passphrase: None })
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let sftp = session.sftp().unwrap_or_else(|e| panic!("{e}"));
    (server, session, sftp)
}

#[test]
fn write_read_exists() {
    let (server, _session, sftp) = sftp();
    assert!(!sftp.exists("notes.txt").unwrap());
    sftp.write("notes.txt", b"first").unwrap();
    assert!(sftp.exists("notes.txt").unwrap());
    assert!(sftp.exists("/notes.txt").unwrap());
    assert_eq!(sftp.read("notes.txt").unwrap(), b"first");
    assert_eq!(std::fs::read(server.home.path().join("notes.txt")).unwrap(), b"first");
    // Writing replaces the whole content.
    sftp.write("notes.txt", b"2nd").unwrap();
    assert_eq!(sftp.read("notes.txt").unwrap(), b"2nd");
    sftp.write("empty", b"").unwrap();
    assert_eq!(sftp.read("empty").unwrap(), b"");
    // Larger than one SFTP packet.
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    sftp.write("big.bin", &big).unwrap();
    assert_eq!(sftp.read("big.bin").unwrap(), big);

    let e = sftp.read("missing.txt").unwrap_err();
    assert_eq!((e.kind(), e.message()), (ErrorKind::Sftp, "cannot read `missing.txt`: no such file or directory"));
}

#[test]
fn directories() {
    let (server, _session, sftp) = sftp();
    sftp.mkdir("docs").unwrap();
    sftp.mkdir("docs/sub").unwrap();
    sftp.write("docs/b.txt", b"bbb").unwrap();
    sftp.write("docs/a.txt", b"a").unwrap();
    let entries = sftp.list("docs").unwrap();
    let names: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
    assert_eq!(names, [("a.txt", false), ("b.txt", false), ("sub", true)]);
    assert_eq!(entries[1].size, 3);
    assert!(entries[1].modified.is_some());
    assert!(server.home.path().join("docs/sub").is_dir());

    let e = sftp.mkdir("docs").unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Sftp);
    assert!(e.message().starts_with("cannot create the directory `docs`"), "{e}");
    let e = sftp.list("nowhere").unwrap_err();
    assert_eq!(e.message(), "cannot list `nowhere`: no such file or directory");
}

#[test]
fn rename_and_remove() {
    let (server, _session, sftp) = sftp();
    sftp.write("old.txt", b"x").unwrap();
    sftp.rename("old.txt", "new.txt").unwrap();
    assert!(!sftp.exists("old.txt").unwrap());
    assert_eq!(sftp.read("new.txt").unwrap(), b"x");
    let e = sftp.rename("old.txt", "other.txt").unwrap_err();
    assert!(e.message().starts_with("cannot rename `old.txt` to `other.txt`"), "{e}");

    sftp.remove("new.txt").unwrap();
    assert!(!sftp.exists("new.txt").unwrap());
    sftp.mkdir("dir").unwrap();
    sftp.remove("dir").unwrap();
    assert!(!server.home.path().join("dir").exists());
    let e = sftp.remove("new.txt").unwrap_err();
    assert_eq!(e.message(), "cannot remove `new.txt`: no such file or directory");
}

#[test]
fn upload_and_download() {
    let (server, _session, sftp) = sftp();
    let local = TempDir::new("local");
    let content: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let source = local.path().join("source.bin");
    std::fs::write(&source, &content).unwrap();

    sftp.upload(&source, "uploaded.bin").unwrap();
    assert_eq!(std::fs::read(server.home.path().join("uploaded.bin")).unwrap(), content);

    let back = local.path().join("back.bin");
    sftp.download("uploaded.bin", &back).unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), content);

    let e = sftp.upload(&local.path().join("absent"), "x").unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Sftp);
    assert!(e.message().starts_with("cannot read the local file"), "{e}");
    let e = sftp.download("absent", &local.path().join("x")).unwrap_err();
    assert_eq!(e.message(), "cannot read `absent`: no such file or directory");
    assert!(!local.path().join("x").exists(), "nothing created locally when the remote file is missing");
}

#[test]
fn sftp_and_commands_share_the_connection() {
    let (_server, session, sftp) = sftp();
    sftp.write("made-by-sftp", b"hi").unwrap();
    assert_eq!(session.run(&["cat", "made-by-sftp"]).unwrap().stdout, b"hi");
    let second = session.sftp().unwrap();
    assert_eq!(second.read("made-by-sftp").unwrap(), b"hi");
}
