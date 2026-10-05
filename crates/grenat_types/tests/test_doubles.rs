//! Test doubles that exist inside a test only, checked: `Http.requests`,
//! `mock_env`, `mock_mail` — typed, and refused anywhere else (E0500).

mod common;

use common::*;

#[test]
fn inside_a_test_they_are_typed() {
    clean(
        "\
def open_issue(title: String) -> Int uses net(\"tracker.acme.io\"), env
  token = Env.fetch(\"TRACKER_TOKEN\")
  Http.post(\"https://tracker.acme.io/issues\", json: {title:}, headers: {\"Authorization\" => \"Bearer #{token}\"}).status
end
def region -> String uses env = Env.key?(\"REGION\") ? Env.fetch(\"REGION\") : \"eu\"
test \"what the tracker received\" do
  mock_env({\"TRACKER_TOKEN\" => \"t\", \"REGION\" => \"us\"})
  mock_mail(raise: \"SMTP down\")
  mock_mail(raise: nil)
  assert_equal 201, open_issue(\"x\")
  req = Http.requests.last
  assert_equal \"POST\", req[\"method\"]
  assert_equal \"Bearer t\", req[\"headers\"][\"Authorization\"]
  assert_equal 1, Http.requests.size
  assert_equal \"us\", region.downcase
end
",
    );
}

#[test]
fn http_requests_is_an_array_of_hashes() {
    let src = "test \"t\" do\n  n = Http.requests.upcase\nend\n";
    let d = single(src, "E0200", "upcase");
    assert!(d.message.contains("unknown method `upcase` for `Array(Hash(String, ?))`"), "{}", d.message);
}

#[test]
fn outside_a_test_they_do_not_exist() {
    for (src, at) in [
        ("p Http.requests\n", "Http.requests"),
        ("def helper\n  mock_env({\"A\" => \"b\"})\nend\n", "mock_env({\"A\" => \"b\"})"),
        ("mock_mail(raise: \"SMTP down\")\n", "mock_mail(raise: \"SMTP down\")"),
    ] {
        let d = single(src, "E0500", at);
        assert!(d.message.contains("can only be used inside a test"), "{}", d.message);
    }
}

#[test]
fn their_arguments_are_checked() {
    for (call, at, says) in [
        (
            "mock_env({\"PORT\" => 8080})",
            "{\"PORT\" => 8080}",
            "`mock_env` expects `Hash(String, String)`, got `Hash(String, Int)`",
        ),
        ("mock_env([\"A\"])", "[\"A\"]", "got `Array(String)`"),
        ("mock_env()", "mock_env()", "`mock_env` expects a hash"),
        (
            "mock_env({\"T\" => Credentials.fetch(:a, :b)})",
            "{\"T\" => Credentials.fetch(:a, :b)}",
            "got `Hash(String, Secret)`",
        ),
        ("mock_mail(raise: 3)", "3", "`mock_mail` expects `raise:` a message (`String`), got `Int`"),
        ("mock_mail(\"x\")", "mock_mail(\"x\")", "`mock_mail` expects why sending fails"),
        ("mock_mail(fail: \"x\")", "mock_mail(fail: \"x\")", "`mock_mail` expects why sending fails"),
    ] {
        let src = format!("test \"t\" do\n  {call}\nend\n");
        let d = single(&src, "E0200", at);
        assert!(d.message.contains(says), "{call}: {}", d.message);
    }
}

#[test]
fn env_key_is_an_env_effect() {
    let d = single("def f -> Bool uses time = Env.key?(\"A\")\n", "E0300", "Env.key?(\"A\")");
    assert!(d.message.contains("uses effect `env(\"A\")`"), "{}", d.message);
}
