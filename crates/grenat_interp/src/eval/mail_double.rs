//! A mail server that fails, in tests: after `mock_mail(raise: "SMTP down")`,
//! every email the test sends raises `MailError` ("SMTP down") instead of
//! joining `Mail.deliveries` — a crash between two workflow steps, without
//! touching the credentials. `mock_mail(raise: nil)` brings the server back;
//! each test starts with a working one.

use crate::prelude::*;

impl<'p> Interp<'p> {
    /// `mock_mail(raise: "SMTP down")`.
    pub(crate) fn mock_mail(&mut self, args: &Args<'p>) -> R<'p> {
        const USAGE: &str = "mock_mail(raise: \"SMTP down\")";
        self.only_in_tests("mock_mail", USAGE)?;
        let failure = match (args.pos.as_slice(), args.named.as_slice()) {
            ([], [(option, value)]) if option == "raise" => match value.untainted() {
                Value::Str(reason) => Some(reason.to_string()),
                Value::Nil => None,
                other => {
                    return raise(
                        "TypeError",
                        format!("`mock_mail` expects `raise:` a message, got {}", other.inspect()),
                    );
                }
            },
            _ => return raise("ArgumentError", format!("`mock_mail` expects why sending fails: `{USAGE}`")),
        };
        *self.mail_failure.borrow_mut() = failure;
        Ok(Value::Nil)
    }

    /// Fails as `mock_mail` said, if it did.
    pub(crate) fn mail_double(&self) -> Result<(), Ctrl<'p>> {
        match self.mail_failure.borrow().clone() {
            Some(reason) => raise("MailError", reason),
            None => Ok(()),
        }
    }
}
