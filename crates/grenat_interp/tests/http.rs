//! `Http`: real requests to a local server, capabilities, taint, `mock_http`.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, run_tests};
use serde_json::json;

/// A local HTTP server; returns its base URL (`http://127.0.0.1:port`).
fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut parts = first.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let (mut length, mut auth) = (0, String::new());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap(),
                        "authorization" => auth = value.trim().to_string(),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let (status, content) = match path.as_str() {
                    "/repo" => (200, json!({"name": "grenat", "stars": 42}).to_string()),
                    "/echo" => (200, json!({"method": method, "body": body, "auth": auth}).to_string()),
                    "/slow" => {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        (200, "late".to_string())
                    }
                    _ => (404, "no such page".to_string()),
                };
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nX-Rate: 99\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{content}",
                    content.len()
                );
            });
        }
    });
    base
}

fn run_http(src: &str) -> String {
    run(&src.replace("BASE", &server()))
}

#[test]
fn get_returns_status_headers_and_body() {
    let out = run_http(
        "\
res = Http.get(\"BASE/repo\")
p res.status, res.ok?
p res.json[\"stars\"]
p res.headers[\"x-rate\"]
missing = Http.get(\"BASE/nowhere\")
p missing.status, missing.ok?, missing.body
",
    );
    assert_eq!(out, "200\ntrue\n~42\n~\"99\"\n404\nfalse\n~\"no such page\"\n");
}

#[test]
fn requests_send_json_bodies_and_headers() {
    let out = run_http(
        "\
res = Http.post(\"BASE/echo\", json: {title: \"Bug\", labels: [\"a\"]}, headers: {\"Authorization\" => \"Bearer t0k\"})
echo = res.json.trust!
p echo[\"method\"], Json.parse(echo[\"body\"]), echo[\"auth\"]
p Http.put(\"BASE/echo\", body: \"raw\").json.trust![\"body\"]
p Http.patch(\"BASE/echo\").json.trust![\"method\"], Http.delete(\"BASE/echo\").json.trust![\"method\"]
",
    );
    assert_eq!(
        out,
        "\"POST\"\n{\"labels\" => [\"a\"], \"title\" => \"Bug\"}\n\"Bearer t0k\"\n\"raw\"\n\"PATCH\"\n\"DELETE\"\n"
    );
}

#[test]
fn network_failures_and_timeouts_are_http_errors() {
    let base = server();
    let e = run_err(&format!("Http.get(\"{base}/slow\", timeout: 0.3)\n"), Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert!(e.message.starts_with(&format!("GET {base}/slow: ")), "{}", e.message);
    let e = run_err("Http.get(\"http://127.0.0.1:1/x\")\n", Vec::new());
    assert_eq!(e.ty, "HttpError");
    let e = run_err("Http.get(\"ftp://x\")\n", Vec::new());
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("ArgumentError", "`Http.get` expects an http(s) URL, got \"ftp://x\"")
    );
    let e = run_err("Http.get(\"http://x\", retries: 3)\n", Vec::new());
    assert_eq!(e.message, "invalid `Http` option `retries: 3`");
}

#[test]
fn hosts_are_capabilities() {
    let src = "\
def github(url: String) -> Int uses net(\"api.github.com\")
  Http.get(url).status
end
def anywhere(url: String) -> Int uses net
  Http.get(url).status
end
p anywhere(\"BASE/repo\")
github(\"BASE/repo\")
";
    let src = src.replace("BASE", &server());
    let e = run_err(&src, Vec::new());
    assert_eq!(e.ty, "CapabilityError");
    assert!(e.message.contains("is not allowed by `github` (uses net(\"api.github.com\"))"), "{}", e.message);
}

#[test]
fn nothing_untrusted_goes_out_and_what_comes_back_is_untrusted() {
    let base = server();
    // a model's answer, unchecked, cannot be sent
    let src = format!("{SUMMARY}s = summarize(\"x\")\nHttp.post(\"{base}/echo\", json: {{title: s.title}})\n");
    let e = run_err(&src, vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
    assert!(e.message.starts_with("an untrusted value reaches `Http.post`"), "{}", e.message);
    // nor can a response be sent on as it is
    let e = run_err(&format!("r = Http.get(\"{base}/repo\")\nHttp.post(\"{base}/echo\", body: r.body)\n"), Vec::new());
    assert_eq!(e.ty, "TaintError");
    // once checked, it can
    let out = run(&format!(
        "r = Http.get(\"{base}/repo\")\nstars = r.json[\"stars\"].check {{ |n| n > 0 }}?\np Http.post(\"{base}/echo\", json: {{stars: stars}}).status\n"
    ));
    assert_eq!(out, "200\n");
    // parsing untrusted text gives untrusted data
    assert_eq!(run(&format!("p Json.parse(Http.get(\"{base}/repo\").body)[\"name\"]\n")), "~\"grenat\"\n");
}

fn tests_output(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        dir: Some(temp_dir("http")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

#[test]
fn tests_stub_the_network_and_never_reach_it() {
    let results = tests_output(
        "\
test \"stubbed\" do
  mock_http \"GET https://api.github.com/repos/acme/app\", json: {stars: 7}
  mock_http \"https://api.github.com/search/*\", status: 503, body: \"down\"
  assert_equal 7, Http.get(\"https://api.github.com/repos/acme/app\").json[\"stars\"].trust!
  assert_equal 503, Http.get(\"https://api.github.com/search/code?q=x\").status
  assert_equal 503, Http.post(\"https://api.github.com/search/x\").status
end
test \"method matters\" do
  mock_http \"GET https://api.github.com/repos/acme/app\", json: {}
  Http.post(\"https://api.github.com/repos/acme/app\")
end
test \"stubs do not outlive their test\" do
  Http.get(\"https://api.github.com/repos/acme/app\")
end
",
    );
    assert_eq!(results[0].1, None);
    assert_eq!(
        results[1].1.as_deref(),
        Some(
            "HttpError: no network in tests: `POST https://api.github.com/repos/acme/app` is not stubbed with `mock_http`"
        )
    );
    assert!(results[2].1.as_deref().unwrap().starts_with("HttpError: no network in tests"));
}

#[test]
fn a_model_and_the_network_together() {
    // the model's answer, checked, goes to a web service
    let base = server();
    let src = format!(
        "{SUMMARY}def main uses llm, net\n  s = summarize(\"x\").check {{ |s| s.title.size < 40 }}?\n  p Http.post(\"{base}/echo\", json: {{title: s.title}}).json.trust![\"body\"]\nend\n"
    );
    let out = run_with(&src, vec![summary_reply()], &[]).ok();
    assert_eq!(out, "\"{\\\"title\\\":\\\"Cat\\\"}\"\n");
    let _ = Response::text_reply("");
}

#[test]
fn a_page_as_text_stays_untrusted() {
    let base = server();
    let out = run(&format!("p Html.text(Http.get(\"{base}/nowhere\").body)\np Html.text(\"<b>a</b> &amp; b\")\n"));
    assert_eq!(out, "~\"no such page\"\n\"a & b\"\n");
}

#[test]
fn secrets_are_revealed_only_in_the_request() {
    let out = run_http(
        "\
mock_credentials({\"api\" => {\"token\" => \"t0k\"}})
token = Credentials.fetch(:api, :token)
res = Http.post(\"BASE/echo\", headers: {\"Authorization\" => \"Bearer #{token}\"}, json: {key: token})
echo = res.json.trust!
p echo[\"auth\"], Json.parse(echo[\"body\"])
puts \"Bearer #{token}\"
p token
p Json.dump({key: token})
",
    );
    assert_eq!(out, "\"Bearer t0k\"\n{\"key\" => \"t0k\"}\n[secret]\n[secret]\n\"{\\\"key\\\":\\\"[secret]\\\"}\"\n");
}

#[test]
fn an_error_does_not_name_a_url_holding_a_secret() {
    // nobody listens there: the request fails
    let src = "mock_credentials({\"api\" => {\"key\" => \"s3cr3t\"}})\nkey = Credentials.fetch(:api, :key)\nHttp.get(\"http://127.0.0.1:1/x?key=#{key}\")\n";
    let e = run_err(src, Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert!(!e.message.contains("s3cr3t"), "{}", e.message);
    assert!(e.message.contains("[secret]"), "{}", e.message);
}

// ── Proxies ──────────────────────────────────────────────────

/// A local proxy speaking SOCKS5, SOCKS4(a) and HTTP CONNECT on one port,
/// told apart by the first byte. It answers for `grenat.test` itself (a
/// name only the proxy can resolve), requires `credentials` when given, and
/// notes what it is asked (`auth u:p`, `socks5 grenat.test:80`…).
struct Proxy {
    address: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    fn start(credentials: Option<(&str, &str)>) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let credentials = credentials.map(|(u, p)| (u.to_string(), p.to_string()));
        let notes = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (notes, credentials) = (notes.clone(), credentials.clone());
                std::thread::spawn(move || {
                    let _ = serve_proxy(stream, credentials, &notes);
                });
            }
        });
        Proxy { address, seen }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

type Notes = Mutex<Vec<String>>;

fn serve_proxy(mut client: TcpStream, credentials: Option<(String, String)>, notes: &Notes) -> std::io::Result<()> {
    let target = match read_bytes(&mut client, 1)?[0] {
        5 => socks5(&mut client, credentials, notes)?,
        4 => socks4(&mut client, notes)?,
        _ => connect(&mut client, notes)?,
    };
    let Some(target) = target else { return Ok(()) };
    let upstream = TcpStream::connect(target.replace("grenat.test", "127.0.0.1"))?;
    let (mut up_read, mut client_write) = (upstream.try_clone()?, client.try_clone()?);
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut up_read, &mut client_write);
        let _ = client_write.shutdown(std::net::Shutdown::Both);
    });
    let mut upstream = upstream;
    let _ = std::io::copy(&mut client, &mut upstream);
    Ok(())
}

fn read_bytes(stream: &mut TcpStream, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0; n];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_text(stream: &mut TcpStream) -> std::io::Result<String> {
    let n = read_bytes(stream, 1)?[0] as usize;
    Ok(String::from_utf8_lossy(&read_bytes(stream, n)?).into_owned())
}

/// SOCKS5 (RFC 1928, RFC 1929); the version byte is read.
fn socks5(
    client: &mut TcpStream,
    credentials: Option<(String, String)>,
    notes: &Notes,
) -> std::io::Result<Option<String>> {
    let count = read_bytes(client, 1)?[0] as usize;
    let methods = read_bytes(client, count)?;
    let method = if credentials.is_some() { 2 } else { 0 };
    if !methods.contains(&method) {
        client.write_all(&[5, 0xFF])?;
        return Ok(None);
    }
    client.write_all(&[5, method])?;
    if let Some((user, password)) = credentials {
        read_bytes(client, 1)?;
        let (given_user, given_password) = (read_text(client)?, read_text(client)?);
        notes.lock().unwrap().push(format!("auth {given_user}:{given_password}"));
        if (given_user, given_password) != (user, password) {
            client.write_all(&[1, 1])?;
            return Ok(None);
        }
        client.write_all(&[1, 0])?;
    }
    let head = read_bytes(client, 4)?;
    let host = match head[3] {
        1 => read_bytes(client, 4)?.iter().map(u8::to_string).collect::<Vec<_>>().join("."),
        3 => read_text(client)?,
        _ => return Ok(None),
    };
    let port = u16::from_be_bytes(read_bytes(client, 2)?.try_into().unwrap());
    let target = format!("{host}:{port}");
    notes.lock().unwrap().push(format!("socks5 {target}"));
    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])?;
    Ok(Some(target))
}

/// SOCKS4 and SOCKS4a; the version byte is read.
fn socks4(client: &mut TcpStream, notes: &Notes) -> std::io::Result<Option<String>> {
    let head = read_bytes(client, 7)?;
    let port = u16::from_be_bytes([head[1], head[2]]);
    let until_nul = |client: &mut TcpStream| -> std::io::Result<String> {
        let mut text = Vec::new();
        loop {
            match read_bytes(client, 1)?[0] {
                0 => return Ok(String::from_utf8_lossy(&text).into_owned()),
                b => text.push(b),
            }
        }
    };
    until_nul(client)?;
    let host = match &head[3..7] {
        [0, 0, 0, _] => until_nul(client)?,
        ip => ip.iter().map(u8::to_string).collect::<Vec<_>>().join("."),
    };
    let target = format!("{host}:{port}");
    notes.lock().unwrap().push(format!("socks4 {target}"));
    client.write_all(&[0, 0x5A, 0, 0, 0, 0, 0, 0])?;
    Ok(Some(target))
}

/// An HTTP proxy: `CONNECT host:port`; the first byte is read.
fn connect(client: &mut TcpStream, notes: &Notes) -> std::io::Result<Option<String>> {
    let mut head = vec![b'C'];
    while !head.ends_with(b"\r\n\r\n") {
        head.push(read_bytes(client, 1)?[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let target = head.split_whitespace().nth(1).unwrap_or_default().to_string();
    notes.lock().unwrap().push(format!("connect {target}"));
    if let Some(auth) = head.lines().find(|l| l.to_ascii_lowercase().starts_with("proxy-authorization:")) {
        notes.lock().unwrap().push(auth.to_string());
    }
    client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
    Ok(Some(target))
}

fn port_of(base: &str) -> String {
    base.rsplit(':').next().unwrap().to_string()
}

/// `src` run with `--log`: its output and log lines.
fn logged(src: &str) -> String {
    run_mode(src, grenat_interp::Scripted::new([]), &[], &[], Mode { log: true, ..Mode::default() }).ok()
}

/// A program whose credentials hold `url` as `proxy.url`, then `rest`.
fn with_secret_proxy(url: &str, rest: &str) -> String {
    format!(
        "mock_credentials({{\"proxy\" => {{\"url\" => \"{url}\"}}}})\nproxy = Credentials.fetch(:proxy, :url)\n{rest}"
    )
}

#[test]
fn requests_go_through_a_socks5_proxy() {
    let (base, proxy) = (server(), Proxy::start(None));
    let port = port_of(&base);
    let out = run(&format!(
        "p Http.get(\"{base}/repo\", proxy: \"socks5://{0}\").json[\"stars\"]\np Http.post(\"{base}/echo\", body: \"hi\", proxy: \"socks5://{0}\").json.trust![\"body\"]\n",
        proxy.address
    ));
    assert_eq!(out, "~42\n\"hi\"\n");
    assert_eq!(proxy.seen(), [format!("socks5 127.0.0.1:{port}"), format!("socks5 127.0.0.1:{port}")]);
}

#[test]
fn socks5h_lets_the_proxy_resolve_the_host() {
    let (base, proxy) = (server(), Proxy::start(None));
    let port = port_of(&base);
    // `grenat.test` exists only for the proxy
    let url = format!("http://grenat.test:{port}/repo");
    let out = run(&format!("p Http.get(\"{url}\", proxy: \"socks5h://{}\").status\n", proxy.address));
    assert_eq!(out, "200\n");
    assert_eq!(proxy.seen(), [format!("socks5 grenat.test:{port}")]);
    // with socks5, the host is resolved here, where it does not exist
    let e = run_err(&format!("Http.get(\"{url}\", proxy: \"socks5://{}\")\n", proxy.address), Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert_eq!(proxy.seen().len(), 1);
}

#[test]
fn credentials_go_to_the_proxy() {
    let (base, proxy) = (server(), Proxy::start(Some(("ada", "p@ss"))));
    let out = run(&format!("p Http.get(\"{base}/repo\", proxy: \"socks5://ada:p@ss@{}\").status\n", proxy.address));
    assert_eq!(out, "200\n");
    // a secret in the proxy URL, revealed to the proxy only
    let src = format!(
        "mock_credentials({{\"proxy\" => {{\"password\" => \"p@ss\"}}}})\npw = Credentials.fetch(:proxy, :password)\np Http.get(\"{base}/repo\", proxy: \"socks5://ada:#{{pw}}@{}\").status\n",
        proxy.address
    );
    assert_eq!(run(&src), "200\n");
    let seen = proxy.seen();
    assert_eq!((seen[0].as_str(), seen[2].as_str()), ("auth ada:p@ss", "auth ada:p@ss"));
}

#[test]
fn a_wrong_password_is_an_http_error_that_keeps_the_secret() {
    let (base, proxy) = (server(), Proxy::start(Some(("ada", "right"))));
    let e =
        run_err(&format!("Http.get(\"{base}/repo\", proxy: \"socks5://ada:wrong@{}\")\n", proxy.address), Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert!(e.message.starts_with(&format!("GET {base}/repo: ")), "{}", e.message);
    let url = format!("socks5://ada:s3cr3t@{}", proxy.address);
    let e = run_err(&with_secret_proxy(&url, &format!("Http.get(\"{base}/repo\", proxy: proxy)\n")), Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert!(!e.message.contains("s3cr3t"), "{}", e.message);
    assert_eq!(proxy.seen(), ["auth ada:wrong", "auth ada:s3cr3t"]);
}

#[test]
fn an_unreachable_proxy_is_an_http_error() {
    let base = server();
    let e = run_err(&format!("Http.get(\"{base}/repo\", proxy: \"socks5://127.0.0.1:1\")\n"), Vec::new());
    assert_eq!(e.ty, "HttpError");
    let src =
        with_secret_proxy("socks5://ada:s3cr3t@127.0.0.1:1", &format!("Http.get(\"{base}/repo\", proxy: proxy)\n"));
    let e = run_err(&src, Vec::new());
    assert_eq!(e.ty, "HttpError");
    assert!(!e.message.contains("s3cr3t"), "{}", e.message);
}

#[test]
fn an_invalid_proxy_is_an_argument_error() {
    let e = run_err("Http.get(\"http://x.io\", proxy: \"gopher://h:70\")\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
    assert_eq!(
        e.message,
        "invalid `Http` option `proxy: \"gopher://h:70\"`: a proxy URL starts with http://, https://, socks4://, socks4a://, socks5://, socks5h://"
    );
    let e = run_err("Http.get(\"http://x.io\", proxy: \"socks5://h:port\")\n", Vec::new());
    assert_eq!(
        e.message,
        "invalid `Http` option `proxy: \"socks5://h:port\"`: a proxy URL is `scheme://[user:password@]host[:port]`"
    );
    let e = run_err("Http.get(\"http://x.io\", proxy: true)\n", Vec::new());
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("ArgumentError", "invalid `Http` option `proxy: true`"));
    // a secret is not named
    let e =
        run_err(&with_secret_proxy("bogus://ada:s3cr3t@h", "Http.get(\"http://x.io\", proxy: proxy)\n"), Vec::new());
    assert_eq!(e.ty, "ArgumentError");
    assert!(!e.message.contains("s3cr3t") && e.message.contains("[secret]"), "{}", e.message);
}

#[test]
fn socks4_and_http_proxies() {
    let (base, proxy) = (server(), Proxy::start(None));
    let port = port_of(&base);
    let out = run(&format!(
        "p Http.get(\"{base}/repo\", proxy: \"socks4://{0}\").status\np Http.get(\"http://grenat.test:{port}/repo\", proxy: \"socks4a://{0}\").status\np Http.get(\"{base}/echo\", proxy: \"http://ada:pw@{0}\").json.trust![\"method\"]\n",
        proxy.address
    ));
    assert_eq!(out, "200\n200\n\"GET\"\n");
    let seen = proxy.seen();
    assert_eq!(
        seen[..3],
        [format!("socks4 127.0.0.1:{port}"), format!("socks4 grenat.test:{port}"), format!("connect 127.0.0.1:{port}")]
    );
    // `ada:pw` in base64
    assert_eq!(seen[3].to_ascii_lowercase(), "proxy-authorization: basic ywrhonb3");
}

#[test]
fn proxy_false_goes_direct_and_nil_is_the_environment() {
    let base = server();
    let out = run(&format!(
        "p Http.get(\"{base}/repo\", proxy: false).status\np Http.head(\"{base}/repo\", proxy: nil).status\n"
    ));
    assert_eq!(out, "200\n200\n");
}

#[test]
fn the_log_names_the_proxy_without_its_credentials() {
    let (base, proxy) = (server(), Proxy::start(Some(("ada", "s3cr3t"))));
    let out = logged(&format!("Http.get(\"{base}/repo\", proxy: \"socks5://ada:s3cr3t@{}\")\n", proxy.address));
    assert!(out.contains(&format!("[http] GET {base}/repo via socks5://{} → 200", proxy.address)), "{out}");
    assert!(!out.contains("s3cr3t"), "{out}");
    let url = format!("socks5://ada:s3cr3t@{}", proxy.address);
    let out = logged(&with_secret_proxy(&url, &format!("Http.get(\"{base}/repo\", proxy: proxy)\n")));
    assert!(out.contains(&format!("[http] GET {base}/repo via [secret] → 200")), "{out}");
    assert!(!out.contains("s3cr3t") && !out.contains(&proxy.address), "{out}");
}

#[test]
fn stubs_answer_whatever_the_proxy() {
    let results = tests_output(
        "\
test \"stubbed behind a proxy\" do
  mock_http \"GET https://api.github.com/x\", json: {ok: true}
  assert_equal 200, Http.get(\"https://api.github.com/x\", proxy: \"socks5://127.0.0.1:1\").status
end
",
    );
    assert_eq!(results[0].1, None);
}
