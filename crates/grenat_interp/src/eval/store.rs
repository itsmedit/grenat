//! The application's operations store (`grenat_ops`: jobs, approvals, model
//! calls, events, eval runs), in its database.

use crate::builtins::db_error;
use crate::prelude::*;

pub(crate) fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

impl<'p> Interp<'p> {
    /// Runs `f` on the application's database, off the worker threads;
    /// `what` needs it ("jobs", "approvals").
    pub(crate) fn store<T: Send>(
        &mut self,
        what: &str,
        f: impl FnOnce(&mut dyn grenat_db::Connection) -> Result<T, String> + Send,
    ) -> Result<T, Ctrl<'p>> {
        if self.app_db.borrow().is_none() && self.offline {
            // a test that uses jobs without migrations still needs a database
            self.fresh_test_database()?;
        }
        let Some(database) = self.app_db.borrow().clone() else {
            return raise("DbError", format!("{what} need a database: declare one (`database Env.fetch(\"DATABASE_URL\")`)"));
        };
        let connection = self.connection_of(&database).expect("a database");
        grenat_green::blocking(|| f(&mut **connection.lock())).or_else(db_error)
    }
}
