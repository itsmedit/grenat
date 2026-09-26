//! Where a connection goes, as programs write it: `user@host`, with an
//! optional port (`user@host:2222`, `user@[::1]:2222`).

/// A user on a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub user: String,
    pub host: String,
    pub port: Option<u16>,
}

impl Target {
    /// `user@host`, `user@host:port`, `user@[v6]`, `user@[v6]:port`, or a
    /// bare IPv6 address after the `@`; `None` for anything else.
    pub(crate) fn parse(text: &str) -> Option<Target> {
        let (user, address) = text.rsplit_once('@')?;
        let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
            let (host, rest) = bracketed.split_once(']')?;
            match rest.strip_prefix(':') {
                Some(port) => (host, Some(port.parse().ok()?)),
                None if rest.is_empty() => (host, None),
                None => return None,
            }
        } else {
            match address.split_once(':') {
                // an IPv6 address without brackets has no port
                Some((_, rest)) if rest.contains(':') => (address, None),
                Some((host, port)) => (host, Some(port.parse().ok()?)),
                None => (address, None),
            }
        };
        let valid = !user.is_empty() && !host.is_empty() && port != Some(0);
        valid.then(|| Target { user: user.to_string(), host: host.to_string(), port })
    }

    /// `user@host`: how logs, errors and `mock_ssh` name it.
    pub(crate) fn label(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }
}

#[cfg(test)]
mod tests {
    use super::Target;

    fn target(user: &str, host: &str, port: Option<u16>) -> Option<Target> {
        Some(Target { user: user.into(), host: host.into(), port })
    }

    #[test]
    fn users_hosts_and_ports() {
        assert_eq!(Target::parse("deploy@api.acme.com"), target("deploy", "api.acme.com", None));
        assert_eq!(Target::parse("deploy@api.acme.com:2222"), target("deploy", "api.acme.com", Some(2222)));
        assert_eq!(Target::parse("root@[::1]:22"), target("root", "::1", Some(22)));
        assert_eq!(Target::parse("root@[::1]"), target("root", "::1", None));
        assert_eq!(Target::parse("root@fe80::1"), target("root", "fe80::1", None));
        assert_eq!(Target::parse("a@b@10.0.0.1"), target("a@b", "10.0.0.1", None));
        assert_eq!(Target::parse("deploy@api.acme.com").unwrap().label(), "deploy@api.acme.com");
    }

    #[test]
    fn what_names_no_server() {
        for text in ["api.acme.com", "@host", "user@", "user@host:", "user@host:0", "user@host:x", "u@[::1", "u@[::1]x"]
        {
            assert_eq!(Target::parse(text), None, "{text}");
        }
    }
}
