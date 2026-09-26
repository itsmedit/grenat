//! `mock_ssh "deploy@api.acme.com", commands: {…}, files: {…}`: a server
//! for the rest of a test. A command's answer is its output, or a hash of
//! `stdout:`, `stderr:` and `status:`.

use grenat_ssh::Output;

use crate::prelude::*;
use crate::ssh::{Double, Target};

impl<'p> Interp<'p> {
    pub(crate) fn mock_ssh(&mut self, args: &Args<'p>) -> R<'p> {
        let target = match args.pos.first().map(Value::untainted) {
            Some(Value::Str(text)) => Target::parse(text),
            _ => None,
        };
        let Some(target) = target else {
            return raise(
                "ArgumentError",
                "`mock_ssh` expects a server: `mock_ssh \"deploy@api.acme.com\", commands: {…}`",
            );
        };
        let (mut commands, mut files) = (Vec::new(), Vec::new());
        for (option, value) in &args.named {
            match (option.as_str(), value.untainted()) {
                ("commands", Value::Hash(entries)) => {
                    for (line, answer) in entries.borrow().iter() {
                        commands.push((line.to_display(), output(answer)?));
                    }
                }
                ("files", Value::Hash(entries)) => {
                    files.extend(
                        entries
                            .borrow()
                            .iter()
                            .map(|(path, content)| (path.to_display(), content.to_display().into_bytes())),
                    );
                }
                (option, v) => {
                    return raise("ArgumentError", format!("invalid `mock_ssh` option `{option}: {}`", v.inspect()));
                }
            }
        }
        let label = target.label();
        let double = Double::new(&label, commands, files);
        self.ssh_stubs.borrow_mut().insert(label, Arc::new(Mutex::new(double)));
        Ok(Value::Nil)
    }
}

/// What a mocked command does: print a text, or `{stdout:, stderr:, status:}`.
fn output<'p>(answer: &Value<'p>) -> Result<Output, Ctrl<'p>> {
    let Value::Hash(entries) = answer.untainted() else {
        return Ok(Output { status: Some(0), stdout: answer.to_display().into_bytes(), ..Output::default() });
    };
    let mut output = Output { status: Some(0), ..Output::default() };
    for (key, value) in entries.borrow().iter() {
        match (key.to_display().as_str(), value.untainted()) {
            ("stdout", v) => output.stdout = v.to_display().into_bytes(),
            ("stderr", v) => output.stderr = v.to_display().into_bytes(),
            ("status", Value::Int(n)) if (0..=255).contains(n) => output.status = Some(*n as u32),
            (key, v) => {
                return raise(
                    "ArgumentError",
                    format!("invalid `mock_ssh` command answer `{key}: {}` (stdout:, stderr:, status:)", v.inspect()),
                );
            }
        }
    }
    Ok(output)
}
