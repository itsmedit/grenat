# Grenat — SSH and SFTP (`Ssh.connect`)

Read this when a program runs commands on, or copies files to, another machine over SSH. It assumes the core reference, `llms.txt`.

```ruby
def uptime -> String uses ssh("api.acme.com"), env
  server = Ssh.connect("deploy@api.acme.com", key: Credentials.fetch(:deploy, :ssh_key))
  res = server.run(["uptime"])  # an argument vector, never a shell line
  res.stdout.check { |o| o.size < 1000 }?
end
```

- `Ssh.connect("user@host", key: secret)` (or `password:`; `port:`) → `run([...])` with `status`, `ok?`, `stdout`, `stderr` (untrusted); a failing command is an answer, not an error. `upload(local, remote)`, `download(remote, local)`, `sftp` (`list`, `read`, `write`, `remove`, `mkdir`, `rename`, `exists?`), `close`.
- The effect names the host, without the user: `ssh("api.acme.com")`; the key comes from `Credentials.fetch` (a `Secret`).
- Errors: `SshError`, `HostKeyError`, `SshAuthError`, `SftpError`, `TimeoutError`.

## Tests

```ruby
test "uptime" do
  mock_ssh "deploy@api.acme.com", commands: {"uptime" => "up 3 days"}
  assert_equal "up 3 days", uptime
end
```
