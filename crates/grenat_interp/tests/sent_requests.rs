//! `Http.requests`: what a test's program sent, answered by `mock_http` or
//! not, in order — secrets kept secret, each test starting with none, and
//! nothing outside a test.

mod common;

use common::*;

const TRACKER: &str = "\
def open_issue(title: String, token: String) -> Int uses net(\"tracker.acme.io\")
  Http.post(
    \"https://tracker.acme.io/issues\",
    json: {title:, labels: [\"bug\"]},
    headers: {\"Authorization\" => \"Bearer #{token}\"},
  ).status
end
";

#[test]
fn a_test_reads_what_a_service_received() {
    tests_pass(&format!(
        "{TRACKER}\
test \"the tracker receives the issue\" do
  mock_http \"POST https://tracker.acme.io/issues\", status: 201
  assert_equal 201, open_issue(\"Login fails\", \"t\")
  req = Http.requests.last
  assert_equal \"POST\", req[\"method\"]
  assert_equal \"https://tracker.acme.io/issues\", req[\"url\"]
  assert_equal({{\"title\" => \"Login fails\", \"labels\" => [\"bug\"]}}, req[\"json\"])
  assert_equal \"{{\\\"labels\\\":[\\\"bug\\\"],\\\"title\\\":\\\"Login fails\\\"}}\", req[\"body\"]
  assert_equal \"Bearer t\", req[\"headers\"][\"Authorization\"]
  assert_equal \"application/json\", req[\"headers\"][\"Content-Type\"]
end
"
    ));
}

#[test]
fn every_request_in_order_stubbed_or_not() {
    tests_pass(
        "\
def sync uses net(\"api.acme.io\")
  Http.get(\"https://api.acme.io/items\", query: {page: 2, q: \"a b\"})
  Http.delete(\"https://api.acme.io/items/1\")
end

test \"in order, the unanswered one too\" do
  mock_http \"GET https://api.acme.io/items*\", json: []
  assert_raises HttpError do
    sync
  end
  sent = Http.requests
  assert_equal [\"GET\", \"DELETE\"], sent.map { |r| r[\"method\"] }
  assert_equal \"https://api.acme.io/items?page=2&q=a%20b\", sent[0][\"url\"]
  assert_equal nil, sent[0][\"body\"]
  assert_equal nil, sent[0][\"json\"]
  assert_equal({}, sent[1][\"headers\"])
end

test \"each test starts with none\" do
  assert_equal [], Http.requests
end
",
    );
}

#[test]
fn a_body_is_parsed_only_when_it_is_json() {
    tests_pass(
        "\
def post(body: String) uses net(\"api.acme.io\")
  Http.post(\"https://api.acme.io/raw\", body:)
end

test \"a text body, then a JSON one\" do
  mock_http \"https://api.acme.io/*\", status: 204
  post(\"plain text\")
  post(Json.generate({n: 1}))
  first = Http.requests.first
  assert_equal \"plain text\", first[\"body\"]
  assert_equal nil, first[\"json\"]
  assert_equal({\"n\" => 1}, Http.requests.last[\"json\"])
end
",
    );
}

#[test]
fn secrets_stay_secrets() {
    let output = tests_pass(
        "\
def report(id: Int) uses net(\"tracker.acme.io\"), env
  token = Credentials.fetch(:tracker, :token)
  Http.post(
    \"https://tracker.acme.io/issues/#{id}?key=#{token}\",
    json: {id:, token:},
    headers: {\"Authorization\" => \"Bearer #{token}\", \"Accept\" => \"text/plain\"},
  )
end

test \"a secret sent is a secret recorded\" do
  mock_credentials({\"tracker\" => {\"token\" => \"s3cr3t\"}})
  mock_http \"POST https://tracker.acme.io/*\", status: 201
  report(4)
  req = Http.requests.first
  assert_equal \"Bearer s3cr3t\", req[\"headers\"][\"Authorization\"]
  assert_equal \"https://tracker.acme.io/issues/4?key=s3cr3t\", req[\"url\"]
  assert_equal \"s3cr3t\", req[\"json\"][\"token\"]
  assert_equal 4, req[\"json\"][\"id\"]
  p req
  puts req[\"body\"]
end
",
    );
    assert!(!output.contains("s3cr3t"), "{output}");
    assert!(output.contains("\"Authorization\" => [secret]"), "{output}");
    assert!(output.contains("\"Accept\" => \"text/plain\""), "{output}");
    assert!(output.ends_with("[secret]\n"), "{output}");
}

#[test]
fn a_secret_body_parses_into_secrets() {
    let output = tests_pass(
        "\
def send_key uses net(\"vault.acme.io\"), env
  Http.put(\"https://vault.acme.io/k\", body: \"{\\\"key\\\": \\\"#{Credentials.fetch(:vault, :key)}\\\"}\")
end

test \"a body holding a secret\" do
  mock_credentials({\"vault\" => {\"key\" => \"k-42\"}})
  mock_http \"PUT https://vault.acme.io/k\", status: 200
  send_key
  json = Http.requests.first[\"json\"]
  assert_equal \"k-42\", json[\"key\"]
  p json
end
",
    );
    assert_eq!(output, "{\"key\" => [secret]}\n");
}

#[test]
fn audio_downloads_are_requests_too() {
    tests_pass(
        "\
def fetch(url: String) uses net
  Audio.url(url)
end

test \"a download\" do
  mock_http \"GET https://cdn.acme.io/call.mp3\", body: \"ID3\"
  fetch(\"https://cdn.acme.io/call.mp3\")
  assert_equal \"https://cdn.acme.io/call.mp3\", Http.requests.first[\"url\"]
end
",
    );
}

#[test]
fn only_a_test_reads_them() {
    let e = run_err("p Http.requests\n", Vec::new());
    assert_eq!(e.ty, "RuntimeError");
    assert!(e.message.contains("`Http.requests` only works in a test"), "{}", e.message);
    // nor while a test file loads
    let parsed = grenat_parser::parse("Http.requests\ntest \"t\" do\nend\n");
    let options = grenat_interp::Options { journal: Some(temp_dir("journal")), ..Default::default() };
    assert_eq!(grenat_interp::run_tests(&parsed.program, options).unwrap_err().ty, "RuntimeError");
}
