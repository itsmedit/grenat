//! Web applications: routes, parameters, responses, and pages that never
//! carry an unescaped untrusted value.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, run_tests};

fn results(src: &str) -> Vec<(String, Option<String>)> {
    let parsed = grenat_parser::parse(src);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    run_tests(&parsed.program, options)
        .unwrap()
        .into_iter()
        .map(|o| (o.name, o.error.map(|e| format!("{}: {}", e.ty, e.message))))
        .collect()
}

fn all_pass(src: &str) {
    for (name, error) in results(src) {
        assert!(error.is_none(), "`{name}`: {}", error.unwrap());
    }
}

const APP: &str = "\
struct Ticket
  id: Int
  subject: String
end

get \"/tickets/:id\" do |req|
  id = req.params[\"id\"].check { |i| i.to_i > 0 }?.to_i
  json(Ticket(id:, subject: \"Ticket #{id}\"))
end

post \"/tickets\" do |req|
  subject = req.json.trust![\"subject\"]
  status 201, json(Ticket(id: 7, subject:))
end

get \"/search\" do |req|
  \"q=#{req.params[\"q\"].trust!} page=#{req.params[\"page\"].trust!}\"
end

get \"/hello\" do |req|
  html \"<h1>Hello #{Html.escape(req.params[\"name\"])}</h1>\"
end

get \"/old\" do |req|
  redirect \"/tickets/1\"
end

delete \"/tickets/:id\" do |req|
  nil
end
";

#[test]
fn routes_parameters_and_responses() {
    all_pass(&format!(
        "{APP}
test \"a route with a parameter\" do
  r = request :get, \"/tickets/42\"
  assert_equal 200, r[\"status\"]
  assert_equal \"application/json\", r[\"content_type\"]
  assert_equal \"{{\\\"id\\\":42,\\\"subject\\\":\\\"Ticket 42\\\"}}\", r[\"body\"]
end
test \"a created resource\" do
  r = request :post, \"/tickets\", json: {{subject: \"Bug\"}}
  assert_equal 201, r[\"status\"]
  assert r[\"body\"].include?(\"Bug\")
end
test \"query parameters, decoded\" do
  assert_equal \"q=rust lang page=2\", (request :get, \"/search?q=rust+lang&page=2\")[\"body\"]
end
test \"a page, escaped\" do
  r = request :get, \"/hello?name=%3Cscript%3Ealert(1)%3C%2Fscript%3E\"
  assert_equal \"<h1>Hello &lt;script&gt;alert(1)&lt;/script&gt;</h1>\", r[\"body\"]
  assert_equal \"text/html; charset=utf-8\", r[\"content_type\"]
end
test \"redirects, no content, errors\" do
  r = request :get, \"/old\"
  assert_equal 302, r[\"status\"]
  assert_equal \"/tickets/1\", r[\"headers\"][\"Location\"]
  assert_equal 204, (request :delete, \"/tickets/3\")[\"status\"]
  assert_equal 404, (request :get, \"/nowhere\")[\"status\"]
  assert_equal 405, (request :put, \"/tickets/3\")[\"status\"]
end
"
    ));
}

#[test]
fn pages_never_carry_an_unescaped_untrusted_value() {
    let results = results(
        "\
get \"/raw\" do |req|
  html \"<p>#{req.params[\"name\"]}</p>\"
end
get \"/go\" do |req|
  redirect req.params[\"to\"]
end
test \"raw\" do
  request :get, \"/raw?name=x\"
end
test \"open redirect\" do
  request :get, \"/go?to=https://evil.io\"
end
",
    );
    assert!(
        results[0]
            .1
            .as_deref()
            .unwrap()
            .starts_with("TaintError: an untrusted value reaches `html` (a page) without escaping"),
        "{results:?}"
    );
    assert!(
        results[1].1.as_deref().unwrap().starts_with("TaintError: an untrusted value is the URL of `redirect`"),
        "{results:?}"
    );
}
