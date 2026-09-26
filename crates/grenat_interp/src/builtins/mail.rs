//! `Mail`: sending email.
//!
//! ```ruby
//! mailer = Mail.connect(Env.fetch("SMTP_URL"))   # smtps://user:password@smtp.example.com:465
//! mailer.send(from: "bot@acme.com", to: "team@acme.com", subject: "Hi", body: "…")
//! ```
//!
//! Sending is a `net` effect on the SMTP server's host; nothing untrusted
//! goes out (a message reaches people). Tests never send: messages are kept
//! in `Mail.deliveries`, as hashes.

use crate::mail::Email;
use crate::prelude::*;

use super::*;

/// The record `Mail.connect` returns.
pub(crate) const MAILER: &str = "Mailer";

pub(crate) fn call_mail<'p>(interp: &mut Interp<'p>, name: &str, args: &Args<'p>) -> R<'p> {
    match name {
        "connect" => {
            let url = text_arg(args, 0, name)?;
            let Some(host) = url
                .split("://")
                .nth(1)
                .and_then(|rest| rest.rsplit('@').next())
                .and_then(|a| a.split([':', '/']).next())
            else {
                return raise("ArgumentError", format!("`Mail.connect` expects smtp://… or smtps://…, got {url:?}"));
            };
            let host = host.to_string();
            let mut mailers = interp.mailers.borrow_mut();
            mailers.push((url, host));
            Ok(Value::record(MAILER, vec![("id".into(), Value::Int(mailers.len() as i64 - 1))]))
        }
        "deliveries" => {
            let sent = interp.deliveries.borrow().iter().map(delivery).collect();
            Ok(Value::array(sent))
        }
        _ => raise("NoMethodError", format!("unknown method `Mail.{name}`")),
    }
}

/// `mailer.send(from:, to:, subject:, body:)`.
pub(crate) fn mailer_send<'p>(interp: &mut Interp<'p>, fields: &Fields<'p>, args: &Args<'p>) -> R<'p> {
    let Some(Value::Int(id)) = fields.first().map(|(_, v)| v.clone()) else {
        return raise("MailError", "not a mailer");
    };
    let (url, host) = interp.mailers.borrow()[id as usize].clone();
    if args.named.iter().any(|(_, v)| v.contains_taint()) {
        return raise("TaintError", "an untrusted value reaches `send` (an email, effect `net`) without validation");
    }
    let text = |n: &str| args.named.iter().find(|(k, _)| k == n).map(|(_, v)| v.to_display());
    let to = match args.named.iter().find(|(k, _)| k == "to").map(|(_, v)| v.untainted().clone()) {
        Some(Value::Array(items)) => items.borrow().iter().map(Value::to_display).collect(),
        Some(one) => vec![one.to_display()],
        None => Vec::new(),
    };
    let (Some(from), Some(subject), Some(body)) = (text("from"), text("subject"), text("body")) else {
        return raise("ArgumentError", "`send` expects `from:`, `to:`, `subject:` and `body:`");
    };
    if to.is_empty() {
        return raise("ArgumentError", "`send` expects `to:` (an address, or several)");
    }
    interp.check_net(&host, &format!("smtp://{host}"))?;
    let email = Email { from, to, subject, body };
    if interp.offline {
        interp.deliveries.borrow_mut().push(email);
        return Ok(Value::Nil);
    }
    grenat_green::blocking(|| crate::mail::send(&url, &email)).or_else(|e| raise("MailError", e))?;
    if interp.log {
        interp.write_err(&format!("[mail] {} → {}\n", email.subject, email.to.join(", ")));
    }
    Ok(Value::Nil)
}

fn delivery<'p>(email: &Email) -> Value<'p> {
    let pairs = vec![
        (Value::str("from"), Value::str(&email.from)),
        (Value::str("to"), Value::array(email.to.iter().map(Value::str).collect())),
        (Value::str("subject"), Value::str(&email.subject)),
        (Value::str("body"), Value::str(&email.body)),
    ];
    Value::Hash(Arc::new(Mutex::new(pairs)))
}
