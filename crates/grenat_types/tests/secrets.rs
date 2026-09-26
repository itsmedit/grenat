//! Secrets (E0414): a `Secret` is not a `String`, and never reaches a model.

mod common;

use common::*;

const MODEL: &str = "model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"\n";

#[test]
fn secrets_serve_in_urls_headers_and_connections() {
    clean(
        "def main uses net, env
  token = Credentials.fetch(:github, :token)
  Http.get(\"https://api.github.com/user\", headers: {\"Authorization\" => \"Bearer #{token}\"})
  Http.get(\"https://example.com/?key=\" + Credentials.fetch(:x, :key))
  maybe = Credentials.dig(:github, :other)
  puts token
end
def headers(token: Secret) -> Hash(String, Secret)
  {\"Authorization\" => \"Bearer #{token}\"}
end
",
    );
}

#[test]
fn a_secret_is_not_a_string() {
    let src = "def shout(text: String) = text.upcase\nshout(Credentials.fetch(:github, :token))\n";
    let d = single(src, "E0414", "Credentials.fetch(:github, :token)");
    assert!(d.message.contains("`shout` expects a `String` for `text`: a secret is not one"), "{}", d.message);
}

#[test]
fn a_secret_never_reaches_a_model() {
    let src = format!("{MODEL}prompt leak(t: String) -> ~String using :fast\n  user \"x\"\nend\nprompt inside(t: Secret) -> ~String using :fast\n  user \"Token: #{{t}}\"\nend\n");
    let d = single(&src, "E0414", "\"Token: #{t}\"");
    assert!(d.message.contains("a secret reaches a model through `user`"), "{}", d.message);
    let agent = format!("{MODEL}agent A\n  model :fast\n  on Go(t: Secret) -> ~String\n    run \"use #{{t}}\"\n  end\nend\n");
    let d = diags(&agent);
    assert!(d.iter().any(|d| d.code == Some("E0414") && d.message.contains("through `run`")), "{}", render(&agent, &d));
}

#[test]
fn a_secret_offers_no_method_but_to_s() {
    single("t = Credentials.fetch(:a, :b)\nt.size\n", "E0414", "size");
    clean("def main uses env\n  t = Credentials.fetch(:a, :b).to_s\n  puts t\nend\n");
}

#[test]
fn a_tool_never_takes_nor_gives_a_secret() {
    let d = diags("tool login(password: Secret) -> String\n  \"ok\"\nend\n");
    assert!(d.iter().any(|d| d.message.contains("a secret never goes to nor comes from a model")), "{}", render("", &d));
}
