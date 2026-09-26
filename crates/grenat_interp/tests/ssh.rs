//! `Ssh` and SFTP: against a real server in process (`grenat_ssh::fake`),
//! and against `mock_ssh` in tests; host keys, credentials as secrets,
//! capabilities, taint, logs.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, run_tests};
use grenat_ssh::fake::socks::Socks;
use grenat_ssh::fake::{self, PASSWORD, TestServer, key_text, new_key};

/// A server letting in the key of `deploy.ssh_key` (in the credentials the
/// program's first line gives), and how to connect to it.
struct Remote {
    server: TestServer,
    /// The program's first lines: its credentials and `server = Ssh.connect(…)`.
    prelude: String,
}

fn remote() -> Remote {
    remote_with("")
}

/// `Ssh.connect` given more options (`, proxy: …`).
fn remote_with(options: &str) -> Remote {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let prelude = format!(
        "mock_credentials({{\"deploy\" => {{\"ssh_key\" => {:?}, \"password\" => \"{PASSWORD}\"}}}})\n\
         server = Ssh.connect(\"alice@127.0.0.1\", port: {}, key: Credentials.fetch(:deploy, :ssh_key), \
         fingerprint: \"{}\", timeout: 10{options})\n",
        key_text(&key),
        server.port,
        server.fingerprint()
    );
    Remote { server, prelude }
}

/// `connect` with options of the test's choosing, to a server with any key.
fn connect_to(server: &TestServer, options: &str) -> String {
    format!(
        "mock_credentials({{\"deploy\" => {{\"password\" => \"{PASSWORD}\", \"wrong\" => \"guess-123\"}}}})\n\
         server = Ssh.connect(\"alice@127.0.0.1\", port: {}, {options})\n",
        server.port
    )
}

fn error(src: &str) -> String {
    let e = run_err(src, Vec::new());
    format!("{}: {}", e.ty, e.message)
}

#[test]
fn commands_run_on_the_server() {
    let r = remote();
    let out = run(&format!(
        "{}\
p server
res = server.run([\"echo\", \"hello world\"])
p res.status, res.ok?, res.stdout, res.signal
bad = server.run([\"sh\", \"-c\", \"echo oops >&2; exit 3\"])
p bad.status, bad.ok?, bad.stderr
p server.run([\"sh\", \"-c\", \"kill -TERM $$\"]).signal
p server.run([\"printf\", \"[%s]\", \"; touch pwned\", \"$(id)\", 42]).stdout.trust!
server.close
server.close
",
        r.prelude
    ));
    assert_eq!(
        out,
        format!(
            "SshSession(id: 0, user: \"alice\", host: \"127.0.0.1\", port: {})\n0\ntrue\n~\"hello world\\n\"\nnil\n\
             3\nfalse\n~\"oops\\n\"\n~\"TERM\"\n\"[; touch pwned][$(id)][42]\"\n",
            r.server.port
        )
    );
    // each argument arrived as one argument: nothing ran through the shell
    assert_eq!(std::fs::read_dir(r.server.home.path()).unwrap().count(), 0);
    assert_eq!(r.server.commands()[0], "echo 'hello world'");
    let e = error(&format!("{}server.close\nserver.run([\"true\"])\n", r.prelude));
    assert_eq!(e, "SshError: the SSH connection to alice@127.0.0.1 is closed");
}

#[test]
fn files_over_sftp() {
    let r = remote();
    let out = run(&format!(
        "{}\
sftp = server.sftp
p sftp
p sftp.exists?(\"notes.txt\")
sftp.write(\"notes.txt\", \"first\")
p sftp.exists?(\"notes.txt\"), sftp.read(\"notes.txt\")
sftp.mkdir(\"docs\")
sftp.rename(\"notes.txt\", \"docs/n.txt\")
entries = sftp.list(\"docs\")
p entries.tainted?
e = entries.first.trust!
p e.name, e.size, e.dir?, e.modified > 0
p sftp.list(\"/\").trust!.map {{ |x| x.name }}
sftp.remove(\"docs/n.txt\")
sftp.remove(\"docs\")
p sftp.exists?(\"docs\")
",
        r.prelude
    ));
    assert_eq!(
        out,
        "Sftp(id: 0, host: \"127.0.0.1\")\nfalse\ntrue\n~\"first\"\ntrue\n\"n.txt\"\n5\nfalse\ntrue\n[\"docs\"]\nfalse\n"
    );
    let e = error(&format!("{}server.sftp.read(\"missing.txt\")\n", r.prelude));
    assert_eq!(e, "SftpError: cannot read `missing.txt`: no such file or directory");
}

#[test]
fn uploads_and_downloads() {
    let r = remote();
    let local = temp_dir("ssh-transfers");
    std::fs::write(local.join("app.tar.gz"), [0u8, 1, 2, 255]).unwrap();
    let (up, back) = (local.join("app.tar.gz"), local.join("back.tar.gz"));
    run(&format!(
        "{}server.upload(\"{}\", \"app.tar.gz\")\nserver.sftp.download(\"app.tar.gz\", \"{}\")\n",
        r.prelude,
        up.display(),
        back.display()
    ));
    assert_eq!(std::fs::read(r.server.home.path().join("app.tar.gz")).unwrap(), [0, 1, 2, 255]);
    assert_eq!(std::fs::read(&back).unwrap(), [0, 1, 2, 255]);
    // the local side is a file system effect
    let src = format!(
        "{}def fetch(s: SshSession) uses ssh, fs.write(\"{dir}/in\")\n  s.download(\"app.tar.gz\", \"{dir}/out/x\")\nend\nfetch(server)\n",
        r.prelude,
        dir = local.display()
    );
    let e = error(&src);
    assert!(e.starts_with("CapabilityError: `fs.write` on `"), "{e}");
    let src = format!(
        "{}def push(s: SshSession) uses ssh, fs.write\n  s.upload(\"{}\", \"x\")\nend\npush(server)\n",
        r.prelude,
        up.display()
    );
    assert!(error(&src).starts_with("CapabilityError: `fs.read` on `"));
}

#[test]
fn host_keys_are_verified() {
    let key = new_key();
    let server = TestServer::start(key.public_key());
    let dir = temp_dir("ssh-known-hosts");
    let known_hosts = dir.join("known_hosts");
    std::fs::write(&known_hosts, "").unwrap();
    let e = error(&connect_to(
        &server,
        &format!("password: Credentials.fetch(:deploy, :password), known_hosts: \"{}\"", known_hosts.display()),
    ));
    assert!(e.starts_with("HostKeyError: the host key of 127.0.0.1:"), "{e}");
    assert!(e.contains(&format!("is unknown: it offers ssh-ed25519 {}", server.fingerprint())), "{e}");
    assert!(
        e.ends_with(&format!(
            "then pass `fingerprint: \"{}\"` to `Ssh.connect`, or `known_hosts:` a file that records it",
            server.fingerprint()
        )),
        "{e}"
    );
    let e = error(&connect_to(
        &server,
        "password: Credentials.fetch(:deploy, :password), fingerprint: \"SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\"",
    ));
    assert!(e.starts_with("HostKeyError: ") && e.contains("not the expected SHA256:AAAA"), "{e}");
    assert!(server.commands().is_empty());
    // a file recording the key
    let line = format!("[127.0.0.1]:{} {}\n", server.port, server.host_key.public_key().to_openssh().unwrap());
    std::fs::write(&known_hosts, line).unwrap();
    let out = run(&format!(
        "{}p server.run([\"echo\", \"known\"]).stdout.trust!\n",
        connect_to(
            &server,
            &format!("password: Credentials.fetch(:deploy, :password), known_hosts: \"{}\"", known_hosts.display())
        )
    ));
    assert_eq!(out, "\"known\\n\"\n");
}

#[test]
fn credentials_are_secrets_never_shown() {
    let server = TestServer::start(new_key().public_key());
    let fingerprint = format!("fingerprint: \"{}\"", server.fingerprint());
    let out = run(&format!(
        "{}p server.run([\"whoami\"]).ok?\n",
        connect_to(&server, &format!("password: Credentials.fetch(:deploy, :password), {fingerprint}"))
    ));
    assert_eq!(out, "true\n");
    let e = error(&connect_to(&server, &format!("password: Credentials.fetch(:deploy, :wrong), {fingerprint}")));
    assert!(e.starts_with("SshAuthError: ") && e.contains("refused the password of `alice`"), "{e}");
    assert!(!e.contains("guess-123"), "{e}");
    let e = error(&connect_to(&server, &format!("key: \"not a key\", {fingerprint}")));
    assert!(e.starts_with("SshAuthError: the private key"), "{e}");
    // a secret goes nowhere but the connection
    let r = remote();
    let e = error(&format!("{}server.run([\"echo\", Credentials.fetch(:deploy, :password)])\n", r.prelude));
    assert!(e.starts_with("SecretError: a secret never goes to the server through `run`"), "{e}");
    let e =
        error(&format!("{}server.sftp.write(\"x\", \"pw: #{{Credentials.fetch(:deploy, :password)}}\")\n", r.prelude));
    assert!(e.starts_with("SecretError: "), "{e}");
    assert_eq!(r.server.commands(), Vec::<String>::new());
}

#[test]
fn through_a_socks5_proxy() {
    let proxy = Socks::start(None);
    let r = remote_with(&format!(", proxy: \"socks5://127.0.0.1:{}\"", proxy.port));
    assert_eq!(run(&format!("{}p server.run([\"echo\", \"via\"]).stdout.trust!\n", r.prelude)), "\"via\\n\"\n");
    assert_eq!(proxy.targets(), [format!("127.0.0.1:{}", r.server.port)]);
    // a proxy that is a secret is never named
    let port = fake::closed_port();
    let src = format!(
        "mock_credentials({{\"proxy\" => {{\"url\" => \"socks5://bob:pr0xy@127.0.0.1:{port}\"}}}})\n\
         Ssh.connect(\"alice@127.0.0.1\", password: \"x\", fingerprint: \"SHA256:x\", proxy: Credentials.fetch(:proxy, :url))\n"
    );
    let e = error(&src);
    assert!(e.starts_with("SshError: cannot reach the proxy [secret]: "), "{e}");
    assert!(!e.contains(&port.to_string()) && !e.contains("pr0xy"), "{e}");
    let e = error("Ssh.connect(\"alice@127.0.0.1\", password: \"x\", proxy: \"http://proxy:3128\")\n");
    assert!(e.starts_with("ArgumentError: `Ssh.connect` goes through SOCKS5 proxies only"), "{e}");
    // nor are the credentials of a proxy refused
    for (proxy, shown) in [
        ("\"http://bob:hunter2@proxy.corp:8080\"", "got \"http://proxy.corp:8080\""),
        ("\"bob:hunter2@proxy.corp:8080\"", "got \"proxy.corp:8080\""),
        ("Credentials.fetch(:proxy, :url)", "got [secret]"),
    ] {
        let e = error(&format!(
            "mock_credentials({{\"proxy\" => {{\"url\" => \"http://bob:hunter2@proxy.corp:8080\"}}}})\n\
             Ssh.connect(\"alice@127.0.0.1\", password: \"x\", proxy: {proxy})\n"
        ));
        assert!(e.starts_with("ArgumentError: `Ssh.connect` goes through SOCKS5 proxies only"), "{e}");
        assert!(e.ends_with(shown) && !e.contains("hunter2") && !e.contains("bob"), "{e}");
    }
}

#[test]
fn logs_name_no_secret() {
    let proxy = Socks::start(Some(("bob", "pr0xy")));
    let r = remote();
    let src = format!(
        "{}server.run([\"echo\", \"a\"])\nserver.sftp.write(\"f\", \"x\")\nserver.sftp.exists?(\"f\")\n\
         mock_credentials({{\"proxy\" => {{\"url\" => \"socks5://bob:pr0xy@127.0.0.1:{}\"}}}})\n\
         Ssh.connect(\"alice@127.0.0.1\", port: {}, password: \"{PASSWORD}\", fingerprint: \"{}\", proxy: Credentials.fetch(:proxy, :url))\n",
        r.prelude,
        proxy.port,
        r.server.port,
        r.server.fingerprint()
    );
    let log = run_mode(&src, grenat_interp::Scripted::new([]), &[], &[], Mode { log: true, ..Mode::default() }).ok();
    let label = format!("[ssh] alice@127.0.0.1:{}: connected (ssh-ed25519 {})", r.server.port, r.server.fingerprint());
    assert!(log.contains(&format!("{label}\n")), "{log}");
    assert!(log.contains("[ssh] alice@127.0.0.1: echo a → 0\n"), "{log}");
    assert!(log.contains("[ssh] alice@127.0.0.1: write f\n"), "{log}");
    assert!(log.contains("[ssh] alice@127.0.0.1: exists? f → true\n"), "{log}");
    assert!(log.contains(&format!("{label} via [secret]\n")), "{log}");
    assert!(!log.contains("pr0xy") && !log.contains("BEGIN OPENSSH"), "{log}");
}

#[test]
fn servers_are_capabilities() {
    let r = remote();
    let src = format!(
        "{}\
def restart(s: SshSession) uses ssh(\"127.0.0.1\")
  s.run([\"true\"]).ok?
end
def elsewhere(s: SshSession) uses ssh(\"api.acme.com\")
  s.sftp.exists?(\"x\")
end
def open uses ssh(\"api.acme.com\")
  Ssh.connect(\"alice@127.0.0.1\", password: \"x\")
end
p restart(server)
elsewhere(server)
",
        r.prelude
    );
    let run = run_with(&src, Vec::new(), &[]);
    assert_eq!(run.output, "true\n");
    let e = run.err();
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("CapabilityError", "`ssh` to `127.0.0.1` is not allowed by `elsewhere` (uses ssh(\"api.acme.com\"))")
    );
    let e = error(&src.replace("p restart(server)\nelsewhere(server)\n", "open\n"));
    assert_eq!(e, "CapabilityError: `ssh` to `127.0.0.1` is not allowed by `open` (uses ssh(\"api.acme.com\"))");
}

#[test]
fn nothing_untrusted_reaches_the_server() {
    let r = remote();
    let head = format!("{}out = server.run([\"echo\", \"a\"]).stdout\n", r.prelude);
    for sink in [
        "server.run([\"echo\", out])",
        "server.sftp.write(\"f\", out)",
        "server.sftp.write(out, \"x\")",
        "server.sftp.read(out)",
        "server.upload(out, \"x\")",
        "server.sftp.rename(\"a\", out)",
        "Ssh.connect(\"alice@#{out}\", password: \"x\")",
    ] {
        let e = error(&format!("{head}{sink}\n"));
        assert!(e.starts_with("TaintError: an untrusted value reaches `"), "{sink}: {e}");
    }
    assert!(r.server.commands().iter().all(|c| c == "echo a"), "{:?}", r.server.commands());
    // the name of a signal is the server's word too
    let src = format!(
        "{}sig = server.run([\"sh\", \"-c\", \"kill -USR1 $$\"]).signal\np sig.tainted?\nserver.run([\"echo\", sig])\n",
        r.prelude
    );
    let signalled = run_with(&src, Vec::new(), &[]);
    assert_eq!(signalled.output, "true\n");
    let e = signalled.err();
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("TaintError", "an untrusted value reaches `run` (effect `ssh`) without validation")
    );
    assert!(!r.server.commands().iter().any(|c| c.starts_with("echo S")), "{:?}", r.server.commands());
    // validated, it may
    let out = run(&format!("{head}p server.run([\"echo\", out.check {{ |o| o.size < 5 }}?]).stdout\n"));
    assert_eq!(out, "~\"a\\n\\n\"\n");
}

#[test]
fn connection_options_are_checked() {
    for (src, expected) in [
        (
            "Ssh.connect(\"api.acme.com\", password: \"x\")",
            "ArgumentError: `Ssh.connect` expects \"user@host\", got \"api.acme.com\"",
        ),
        ("Ssh.connect(\"a@b\")", "ArgumentError: `Ssh.connect` needs credentials"),
        (
            "Ssh.connect(\"a@b\", key: \"k\", password: \"p\")",
            "ArgumentError: `Ssh.connect` takes `key:` or `password:`, not both",
        ),
        (
            "Ssh.connect(\"a@b\", password: \"p\", passphrase: \"x\")",
            "ArgumentError: `Ssh.connect`: a `passphrase:` goes with a `key:`",
        ),
        (
            "Ssh.connect(\"a@b:22\", password: \"p\", port: 2222)",
            "ArgumentError: `Ssh.connect`: the port is given twice",
        ),
        (
            "Ssh.connect(\"a@b\", password: \"p\", fingerprint: \"f\", known_hosts: \"k\")",
            "ArgumentError: `Ssh.connect` takes `known_hosts:` or `fingerprint:`, not both",
        ),
        ("Ssh.connect(\"a@b\", password: \"p\", port: 0)", "ArgumentError: invalid `Ssh.connect` option `port: 0`"),
        (
            "Ssh.connect(\"a@b\", password: \"p\", timeout: 1e20)",
            "ArgumentError: invalid `Ssh.connect` option `timeout: 100000000000000000000`",
        ),
        (
            "Ssh.connect(\"a@b\", password: \"p\", timeout: 1e400)",
            "ArgumentError: invalid `Ssh.connect` option `timeout: ",
        ),
        (
            "Ssh.connect(\"a@b\", password: \"p\", timeout: -1.5)",
            "ArgumentError: invalid `Ssh.connect` option `timeout: -1.5`",
        ),
        (
            "Ssh.connect(\"a@b\", password: \"p\", color: :red)",
            "ArgumentError: invalid `Ssh.connect` option `color: :red`",
        ),
        ("Ssh.open(\"a@b\")", "NoMethodError: unknown method `Ssh.open`"),
    ] {
        let e = error(&format!("{src}\n"));
        assert!(e.starts_with(expected), "{src}: {e}");
    }
    let port = fake::closed_port();
    let e = error(&format!("Ssh.connect(\"alice@127.0.0.1:{port}\", password: \"p\", fingerprint: \"SHA256:x\")\n"));
    assert!(e.starts_with(&format!("SshError: cannot connect to 127.0.0.1:{port}")), "{e}");
    let (_listener, port) = fake::silent_port();
    let e = error(&format!(
        "Ssh.connect(\"alice@127.0.0.1\", port: {port}, password: \"p\", fingerprint: \"SHA256:x\", timeout: 0.3)\n"
    ));
    assert_eq!(e, format!("TimeoutError: connecting to 127.0.0.1:{port} took longer than 300 ms"));
}

/// The outcome of each test of `src`, run as `grenat test` does.
fn tests_of(src: &str) -> Vec<Option<String>> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let outcomes = run_tests(&parsed.program, options).unwrap();
    outcomes.into_iter().map(|o| o.error.map(|e| format!("{}: {}", e.ty, e.message))).collect()
}

#[test]
fn tests_reach_no_server() {
    let local = temp_dir("ssh-mock");
    std::fs::write(local.join("app.tar.gz"), "archive").unwrap();
    let src = format!(
        "\
def deploy(server: SshSession) -> Bool uses ssh(\"api.acme.com\"), fs.read
  server.upload(\"{dir}/app.tar.gz\", \"/srv/app.tar.gz\")
  server.run([\"systemctl\", \"restart\", \"shop\"]).ok?
end
test \"mocked\" do
  mock_ssh \"deploy@api.acme.com\", commands: {{\"systemctl restart shop\" => \"done\", \"false\" => {{stderr: \"no\", status: 1}}, \"cat *\" => \"any\"}}, files: {{\"/srv/VERSION\" => \"1.2\"}}
  server = Ssh.connect(\"deploy@api.acme.com\", key: Credentials.fetch(:deploy, :ssh_key))
  assert deploy(server)
  assert_equal \"done\", server.run([\"systemctl\", \"restart\", \"shop\"]).stdout.trust!
  res = server.run([\"false\"])
  assert_equal [1, false, \"no\"], [res.status, res.ok?, res.stderr.trust!]
  assert_equal \"any\", server.run([\"cat\", \"/etc/hosts\"]).stdout.trust!
  sftp = server.sftp
  assert_equal \"1.2\", sftp.read(\"/srv/VERSION\").trust!
  assert_equal [\"VERSION\", \"app.tar.gz\"], sftp.list(\"/srv\").trust!.map {{ |e| e.name }}
  sftp.write(\"/srv/notes\", \"hi\")
  sftp.download(\"/srv/app.tar.gz\", \"{dir}/back.tar.gz\")
  assert sftp.exists?(\"/srv/notes\")
  server.close
end
test \"a command it was not told about\" do
  mock_ssh \"deploy@api.acme.com\", commands: {{}}
  Ssh.connect(\"deploy@api.acme.com\", password: \"x\").run([\"rm\", \"-rf\", \"/\"])
end
test \"a server not mocked\" do
  Ssh.connect(\"deploy@api.acme.com\", password: \"x\")
end
test \"mocks are the test's own\" do
  Ssh.connect(\"deploy@api.acme.com\", password: \"x\")
end
test \"a later mock reaches an open session\" do
  mock_ssh \"deploy@api.acme.com\", commands: {{\"uptime\" => \"up\"}}
  server = Ssh.connect(\"deploy@api.acme.com\", password: \"x\")
  mock_ssh \"deploy@api.acme.com\", commands: {{\"hostname\" => \"api\"}}, files: {{\"/etc/hostname\" => \"api\"}}
  assert_equal \"api\", server.run([\"hostname\"]).stdout.trust!
  assert_equal \"api\", server.sftp.read(\"/etc/hostname\").trust!
  server.run([\"uptime\"])
end
",
        dir = local.display()
    );
    assert_eq!(
        tests_of(&src),
        [
            None,
            Some("SshError: the mock of `deploy@api.acme.com` has no command `rm -rf /`".into()),
            Some("SshError: no SSH in tests: `deploy@api.acme.com` is not stubbed with `mock_ssh`".into()),
            Some("SshError: no SSH in tests: `deploy@api.acme.com` is not stubbed with `mock_ssh`".into()),
            Some("SshError: the mock of `deploy@api.acme.com` has no command `uptime`".into()),
        ]
    );
    assert_eq!(std::fs::read_to_string(local.join("back.tar.gz")).unwrap(), "archive");
    let e = error("mock_ssh \"x\"\n");
    assert!(e.starts_with("ArgumentError: `mock_ssh` expects a server"), "{e}");
    let e = error("mock_ssh \"a@b\", commands: {\"x\" => {code: 1}}\n");
    assert!(e.starts_with("ArgumentError: invalid `mock_ssh` command answer `code: 1`"), "{e}");
}
