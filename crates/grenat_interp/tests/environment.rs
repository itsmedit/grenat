//! `Env`: the process's environment in a program; in tests, only what
//! `mock_env` gives — merged, forgotten after the test, never the real one.

mod common;

use common::*;

#[test]
fn a_program_reads_the_process_environment() {
    let out = run("\
p Env.get(\"GRENAT_TEST_ENVIRONMENT_ABSENT\")
p Env.key?(\"GRENAT_TEST_ENVIRONMENT_ABSENT\")
p Env.key?(\"PATH\")
p Env.get(\"PATH\") == Env.fetch(\"PATH\")
p Env.fetch(\"GRENAT_TEST_ENVIRONMENT_ABSENT\", \"default\")
");
    assert_eq!(out, "nil\nfalse\ntrue\ntrue\n\"default\"\n");
    let e = run_err("Env.fetch(\"GRENAT_TEST_ENVIRONMENT_ABSENT\")\n", Vec::new());
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("KeyError", "missing environment variable `GRENAT_TEST_ENVIRONMENT_ABSENT`")
    );
}

#[test]
fn a_test_reads_only_what_it_mocks() {
    tests_pass(
        "\
def token -> String uses env = Env.fetch(\"GITHUB_TOKEN\")

test \"mocked values, merged\" do
  mock_env({\"GITHUB_TOKEN\" => \"t\"})
  assert_equal \"t\", token
  mock_env({\"REGION\" => \"eu\", \"GITHUB_TOKEN\" => \"t2\"})
  assert_equal \"t2\", token
  assert_equal \"eu\", Env.get(\"REGION\")
  assert Env.key?(\"REGION\")
end

test \"the real environment does not leak in\" do
  assert_equal nil, Env.get(\"PATH\")
  assert !Env.key?(\"PATH\")
  assert_equal \"none\", Env.fetch(\"PATH\", \"none\")
  e = assert_raises KeyError do
    Env.fetch(\"PATH\")
  end
  assert_equal \"missing environment variable `PATH` (tests see only what `mock_env` gives)\", e.message
end

test \"each test starts with none\" do
  assert_equal nil, Env.get(\"GITHUB_TOKEN\")
end
",
    );
}

#[test]
fn a_test_file_loads_without_the_environment() {
    let output = tests_pass(
        "\
REGION = Env.fetch(\"PATH\", \"none\")

test \"loaded\" do
  puts REGION
end
",
    );
    assert_eq!(output, "none\n");
}

#[test]
fn mock_env_takes_strings_in_a_test() {
    let (results, _) = test_outcomes(
        "\
test \"a number\" do
  mock_env({\"PORT\" => 8080})
end

test \"no hash\" do
  mock_env(\"PORT=8080\")
end

test \"a secret\" do
  mock_env({\"TOKEN\" => Credentials.fetch(:api, :token)})
end
",
    );
    let errors: Vec<&str> = results.iter().map(|(_, e)| e.as_deref().unwrap()).collect();
    assert!(errors[0].starts_with("TypeError: `mock_env` takes a hash of strings"), "{}", errors[0]);
    assert!(errors[0].ends_with("got \"PORT\" => 8080"), "{}", errors[0]);
    assert!(errors[1].starts_with("TypeError: `mock_env` takes a hash of strings"), "{}", errors[1]);
    assert_eq!(errors[2], "SecretError: `Env` gives plain strings: a secret is given with `mock_credentials`");
    let e = run_err("mock_env({\"A\" => \"b\"})\n", Vec::new());
    assert_eq!(e.ty, "RuntimeError");
    assert!(e.message.contains("`mock_env` only works in a test"), "{}", e.message);
}
