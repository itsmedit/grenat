//! `Ssh` and SFTP: the `ssh` effect on a host (E0300), what reaches a
//! server (E0412, E0414), what comes back (untrusted), typed records.

mod common;

use common::*;

const CONNECT: &str = "server = Ssh.connect(\"deploy@api.acme.com\", key: Credentials.fetch(:deploy, :ssh_key))\n";

/// `body` in a `main` using `effects`, after connecting.
fn main(effects: &str, body: &str) -> String {
    format!("def main uses {effects}\n  {CONNECT}{body}end\n")
}

#[test]
fn a_connection_is_an_ssh_effect_on_its_host() {
    clean(&main("ssh(\"api.acme.com\"), env", "  server.run([\"uptime\"])\n"));
    clean(&main("ssh, env", "  server.sftp.read(\"/etc/hostname\")\n"));
    let src = main("ssh(\"db.acme.com\"), env", "");
    let d = single(&src, "E0300", "Ssh.connect(\"deploy@api.acme.com\", key: Credentials.fetch(:deploy, :ssh_key))");
    assert!(d.message.contains("`ssh(\"api.acme.com\")`"), "{}", d.message);
    // a port or an IPv6 address: the host is what the capability names
    clean("def main uses ssh(\"api.acme.com\")\n  Ssh.connect(\"deploy@api.acme.com:2222\", password: \"x\")\nend\n");
    clean("def main uses ssh(\"::1\")\n  Ssh.connect(\"root@[::1]:22\", password: \"x\")\nend\n");
    // a session's methods reach its host, checked at run time
    let src = "def restart(s: SshSession) -> Bool\n  s.run([\"systemctl\", \"restart\", \"shop\"]).ok?\nend\ndef main\n  restart(nil)\nend\n";
    let d = single(src, "E0300", "restart(nil)");
    assert!(d.message.contains("`ssh`"), "{}", d.message);
}

#[test]
fn a_transfer_also_touches_a_local_file() {
    let src = main("ssh, env", "  server.upload(\"dist/app.tar.gz\", \"/srv/app.tar.gz\")\n");
    let d = single(&src, "E0300", "server.upload(\"dist/app.tar.gz\", \"/srv/app.tar.gz\")");
    assert!(d.message.contains("fs.read(\"dist/app.tar.gz\")"), "{}", d.message);
    clean(&main("ssh, env, fs.read(\"dist\")", "  server.upload(\"dist/app.tar.gz\", \"/srv/app.tar.gz\")\n"));
    let src = main("ssh, env, fs.read", "  server.sftp.download(\"/var/log/app.log\", \"logs/app.log\")\n");
    let d = single(&src, "E0300", "server.sftp.download(\"/var/log/app.log\", \"logs/app.log\")");
    assert!(d.message.contains("fs.write(\"logs/app.log\")"), "{}", d.message);
}

#[test]
fn nothing_untrusted_reaches_the_server() {
    let head = "  out = server.run([\"cat\", \"/etc/motd\"]).stdout\n";
    for (sink, at) in [
        ("server.run([\"echo\", out])", "[\"echo\", out]"),
        ("server.sftp.write(\"/tmp/x\", out)", "out"),
        ("server.sftp.remove(out)", "out"),
        ("server.upload(\"a\", out)", "out"),
        ("Ssh.connect(\"deploy@#{out}\", password: \"x\")", "\"deploy@#{out}\""),
    ] {
        let src = main("ssh, env, fs", &format!("{head}  {sink}\n"));
        let d = single(&src, "E0412", at);
        assert!(d.message.contains("(effect `ssh`)"), "{}", d.message);
    }
    clean(&main("ssh, env", &format!("{head}  server.run([\"echo\", out.check {{ |o| o.size < 80 }}?])\n")));
}

#[test]
fn what_the_server_says_is_untrusted() {
    for (read, at) in [
        ("server.run([\"hostname\"]).stdout", "server.run([\"hostname\"]).stdout"),
        ("server.run([\"hostname\"]).stderr", "server.run([\"hostname\"]).stderr"),
        ("server.sftp.read(\"/etc/hostname\")", "server.sftp.read(\"/etc/hostname\")"),
        ("server.sftp.list(\"/srv\").first.name", "server.sftp.list(\"/srv\")"),
        ("server.run([\"hostname\"]).signal.to_s", "server.run([\"hostname\"]).signal"),
    ] {
        let src = main("ssh, env, shell", &format!("  x = {read}\n  Shell.run([\"echo\", x])\n"));
        let d = single(&src, "E0412", "[\"echo\", x]");
        assert_eq!(&src[d.notes[0].0.range()], at);
    }
    // once the listing is trusted, so are its entries' names
    clean(&main(
        "ssh, env, shell",
        "  entries = server.sftp.list(\"/srv\").trust!\n  Shell.run([\"echo\", entries.first.name])\n  names = entries.map { |e| e.name }\n  Shell.run([\"echo\", names.first])\n",
    ));
    // a status, a flag, a size: not text a server could inject through
    clean(&main(
        "ssh, env, shell",
        "  res = server.run([\"true\"])\n  sftp = server.sftp\n  Shell.run([\"echo\", \"#{res.status} #{res.ok?} #{sftp.exists?(\"/x\")}\"])\n",
    ));
}

#[test]
fn a_secret_only_serves_to_connect() {
    clean(
        "def main uses ssh, env\n  Ssh.connect(\"a@b\", key: Credentials.fetch(:k, :key), passphrase: Credentials.fetch(:k, :pass), proxy: Credentials.fetch(:proxy, :url))\nend\n",
    );
    let src = main("ssh, env", "  server.sftp.read(Credentials.fetch(:app, :path))\n");
    let d = single(&src, "E0414", "Credentials.fetch(:app, :path)");
    assert!(d.message.contains("reaches an SSH server through `read`"), "{}", d.message);
    let src = main("ssh, env", "  t = Credentials.fetch(:db, :password)\n  server.run([t, t])\n");
    single(&src, "E0414", "[t, t]");
    // among plain values, in a literal (nested or not), or where one may be nil
    for (sink, at) in [
        ("server.run([\"mysql\", \"-p\", t])", "[\"mysql\", \"-p\", t]"),
        ("server.run([\"echo\", Credentials.dig(:a, :b)])", "[\"echo\", Credentials.dig(:a, :b)]"),
        ("server.sftp.write(\"/srv/.env\", {user: \"app\", password: t})", "{user: \"app\", password: t}"),
        ("server.sftp.write(\"/srv/x\", [\"a\", [\"b\", t]])", "[\"a\", [\"b\", t]]"),
        ("server.sftp.write(\"/srv/x\", {\"a\" => [1, t]})", "{\"a\" => [1, t]}"),
        ("server.run([\"echo\", \"pw: #{t}\"])", "[\"echo\", \"pw: #{t}\"]"),
    ] {
        let src = main("ssh, env", &format!("  t = Credentials.fetch(:db, :password)\n  {sink}\n"));
        let d = single(&src, "E0414", at);
        assert!(d.message.contains("reaches an SSH server"), "{sink}: {}", d.message);
    }
    clean(&main(
        "ssh, env",
        "  t = Credentials.fetch(:db, :password)\n  server.run([\"mysql\", \"-p\", \"x\", [1, 2]])\n",
    ));
    single(
        &main("ssh, env", "  server.sftp.write(\"/srv/.env\", Credentials.fetch(:app, :env))\n"),
        "E0414",
        "Credentials.fetch(:app, :env)",
    );
}

#[test]
fn records_are_typed() {
    clean(
        "\
def report(s: SshSession) -> String uses ssh
  res = s.run([\"uptime\"])
  sftp = s.sftp
  entries = sftp.list(\"/var/log\").trust!
  big = entries.select { |e| !e.dir? && e.size > 1000 }.map { |e| e.name }
  sftp.mkdir(\"/srv/new\")
  sftp.rename(\"/srv/a\", \"/srv/b\")
  s.close
  \"#{s.user}@#{s.host}:#{s.port} #{res.status} #{res.signal} #{big.size} #{entries.first.modified}\"
end
test \"mocked\" do
  mock_ssh \"deploy@api.acme.com\", commands: {\"uptime\" => \"up\"}, files: {\"/srv/a\" => \"x\"}
end
",
    );
    let d = single(&main("ssh, env", "  server.reboot\n"), "E0200", "reboot");
    assert_eq!(d.message, "unknown method `reboot` for `SshSession`");
    single(&main("ssh, env", "  server.run([\"x\"]).exit_code\n"), "E0200", "exit_code");
    single(&main("ssh, env", "  server.sftp.chmod(\"/x\")\n"), "E0200", "chmod");
    let d = single(
        "def f(x: SshResult) -> Int = x.status + 1\ndef g(e: SftpEntry) -> String = e.size\n",
        "E0200",
        "e.size",
    );
    assert!(d.message.contains("String"), "{}", d.message);
}
