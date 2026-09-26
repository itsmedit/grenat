//! `Shell`: programs run as argument vectors, with limits; `tool_timeout`.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use grenat_interp::{Options, Output, Response, run_tests};
use serde_json::json;

#[test]
fn programs_run_as_argument_vectors() {
    let out = run("\
r = Shell.run([\"echo\", \"$HOME; rm -rf nothing\"])
p r.status, r.ok?, r.stdout
f = Shell.run([\"sh\", \"-c\", \"echo oops >&2; exit 3\"])
p f.status, f.ok?, f.stderr
");
    // no shell interprets the arguments; the outputs are untrusted
    assert_eq!(out, "0\ntrue\n~\"$HOME; rm -rf nothing\\n\"\n3\nfalse\n~\"oops\\n\"\n");
}

#[test]
fn a_clean_environment_a_directory_and_a_timeout() {
    let dir = temp_dir("shell-cwd");
    std::fs::write(dir.join("marker.txt"), "").unwrap();
    let out = run(&format!(
        "\
p Shell.run([\"sh\", \"-c\", \"echo x${{CARGO_PKG_NAME}}x $GIVEN\"], env: {{\"GIVEN\" => \"yes\"}}).stdout.trust!
p Shell.run([\"ls\"], cwd: \"{}\").stdout.trust!
",
        dir.display()
    ));
    // the parent's environment is not passed on, only what is given
    assert_eq!(out, "\"xx yes\\n\"\n\"marker.txt\\n\"\n");
    let e = run_err("Shell.run([\"sleep\", \"5\"], timeout: 0.2)\n", Vec::new());
    assert_eq!((e.ty.as_str(), e.message.as_str()), ("ShellError", "`sleep` still running after 200ms: killed"));
    let e = run_err("Shell.run([\"no-such-program-x\"])\n", Vec::new());
    assert!(e.message.starts_with("cannot run `no-such-program-x`"), "{}", e.message);
    let e = run_err("Shell.run(\"ls -la\")\n", Vec::new());
    assert_eq!(e.ty, "ArgumentError");
}

#[test]
fn programs_are_capabilities() {
    let src = "\
def status uses shell(\"git\")
  Shell.run([\"git\", \"--version\"]).ok?
end
def sneaky uses shell(\"git\")
  Shell.run([\"/bin/echo\", \"hi\"])
end
p status
sneaky
";
    let r = run_with(src, Vec::new(), &[]);
    assert_eq!(r.output, "true\n");
    let e = r.err();
    assert_eq!(
        (e.ty.as_str(), e.message.as_str()),
        ("CapabilityError", "running `echo` is not allowed by `sneaky` (uses shell(\"git\"))")
    );
}

#[test]
fn nothing_untrusted_goes_in() {
    let e = run_err(&format!("{SUMMARY}s = summarize(\"x\")\nShell.run([\"echo\", s.title])\n"), vec![summary_reply()]);
    assert_eq!(e.ty, "TaintError");
    let e = run_err("out = Shell.run([\"echo\", \"a\"]).stdout\nShell.run([\"echo\", out])\n", Vec::new());
    assert_eq!(e.ty, "TaintError");
    assert_eq!(
        run("out = Shell.run([\"echo\", \"a\"]).stdout.check { |o| o.size < 5 }?\np Shell.run([\"echo\", out]).ok?\n"),
        "true\n"
    );
}

#[test]
fn the_network_can_be_cut_off() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::Write;
        for mut stream in listener.incoming().flatten() {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    let curl = |network: bool| {
        run_with(
            &format!("p Shell.run([\"curl\", \"-s\", \"-m\", \"3\", \"{url}\"], network: {network}).stdout.trust!\n"),
            Vec::new(),
            &[],
        )
    };
    assert_eq!(curl(true).ok(), "\"ok\"\n");
    match curl(false).result {
        // isolated: curl cannot connect, and prints nothing
        Ok(_) => assert_eq!(curl(false).output, "\"\"\n"),
        Err(e) => assert!(e.message.starts_with("network isolation is not available here"), "{}", e.message),
    }
}

#[test]
fn tests_never_run_programs() {
    let src = "\
test \"stubbed\" do
  mock_shell \"kubectl rollout restart*\", stdout: \"restarted\"
  r = Shell.run([\"kubectl\", \"rollout\", \"restart\", \"deploy/api\"])
  assert_equal \"restarted\", r.stdout.trust!
  mock_shell \"false\", status: 1
  assert !Shell.run([\"false\"]).ok?
end
test \"not stubbed\" do
  Shell.run([\"echo\", \"hi\"])
end
";
    let parsed = grenat_parser::parse(src);
    let options = Options {
        output: Output::Capture(Arc::new(Mutex::new(String::new()))),
        journal: Some(temp_dir("journal")),
        ..Options::default()
    };
    let outcomes = run_tests(&parsed.program, options).unwrap();
    assert!(outcomes[0].error.is_none(), "{:?}", outcomes[0].error);
    let e = outcomes[1].error.as_ref().unwrap();
    assert_eq!(e.message, "no programs in tests: `echo hi` is not stubbed with `mock_shell`");
}

#[test]
fn a_tool_that_runs_too_long_is_stopped_and_reported_to_the_model() {
    let src = "\
model :fast, provider: :anthropic, name: \"claude-haiku-4-5\"
## Waits.
tool wait(seconds: Int) -> String
  sleep seconds
  \"done\"
end
agent Waiter
  model :fast
  tools wait
  tool_timeout 0.2
  on Go -> String
    run \"go\"
  end
end
puts spawn(Waiter).ask(Go())
";
    let replies = vec![
        Response::tool_call("t1", "wait", json!({"seconds": 5})),
        Response::tool_call("t2", "final_answer", json!({"value": "gave up"})),
    ];
    let started = std::time::Instant::now();
    let r = run_with(src, replies, &[]);
    let requests = r.requests.clone();
    assert_eq!(r.ok(), "gave up\n");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    let result = &requests[1]["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
    assert_eq!(result["content"], "TimeoutError: tool `wait` took more than 0.2s");
}
