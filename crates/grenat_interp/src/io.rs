//! Program I/O: standard (or captured) output, errors, line input.

use std::io::Write as _;

use crate::value::Locked;
use crate::*;

impl<'p> Interp<'p> {
    pub(crate) fn write_out(&self, text: &str) {
        match &self.output {
            Output::Stdout => {
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(text.as_bytes());
                let _ = stdout.flush();
            }
            Output::Capture(buffer) => buffer.borrow_mut().push_str(text),
        }
    }

    pub(crate) fn write_err(&self, text: &str) {
        match &self.output {
            Output::Stdout => eprint!("{text}"),
            Output::Capture(buffer) => buffer.borrow_mut().push_str(text),
        }
    }

    /// A line typed by the human; in tests (offline), only scripted lines:
    /// a test never waits for the keyboard.
    pub(crate) fn read_line(&self) -> Result<Option<String>, Ctrl<'p>> {
        if let Some(input) = &mut *self.input.borrow_mut() {
            return Ok(input.pop_front());
        }
        if self.offline {
            return raise("HumanError", "no human in tests: wrap the code in `with_human(approve_all) do … end`");
        }
        Ok(grenat_green::blocking(|| {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
            }
        }))
    }
}
