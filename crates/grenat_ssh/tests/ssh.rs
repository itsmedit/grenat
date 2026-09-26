//! Connections against the in-process server: authentication, host key
//! policies, commands and their quoting, the SOCKS5 proxy, timeouts.

mod server;

use std::time::{Duration, Instant};

use grenat_ssh::{Auth, ErrorKind, KnownHosts, Options, Session};
use russh::keys::ssh_key::{LineEnding, PrivateKey};
use server::socks::Socks;
use server::{PASSWORD, TempDir, TestServer, USER, fingerprint, key_text, new_key};

/// Options for `server` with `key`, trusting the server's fingerprint.
fn options(server: &TestServer, key: &PrivateKey) -> Options {
    Options {
        port: server.port,
        known_hosts: KnownHosts::Fingerprint(server.fingerprint()),
        timeout: Some(Duration::from_secs(10)),
        ..Options::new(USER, "127.0.0.1", Auth::Key { text: key_text(key), passphrase: None })
    }
}

fn connect(options: Options) -> Session {
    Session::connect(options).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn key_authentication_runs_a_command() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let session = connect(options(&server, &key));
    let host_key = session.host_key();
    assert_eq!(
        (host_key.algorithm.as_str(), host_key.fingerprint.clone(), host_key.recorded),
        ("ssh-ed25519", server.fingerprint(), false)
    );
    let output = session.run(&["echo", "hello"]).unwrap();
    assert_eq!(
        (output.stdout.as_slice(), output.stderr.as_slice(), output.status),
        (&b"hello\n"[..], &b""[..], Some(0))
    );
    assert!(output.success());
    // Several commands on one connection.
    assert_eq!(session.run(&["pwd"]).unwrap().stdout, format!("{}\n", server.home.path().display()).into_bytes());
    session.close().unwrap();
}

#[test]
fn a_wrong_key_is_refused() {
    let server = TestServer::start(new_key().public_key());
    let intruder = new_key();
    let e = Session::connect(options(&server, &intruder)).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth, "{e}");
    assert!(e.message().contains(&fingerprint(intruder.public_key())), "{e}");
    assert!(e.message().contains("alice") && e.message().contains("refused"), "{e}");
    let text = key_text(&intruder);
    let secret_line = text.lines().nth(1).unwrap();
    assert!(!e.message().contains(secret_line), "{e}");
    assert!(server.commands().is_empty());
}

#[test]
fn encrypted_keys_need_their_passphrase() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let encrypted = key.encrypt(&mut rand::rng(), "open sesame").unwrap();
    let text = encrypted.to_openssh(LineEnding::LF).unwrap().to_string();
    let with = |passphrase: Option<&str>| Options {
        auth: Auth::Key { text: text.clone(), passphrase: passphrase.map(str::to_string) },
        ..options(&server, &key)
    };
    let session = connect(with(Some("open sesame")));
    assert!(session.run(&["true"]).unwrap().success());

    let missing = Session::connect(with(None)).err().unwrap();
    assert_eq!(missing.kind(), ErrorKind::Auth);
    assert!(missing.message().contains("passphrase is needed"), "{missing}");
    let wrong = Session::connect(with(Some("close sesame"))).err().unwrap();
    assert_eq!(wrong.kind(), ErrorKind::Auth);
    assert!(wrong.message().contains("wrong passphrase") && !wrong.message().contains("close sesame"), "{wrong}");
}

#[test]
fn password_authentication() {
    let server = TestServer::start(new_key().public_key());
    let with = |password: &str| Options { auth: Auth::Password(password.into()), ..options(&server, &new_key()) };
    let session = connect(with(PASSWORD));
    assert_eq!(session.run(&["echo", "in"]).unwrap().stdout, b"in\n");

    let e = Session::connect(with("guess-123")).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth);
    assert!(e.message().contains("refused the password of `alice`"), "{e}");
    assert!(!e.message().contains("guess-123"), "{e}");
}

#[test]
fn an_unknown_host_key_is_refused() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let dir = TempDir::new("unknown");
    let known_hosts = dir.path().join("known_hosts");
    std::fs::write(&known_hosts, "").unwrap();
    let e = Session::connect(Options { known_hosts: KnownHosts::File(known_hosts), ..options(&server, &key) })
        .err()
        .unwrap();
    assert_eq!(e.kind(), ErrorKind::HostKey);
    assert!(e.message().contains("unknown") && e.message().contains(&server.fingerprint()), "{e}");
    assert!(e.message().contains("ssh-ed25519"), "{e}");
    assert!(server.commands().is_empty());
}

#[test]
fn the_expected_fingerprint_only() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    assert!(connect(options(&server, &key)).run(&["true"]).unwrap().success());

    let other = fingerprint(new_key().public_key());
    let e = Session::connect(Options { known_hosts: KnownHosts::Fingerprint(other.clone()), ..options(&server, &key) })
        .err()
        .unwrap();
    assert_eq!(e.kind(), ErrorKind::HostKey);
    assert!(e.message().contains(&server.fingerprint()) && e.message().contains(&other), "{e}");
}

#[test]
fn trust_on_first_use_records_then_knows() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let dir = TempDir::new("tofu");
    let known_hosts = dir.path().join(".ssh").join("known_hosts");

    let first = connect(Options { known_hosts: KnownHosts::RecordInto(known_hosts.clone()), ..options(&server, &key) });
    assert!(first.host_key().recorded);
    let text = std::fs::read_to_string(&known_hosts).unwrap();
    assert!(text.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 AAAA", server.port)), "{text}");
    assert_eq!(text.lines().count(), 1);

    // The recorded key now passes the strict policy, and is not recorded twice.
    let second = connect(Options { known_hosts: KnownHosts::File(known_hosts.clone()), ..options(&server, &key) });
    assert!(!second.host_key().recorded);
    assert!(second.run(&["true"]).unwrap().success());
    let third = connect(Options { known_hosts: KnownHosts::RecordInto(known_hosts.clone()), ..options(&server, &key) });
    assert!(!third.host_key().recorded);
    assert_eq!(std::fs::read_to_string(&known_hosts).unwrap(), text);
}

#[test]
fn a_changed_host_key_is_refused() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let dir = TempDir::new("changed");
    let known_hosts = dir.path().join("known_hosts");
    // What was recorded when another key served this host and port.
    let before = new_key().public_key().to_openssh().unwrap();
    let recorded = format!("[127.0.0.1]:{} {before}\n", server.port);
    std::fs::write(&known_hosts, &recorded).unwrap();

    for policy in [KnownHosts::File(known_hosts.clone()), KnownHosts::RecordInto(known_hosts.clone())] {
        let e = Session::connect(Options { known_hosts: policy, ..options(&server, &key) }).err().unwrap();
        assert_eq!(e.kind(), ErrorKind::HostKey);
        assert!(e.message().contains("CHANGED") && e.message().contains(&server.fingerprint()), "{e}");
        assert!(e.message().contains("line 1"), "{e}");
    }
    assert_eq!(std::fs::read_to_string(&known_hosts).unwrap(), recorded, "never rewritten");
    assert!(server.commands().is_empty());
}

#[test]
fn run_reports_stdout_stderr_and_status() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let session = connect(options(&server, &key));
    let output = session.run(&["sh", "-c", "echo out; echo err >&2; exit 3"]).unwrap();
    assert_eq!(output.stdout, b"out\n");
    assert_eq!(output.stderr, b"err\n");
    assert_eq!((output.status, output.signal.clone(), output.success()), (Some(3), None, false));
    assert_eq!(server.commands(), ["sh -c 'echo out; echo err >&2; exit 3'"]);

    let killed = session.run(&["sh", "-c", "kill -TERM $$"]).unwrap();
    assert_eq!((killed.status, killed.signal.as_deref()), (None, Some("TERM")));

    let empty: [&str; 0] = [];
    assert_eq!(session.run(&empty).unwrap_err().kind(), ErrorKind::Command);
    assert_eq!(session.run(&["echo", "a\0b"]).unwrap_err().kind(), ErrorKind::Command);
    assert_eq!(server.commands().len(), 2, "refused commands never reach the server");
}

#[test]
fn hostile_arguments_stay_single_arguments() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let session = connect(options(&server, &key));
    let hostile = [
        "; touch pwned1",
        "$(touch pwned2)",
        "`touch pwned3`",
        "| touch pwned4",
        "&& touch pwned5",
        "a'b",
        "'; touch pwned6; '",
        "line1\ntouch pwned7",
        "",
        "; rm -rf /nonexistent-grenat-canary",
        "*",
        "$HOME",
        "~",
        "A=b",
        "\\",
        "\"",
        "#",
    ];
    let mut argv = vec!["printf", "[%s]\\n"];
    argv.extend(hostile);
    let output = session.run(&argv).unwrap();
    let expected: String = hostile.iter().map(|arg| format!("[{arg}]\n")).collect();
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    assert!(output.success());
    let pwned: Vec<_> = std::fs::read_dir(server.home.path()).unwrap().collect();
    assert!(pwned.is_empty(), "the shell ran something: {pwned:?}");
    assert_eq!(server.commands(), [grenat_ssh::quote::command_line(&argv).unwrap()]);
}

#[test]
fn through_a_socks5_proxy() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let proxy = Socks::start(None);
    let session =
        connect(Options { proxy: Some(format!("socks5://127.0.0.1:{}", proxy.port)), ..options(&server, &key) });
    assert_eq!(session.run(&["echo", "proxied"]).unwrap().stdout, b"proxied\n");
    assert_eq!(proxy.targets(), [format!("127.0.0.1:{}", server.port)]);

    let guarded = Socks::start(Some(("bob", "pr0xy")));
    let url = |password: &str| format!("socks5://bob:{password}@127.0.0.1:{}", guarded.port);
    let session = connect(Options { proxy: Some(url("pr0xy")), ..options(&server, &key) });
    assert!(session.run(&["true"]).unwrap().success());

    let e = Session::connect(Options { proxy: Some(url("wrong-pw")), ..options(&server, &key) }).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Proxy);
    assert!(e.message().contains("refused the user name or password") && !e.message().contains("wrong-pw"), "{e}");

    let e = Session::connect(Options {
        proxy: Some(format!("socks5://127.0.0.1:{}", guarded.port)),
        ..options(&server, &key)
    })
    .err()
    .unwrap();
    assert!(e.message().contains("requires authentication"), "{e}");

    let unreachable = format!("socks5://127.0.0.1:{}", server::closed_port());
    let e = Session::connect(Options { proxy: Some(unreachable), ..options(&server, &key) }).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Proxy);
    assert!(e.message().starts_with("cannot reach the proxy 127.0.0.1:"), "{e}");

    let e =
        Session::connect(Options { proxy: Some("http://proxy:80".into()), ..options(&server, &key) }).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Proxy);
}

#[test]
fn a_silent_server_times_out() {
    let (_listener, port) = server::silent_port();
    let started = Instant::now();
    let e = Session::connect(Options {
        port,
        known_hosts: KnownHosts::Fingerprint("SHA256:x".into()),
        timeout: Some(Duration::from_millis(300)),
        ..Options::new(USER, "127.0.0.1", Auth::Password(PASSWORD.into()))
    })
    .err()
    .unwrap();
    assert_eq!(e.kind(), ErrorKind::Timeout, "{e}");
    assert_eq!(e.message(), format!("connecting to 127.0.0.1:{port} took longer than 300 ms"));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn connection_errors() {
    let port = server::closed_port();
    let e = Session::connect(Options { port, ..Options::new(USER, "127.0.0.1", Auth::Password(PASSWORD.into())) })
        .err()
        .unwrap();
    assert_eq!(e.kind(), ErrorKind::Connect);
    assert!(e.message().starts_with(&format!("cannot connect to 127.0.0.1:{port}")), "{e}");

    let e = Session::connect(Options::new(USER, "bad host", Auth::Password(PASSWORD.into()))).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Connect);

    let e = Session::connect(Options::new(USER, "127.0.0.1", Auth::Key { text: "nope".into(), passphrase: None }))
        .err()
        .unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth, "the key is read before any connection");
}
