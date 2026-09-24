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

    pub(crate) fn read_line(&self) -> Option<String> {
        if let Some(input) = &mut *self.input.borrow_mut() {
            return input.pop_front();
        }
        grenat_green::blocking(|| {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
            }
        })
    }
}
