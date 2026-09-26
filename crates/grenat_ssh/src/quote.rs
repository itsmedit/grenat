//! A remote command is an argv; SSH carries one string, which the server
//! hands to the user's POSIX shell. Every argument is quoted here so that
//! the shell sees exactly that argument, whatever it contains: no
//! expansion, no second command, no word splitting.

use crate::error::{Error, ErrorKind};

/// The command line the remote shell will split back into `argv`.
pub fn command_line<S: AsRef<str>>(argv: &[S]) -> Result<String, Error> {
    if argv.is_empty() {
        return Err(Error::new(ErrorKind::Command, "a remote command cannot be empty"));
    }
    let mut words = Vec::with_capacity(argv.len());
    for arg in argv {
        let arg = arg.as_ref();
        if arg.contains('\0') {
            return Err(Error::new(ErrorKind::Command, "an argument of a remote command cannot contain a NUL byte"));
        }
        words.push(quote(arg));
    }
    Ok(words.join(" "))
}

/// One argument as a single shell word: left bare when made only of
/// characters no shell treats specially, otherwise inside single quotes,
/// where nothing is special but the closing quote (written `'\''`).
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(is_plain) {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// `=` is not plain: `A=b cmd` would be an assignment, not a command `A=b`;
/// `~` is not plain: it expands to a home directory.
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ',' | ':' | '@' | '%' | '+')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_stay_bare() {
        assert_eq!(
            command_line(&["ls", "-la", "/var/log", "a.b_c+d@e:f,g%h"]).unwrap(),
            "ls -la /var/log a.b_c+d@e:f,g%h"
        );
    }

    #[test]
    fn hostile_arguments_become_single_words() {
        let cases = [
            ("; rm -rf /", "'; rm -rf /'"),
            ("$(id)", "'$(id)'"),
            ("`id`", "'`id`'"),
            ("a'b", r"'a'\''b'"),
            ("'", r"''\'''"),
            ("line1\nline2", "'line1\nline2'"),
            ("", "''"),
            ("*", "'*'"),
            ("$HOME", "'$HOME'"),
            ("~", "'~'"),
            ("A=b", "'A=b'"),
            ("a b", "'a b'"),
            ("x|y&z>w<v", "'x|y&z>w<v'"),
            ("\\", "'\\'"),
        ];
        for (arg, quoted) in cases {
            assert_eq!(quote(arg), quoted, "{arg:?}");
        }
        assert_eq!(command_line(&["echo", "; rm -rf /", ""]).unwrap(), "echo '; rm -rf /' ''");
    }

    #[test]
    fn refusals() {
        let empty: [&str; 0] = [];
        assert_eq!(command_line(&empty).unwrap_err().kind(), ErrorKind::Command);
        let nul = command_line(&["echo", "a\0b"]).unwrap_err();
        assert_eq!(nul.kind(), ErrorKind::Command);
        assert!(nul.message().contains("NUL"));
    }
}
