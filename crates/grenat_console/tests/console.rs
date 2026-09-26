//! The console through requests, as a browser sends them: its pages, its
//! actions, and who may use them.

use grenat_console::{Access, Agent, Application, Console, Exposure, Mcp, McpServer, Request, Response};
use grenat_db::Connection;
use grenat_ops::{approvals, calls, evals, events, jobs, journal};
use serde_json::json;

const HOST: &str = "127.0.0.1:4000";
const NOW: f64 = 1_790_000_000.0;

struct FakeMcp;

impl Mcp for FakeMcp {
    fn tools(&mut self, server: &str) -> Result<Vec<grenat_mcp::Tool>, String> {
        match server {
            "linear" => Ok(vec![grenat_mcp::Tool {
                name: "create_issue".into(),
                description: "Creates an issue.".into(),
                input_schema: json!({"type": "object"}),
                read_only: false,
                destructive: false,
            }]),
            other => Err(format!("{other} is down")),
        }
    }
}

fn app() -> Application {
    Application {
        name: "desk".into(),
        agents: vec![Agent { name: "Triage".into(), handlers: vec!["Classify".into()] }],
        workflows: vec!["publish".into()],
        tools: vec!["lookup".into()],
        routes: vec!["GET /health".into()],
        mcp_servers: vec![
            McpServer { name: "linear".into(), target: "https://mcp.linear.app/mcp".into() },
            McpServer { name: "files".into(), target: "npx server-filesystem".into() },
        ],
        exposures: vec![Exposure { path: "/mcp".into(), tools: vec!["lookup".into()], public: false }],
    }
}

struct Setup {
    console: Console,
    db: Box<dyn Connection>,
    journals: std::path::PathBuf,
}

fn setup(name: &str, access: Access) -> Setup {
    let journals = std::env::temp_dir().join(format!("grenat-console-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&journals);
    let db = grenat_db::connect("sqlite::memory:").unwrap();
    Setup { console: Console::new(app(), journals.clone(), access), db, journals }
}

fn local(name: &str) -> Setup {
    setup(name, Access::for_address(HOST, None).unwrap())
}

impl Setup {
    fn send(&mut self, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> Response {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mut all = vec![("Host".to_string(), HOST.to_string())];
        all.extend(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let request = Request {
            method: method.into(),
            path: path.into(),
            query: query.into(),
            headers: all,
            body: body.as_bytes().to_vec(),
        };
        self.console.handle(&request, self.db.as_mut(), &mut FakeMcp, NOW)
    }

    fn get(&mut self, target: &str) -> Response {
        self.send("GET", target, &[], "")
    }

    /// The CSRF secret, from a form of a page.
    fn csrf(&mut self, page: &str) -> String {
        let body = self.get(page).body;
        let at = body.find("name=\"csrf\" value=\"").expect("a form") + "name=\"csrf\" value=\"".len();
        body[at..at + 64].to_string()
    }

    fn post(&mut self, target: &str, csrf: &str) -> Response {
        self.send("POST", target, &[], &format!("csrf={csrf}"))
    }
}

fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// A job waiting for its approval `message`: (job, approval).
fn waiting(db: &mut dyn Connection, message: &str) -> (i64, i64) {
    let job = jobs::enqueue(db, "publish", "[7]", 0.0, NOW - 600.0).unwrap();
    jobs::claim(db, job, NOW - 590.0).unwrap();
    approvals::ask(db, job, 0, message, NOW - 580.0).unwrap();
    jobs::wait(db, job, NOW - 580.0).unwrap();
    let approval = approvals::pending(db).unwrap().last().unwrap().id;
    (job, approval)
}

#[test]
fn every_page_answers_with_the_security_headers() {
    let mut s = local("pages");
    for page in [
        "/",
        "/approvals",
        "/jobs",
        "/jobs?status=failed",
        "/journals",
        "/costs",
        "/costs?days=30",
        "/evals",
        "/events",
        "/events?only=refusals",
        "/mcp",
    ] {
        let response = s.get(page);
        assert_eq!(response.status, 200, "{page}: {}", response.body);
        assert!(response.body.contains("<title>"), "{page}");
        assert!(header(&response, "Content-Security-Policy").unwrap().contains("default-src 'none'"));
        assert_eq!(header(&response, "X-Frame-Options"), Some("DENY"));
        // with `no-referrer`, browsers post the console's forms with `Origin: null`
        assert_eq!(header(&response, "Referrer-Policy"), Some("same-origin"));
        assert!(!response.body.contains("<script"), "{page}: no script");
    }
    assert_eq!(s.get("/nothing").status, 404);
    assert_eq!(s.get("/jobs/abc").status, 404);
    assert_eq!(s.get("/jobs/99").status, 404);
}

#[test]
fn an_approval_is_decided_and_its_job_queued_again() {
    let mut s = local("approve");
    let (job, approval) = waiting(s.db.as_mut(), "Publish <b>“Rust 2”</b>?");
    let page = s.get("/approvals");
    assert!(page.body.contains("Publish &lt;b&gt;“Rust 2”&lt;/b&gt;?"), "escaped: {}", page.body);
    assert!(page.body.contains("<span class=\"count\">1</span>"), "counted in the navigation");
    let csrf = s.csrf("/approvals");
    let done = s.post(&format!("/approvals/{approval}/approve"), &csrf);
    assert_eq!((done.status, header(&done, "Location")), (303, Some("/approvals?done=approved")));
    assert_eq!(jobs::get(s.db.as_mut(), job).unwrap().unwrap().status, jobs::Status::Queued);
    assert!(s.get("/approvals?done=approved").body.contains("the job runs again"));
    // decided once
    let again = s.post(&format!("/approvals/{approval}/deny"), &csrf);
    assert_eq!(header(&again, "Location"), Some("/approvals?done=stale"));
    assert!(s.get(&format!("/jobs/{job}")).body.contains("approved"));
}

#[test]
fn a_form_from_elsewhere_changes_nothing() {
    let mut s = local("csrf");
    let (_, approval) = waiting(s.db.as_mut(), "Publish?");
    let target = format!("/approvals/{approval}/approve");
    assert_eq!(s.post(&target, "").status, 403, "no secret");
    assert_eq!(s.post(&target, &"0".repeat(64)).status, 403, "a wrong secret");
    let csrf = s.csrf("/approvals");
    let foreign = s.send("POST", &target, &[("Origin", "https://evil.example")], &format!("csrf={csrf}"));
    assert_eq!(foreign.status, 403, "another site");
    let opaque = s.send("POST", &target, &[("Origin", "null")], &format!("csrf={csrf}"));
    assert_eq!(opaque.status, 403, "a sandboxed frame");
    assert_eq!(approvals::pending(s.db.as_mut()).unwrap().len(), 1);
    let ours = s.send("POST", &target, &[("Origin", &format!("http://{HOST}"))], &format!("csrf={csrf}"));
    assert_eq!(ours.status, 303);
}

#[test]
fn a_local_console_answers_this_machine_only() {
    let mut s = local("rebinding");
    let request = |host: &str| Request {
        method: "GET".into(),
        path: "/".into(),
        query: String::new(),
        headers: vec![("Host".into(), host.into())],
        body: Vec::new(),
    };
    assert_eq!(s.console.handle(&request("evil.example:4000"), s.db.as_mut(), &mut FakeMcp, NOW).status, 403);
    assert_eq!(s.console.handle(&request("localhost:4000"), s.db.as_mut(), &mut FakeMcp, NOW).status, 200);
    assert_eq!(s.console.handle(&request("127.0.0.1:4001"), s.db.as_mut(), &mut FakeMcp, NOW).status, 403);
    assert!(Access::for_address("0.0.0.0:4000", None).is_err(), "off this machine, a token is required");
    assert!(Access::for_address("0.0.0.0:4000", Some("short".into())).is_err());
    assert!(Access::for_address("0.0.0.0:4000", Some("a-long-enough-token".into())).is_ok());
}

#[test]
fn with_a_token_one_signs_in_then_out() {
    let mut s = setup("token", Access::for_address("0.0.0.0:4000", Some("a-long-enough-token".into())).unwrap());
    let first = s.get("/");
    assert_eq!((first.status, header(&first, "Location")), (303, Some("/login")));
    assert_eq!(s.get("/login").status, 200);
    let wrong = s.send("POST", "/login", &[], "token=guess");
    assert_eq!(wrong.status, 401);
    assert!(header(&wrong, "Set-Cookie").is_none());
    let right = s.send("POST", "/login", &[], "token=a-long-enough-token");
    assert_eq!(right.status, 303);
    let cookie = header(&right, "Set-Cookie").unwrap().to_string();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    let session = cookie.split(';').next().unwrap().to_string();
    let page = s.send("GET", "/", &[("Cookie", &session)], "");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Sign out"));
    let body = page.body;
    let at = body.find("name=\"csrf\" value=\"").unwrap() + "name=\"csrf\" value=\"".len();
    let csrf = &body[at..at + 64];
    let out = s.send("POST", "/logout", &[("Cookie", &session)], &format!("csrf={csrf}"));
    assert_eq!(header(&out, "Location"), Some("/login"));
    assert_eq!(s.send("GET", "/", &[("Cookie", &session)], "").status, 303, "the session is over");
}

#[test]
fn a_failed_job_its_journal_and_its_retry() {
    let mut s = local("jobs");
    let job = jobs::enqueue(s.db.as_mut(), "publish", "[7]", 0.0, NOW - 60.0).unwrap();
    jobs::claim(s.db.as_mut(), job, NOW - 50.0).unwrap();
    jobs::fail(s.db.as_mut(), job, 3, "IoError: <down>", NOW - 40.0).unwrap();
    let call = calls::Call {
        at: NOW - 45.0,
        model: "claude-haiku-4-5".into(),
        job_id: Some(job),
        cost_usd: 0.25,
        ..calls::Call::default()
    };
    calls::record(s.db.as_mut(), &call).unwrap();
    let path = journal::path(&s.journals, "publish", &[json!(7)], &[]);
    journal::append_step(&path, "draft", 0, &json!({"title": "<i>Rust</i>"})).unwrap();
    let list = s.get("/jobs?status=failed");
    assert!(list.body.contains("IoError: &lt;down&gt;"));
    let detail = s.get(&format!("/jobs/{job}"));
    assert!(detail.body.contains("1 step(s)") && detail.body.contains("$0.2500"), "{}", detail.body);
    let run = path.file_stem().unwrap().to_str().unwrap().to_string();
    let journal = s.get(&format!("/journals/{run}"));
    assert!(journal.body.contains(":draft") && journal.body.contains("&lt;i&gt;Rust&lt;/i&gt;"), "{}", journal.body);
    assert!(journal.body.contains("Not completed"));
    assert!(s.get("/journals").body.contains(&run));
    assert_eq!(s.get("/journals/..%2F..%2Fetc%2Fpasswd").status, 404);
    let csrf = s.csrf(&format!("/jobs/{job}"));
    let retried = s.post(&format!("/jobs/{job}/retry"), &csrf);
    assert_eq!(header(&retried, "Location"), Some(format!("/jobs/{job}?done=retried").as_str()));
    assert_eq!(jobs::get(s.db.as_mut(), job).unwrap().unwrap().status, jobs::Status::Queued);
    let stale = s.post(&format!("/jobs/{job}/retry"), &csrf);
    assert_eq!(header(&stale, "Location"), Some(format!("/jobs/{job}?done=stale").as_str()));
}

#[test]
fn costs_evals_refusals_and_mcp() {
    let mut s = local("reports");
    let call = |agent: &str, cost: f64| calls::Call {
        at: NOW - 3600.0,
        model: "claude-opus-5".into(),
        agent: Some(agent.into()),
        input_tokens: 1000,
        output_tokens: 100,
        cost_usd: cost,
        ..calls::Call::default()
    };
    calls::record(s.db.as_mut(), &call("Triage", 0.5)).unwrap();
    calls::record(s.db.as_mut(), &call("Writer", 2.0)).unwrap();
    let old = calls::Call { at: NOW - 10.0 * 86_400.0, cost_usd: 100.0, ..call("Old", 0.0) };
    calls::record(s.db.as_mut(), &old).unwrap();
    let costs = s.get("/costs").body;
    assert!(costs.contains("$2.50") && costs.contains("Writer") && !costs.contains("Old"), "{costs}");
    assert!(s.get("/costs?days=30").body.contains("$102.50"));
    assert!(s.get("/").body.contains("$2.50"));

    let run = |at: f64, score: f64| evals::Run {
        at,
        name: "triage".into(),
        score,
        threshold: 0.8,
        passed: score >= 0.8,
        rows: 10,
        ..evals::Run::default()
    };
    evals::record(s.db.as_mut(), &run(NOW - 7200.0, 0.7)).unwrap();
    evals::record(s.db.as_mut(), &run(NOW - 3600.0, 0.9)).unwrap();
    let page = s.get("/evals").body;
    assert!(page.contains("90%") && page.contains("<polyline") && page.contains("passed"), "{page}");

    events::record(
        s.db.as_mut(),
        NOW - 60.0,
        "request",
        "GET /page",
        "TaintError",
        "an untrusted value reaches `html`",
    )
    .unwrap();
    events::record(s.db.as_mut(), NOW - 30.0, "job", "job 3 (fetch)", "IoError", "down").unwrap();
    let refusals = s.get("/events?only=refusals").body;
    assert!(refusals.contains("GET /page") && !refusals.contains("job 3 (fetch)"));
    assert!(s.get("/events").body.contains("job 3 (fetch)"));

    let mcp = s.get("/mcp").body;
    assert!(mcp.contains("https://mcp.linear.app/mcp") && mcp.contains("/mcp") && mcp.contains("token"));
    assert!(s.get("/mcp/linear").body.contains("create_issue"));
    assert!(s.get("/mcp/files").body.contains("files is down"));
    assert_eq!(s.get("/mcp/unknown").status, 404, "only declared servers are connected to");
}
