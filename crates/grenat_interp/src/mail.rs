//! Email transport: one message, sent over SMTP.

use lettre::message::{Mailbox, header::ContentType};
use lettre::{Message, SmtpTransport, Transport};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Email {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// Sends `email` through the server at `url` (`smtp://…` with STARTTLS, or
/// `smtps://…`; `user:password@` for authentication).
pub(crate) fn send(url: &str, email: &Email) -> Result<(), String> {
    let mailbox = |address: &str| address.parse::<Mailbox>().map_err(|e| format!("invalid address {address:?}: {e}"));
    let mut builder = Message::builder().from(mailbox(&email.from)?).subject(&email.subject);
    for to in &email.to {
        builder = builder.to(mailbox(to)?);
    }
    let message = builder.header(ContentType::TEXT_PLAIN).body(email.body.clone()).map_err(|e| e.to_string())?;
    let transport = SmtpTransport::from_url(url).map_err(|e| format!("invalid SMTP URL: {e}"))?.build();
    transport.send(&message).map(drop).map_err(|e| e.to_string())
}
