//! The mail effect, checked: reaching an SMTP server is `net` on its host —
//! that of a literal URL (`net("smtp.acme.io")`), or any host when the URL
//! comes from configuration (then checked at run time).

mod common;

use common::*;

const SEND: &str = ".send(from: \"bot@acme.io\", to: \"team@acme.io\", subject: \"Hi\", body: \"…\")";

#[test]
fn a_mailer_needs_net() {
    for uses in ["uses env", "uses env, fs.read"] {
        let src = format!("def notify {uses}\n  Mail.connect(Env.fetch(\"SMTP_URL\")){SEND}\nend\n");
        let d = single(&src, "E0300", "Mail.connect(Env.fetch(\"SMTP_URL\"))");
        assert!(d.message.contains("uses effect `net` without declaring it"), "{}", d.message);
    }
    // a mailer passed along: sending needs it too
    let src = format!("def notify(mailer: Mailer) uses env\n  mailer{SEND}\nend\n");
    single(&src, "E0300", &format!("mailer{SEND}"));
    // making one does, even without sending
    single(
        "def mailer uses env = Mail.connect(Env.fetch(\"SMTP_URL\"))\n",
        "E0300",
        "Mail.connect(Env.fetch(\"SMTP_URL\"))",
    );
}

#[test]
fn a_literal_url_names_the_host_to_grant() {
    let src = format!(
        "def notify uses net(\"api.github.com\")\n  Mail.connect(\"smtps://bot:pw@smtp.acme.io:465\"){SEND}\nend\n"
    );
    let d = single(&src, "E0300", "Mail.connect(\"smtps://bot:pw@smtp.acme.io:465\")");
    assert!(d.message.contains("`net(\"smtp.acme.io\")`"), "{}", d.message);
    assert_eq!(d.help.as_deref(), Some("add `net(\"smtp.acme.io\")` to `uses`"));
    for uses in ["net(\"smtp.acme.io\")", "net", "net(\"api.github.com\"), net(\"smtp.acme.io\")"] {
        clean(&format!("def notify uses {uses}\n  Mail.connect(\"smtp://smtp.acme.io\"){SEND}\nend\n"));
    }
}

#[test]
fn a_configured_url_is_checked_at_run_time() {
    // any `net` grant passes statically: the runtime holds the host to it
    for uses in ["net", "net(\"smtp.acme.io\")"] {
        clean(&format!(
            "def notify uses {uses}, env\n  Mail.connect(Env.fetch(\"SMTP_URL\")){SEND}\n  Mail.connect(Credentials.fetch(:smtp, :url)){SEND}\nend\n"
        ));
    }
}

#[test]
fn main_and_callers_cover_it() {
    let src = format!(
        "def notify uses net(\"smtp.acme.io\")\n  Mail.connect(\"smtp://smtp.acme.io\"){SEND}\nend\ndef main uses net(\"api.github.com\")\n  notify\nend\n"
    );
    let d = single(&src, "E0300", "notify");
    assert!(d.message.contains("`net(\"smtp.acme.io\")`"), "{}", d.message);
}

#[test]
fn in_a_workflow_sending_is_a_step_and_making_a_mailer_is_not() {
    clean(&format!(
        "workflow weekly(week: String) uses net(\"smtp.acme.io\")\n  mailer = Mail.connect(\"smtp://smtp.acme.io\")\n  step(:send) {{ mailer{SEND} }}\nend\n"
    ));
    let src = format!(
        "workflow weekly(week: String) uses net(\"smtp.acme.io\")\n  mailer = Mail.connect(\"smtp://smtp.acme.io\")\n  mailer{SEND}\nend\n"
    );
    single(&src, "E0310", &format!("mailer{SEND}"));
}

#[test]
fn an_untrusted_server_is_refused() {
    let src = format!(
        "{PRELUDE}def main uses llm, net\n  s = summarize(\"x\")\n  Mail.connect(\"smtp://#{{s.title}}\")\nend\n"
    );
    single(&src, "E0412", "\"smtp://#{s.title}\"");
}
