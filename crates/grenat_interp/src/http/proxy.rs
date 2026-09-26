//! Proxies of the HTTP transport: which proxy a request goes through (one
//! it names, the environment's as curl reads it, or none), checking a proxy
//! URL, and naming one without its credentials.

use super::host;

/// The proxy a request asks for.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum ProxyChoice {
    /// The environment's (`https_proxy`, `http_proxy`, `all_proxy`,
    /// `no_proxy`), as curl reads it; none when nothing is set.
    #[default]
    Environment,
    /// No proxy at all, whatever the environment says.
    Direct,
    /// This proxy URL (`socks5://user:pass@host:1080`).
    Url(String),
}

/// The proxy schemes understood, as `scheme://`.
const SCHEMES: &[&str] = &["http", "https", "socks4", "socks4a", "socks5", "socks5h"];

/// Checks that `url` is a proxy URL. The reason given never repeats the URL,
/// which may hold a password.
pub(crate) fn validate(url: &str) -> Result<(), String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(expected_scheme());
    };
    if !SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(expected_scheme());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let port_ok = match host_port.rsplit_once(':') {
        Some((_, port)) if !host_port.ends_with(']') => port.parse::<u16>().is_ok(),
        _ => true,
    };
    if host_port.is_empty() || host_port.starts_with(':') || !port_ok || ureq::Proxy::new(url).is_err() {
        return Err("a proxy URL is `scheme://[user:password@]host[:port]`".into());
    }
    Ok(())
}

fn expected_scheme() -> String {
    let schemes: Vec<String> = SCHEMES.iter().map(|s| format!("{s}://")).collect();
    format!("a proxy URL starts with {}", schemes.join(", "))
}

/// The proxy for a request to `target`: `Ok(None)` to go direct. `env`
/// reads an environment variable (injected, so this is testable).
pub(crate) fn resolve(
    choice: &ProxyChoice,
    target: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<ureq::Proxy>, String> {
    let (url, origin) = match choice {
        ProxyChoice::Direct => return Ok(None),
        ProxyChoice::Url(url) => (url.clone(), None),
        ProxyChoice::Environment => match from_env(target, env) {
            Some((variable, url)) => (url, Some(variable)),
            None => return Ok(None),
        },
    };
    let invalid = |reason: String| match origin {
        Some(variable) => format!("invalid proxy in `{variable}`: {reason}"),
        None => format!("invalid proxy: {reason}"),
    };
    validate(&url).map_err(invalid)?;
    ureq::Proxy::new(&url).map(Some).map_err(|e| invalid(e.to_string()))
}

/// The environment's proxy for `target` and the variable that names it, as
/// curl chooses: `no_proxy` first, then `https_proxy` for an https URL or
/// `http_proxy` for an http one (lowercase only: `HTTP_PROXY` may be set by
/// a CGI request header), then `all_proxy`. Lowercase names come first.
pub(crate) fn from_env(target: &str, env: impl Fn(&str) -> Option<String>) -> Option<(&'static str, String)> {
    let read = |name: &str| env(name).filter(|v| !v.trim().is_empty());
    let host = host(target)?;
    let excluded = read("no_proxy").or_else(|| read("NO_PROXY"));
    if excluded.is_some_and(|list| bypasses(&list, host)) {
        return None;
    }
    let names: &[&'static str] = if target.starts_with("https://") {
        &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    } else {
        &["http_proxy", "all_proxy", "ALL_PROXY"]
    };
    names.iter().find_map(|name| read(name).map(|value| (*name, value.trim().to_string())))
}

/// Whether `no_proxy` (`localhost, .internal.io, *`) exempts `host`: an entry
/// matches the host itself and its subdomains, a leading dot or not.
pub(crate) fn bypasses(no_proxy: &str, host: &str) -> bool {
    let bare = |name: &str| name.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let host = bare(host);
    no_proxy.split([',', ' ']).map(str::trim).filter(|e| !e.is_empty()).any(|entry| {
        let entry = bare(entry.trim_start_matches("*.").trim_start_matches('.'));
        entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))
    })
}

/// `url` without its credentials (`socks5://u:p@host:1080` →
/// `socks5://host:1080`), to be shown.
pub(crate) fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    format!("{scheme}://{host_port}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn proxy_urls_are_checked() {
        for url in [
            "socks5://127.0.0.1:1080",
            "socks5h://user:p@ss@proxy.local:1080",
            "SOCKS4://h:1",
            "socks4a://h",
            "http://proxy:3128",
            "https://proxy",
            "http://[::1]:8080",
        ] {
            assert_eq!(validate(url), Ok(()), "{url}");
        }
        for url in ["127.0.0.1:1080", "ftp://h", "socks5//h", ""] {
            assert!(validate(url).unwrap_err().starts_with("a proxy URL starts with http://"), "{url}");
        }
        for url in ["socks5://", "socks5://:1080", "socks5://h:port", "socks5://h:99999", "socks5://u:s3cr3t@"] {
            let reason = validate(url).unwrap_err();
            assert_eq!(reason, "a proxy URL is `scheme://[user:password@]host[:port]`", "{url}");
        }
    }

    #[test]
    fn a_named_proxy_or_none() {
        let url = ProxyChoice::Url("socks5h://u:p@proxy:1081".into());
        let proxy = resolve(&url, "https://x.io", env(&[])).unwrap().unwrap();
        assert_eq!((proxy.protocol(), proxy.host(), proxy.port()), (ureq::ProxyProtocol::Socks5h, "proxy", 1081));
        assert_eq!((proxy.username(), proxy.password()), (Some("u"), Some("p")));
        let everywhere = env(&[("ALL_PROXY", "socks5://proxy:1080")]);
        assert!(resolve(&ProxyChoice::Direct, "https://x.io", everywhere).unwrap().is_none());
        assert!(resolve(&ProxyChoice::Environment, "https://x.io", env(&[])).unwrap().is_none());
        let bad = ProxyChoice::Url("gopher://u:s3cr3t@h".into());
        assert_eq!(
            resolve(&bad, "https://x.io", env(&[])).unwrap_err(),
            format!("invalid proxy: {}", expected_scheme())
        );
    }

    #[test]
    fn the_environment_is_read_as_curl_reads_it() {
        let vars = env(&[
            ("https_proxy", "socks5://secure:1080"),
            ("HTTPS_PROXY", "socks5://ignored:1080"),
            ("HTTP_PROXY", "http://cgi-header:80"),
            ("ALL_PROXY", "http://all:3128"),
        ]);
        assert_eq!(from_env("https://x.io/a", &vars), Some(("https_proxy", "socks5://secure:1080".into())));
        // `HTTP_PROXY` in capitals is never read
        assert_eq!(from_env("http://x.io/a", &vars), Some(("ALL_PROXY", "http://all:3128".into())));
        let vars = env(&[("http_proxy", " http://plain:80 "), ("all_proxy", "")]);
        assert_eq!(from_env("http://x.io", &vars), Some(("http_proxy", "http://plain:80".into())));
        assert_eq!(from_env("https://x.io", &vars), None);
        let proxy = resolve(&ProxyChoice::Environment, "http://x.io", &vars).unwrap().unwrap();
        assert_eq!(proxy.host(), "plain");
    }

    #[test]
    fn no_proxy_exempts_hosts() {
        let vars = env(&[("ALL_PROXY", "socks5://p:1080"), ("NO_PROXY", "localhost, .internal.io,*.corp")]);
        assert_eq!(from_env("http://localhost:8080/x", &vars), None);
        assert_eq!(from_env("https://api.internal.io", &vars), None);
        assert_eq!(from_env("https://internal.io", &vars), None);
        assert_eq!(from_env("https://a.b.corp", &vars), None);
        assert!(from_env("https://notinternal.io", &vars).is_some());
        assert!(from_env("https://x.io", &vars).is_some());
        let all = env(&[("all_proxy", "socks5://p:1080"), ("no_proxy", "*")]);
        assert_eq!(from_env("https://x.io", &all), None);
        assert!(bypasses("[::1]", "[::1]") && bypasses("::1", "[::1]"));
        assert!(bypasses("Example.COM", "www.example.com"));
        assert!(!bypasses("", "x.io"));
    }

    #[test]
    fn an_invalid_environment_proxy_names_its_variable_not_its_value() {
        let vars = env(&[("https_proxy", "nonsense://u:s3cr3t@h")]);
        let e = resolve(&ProxyChoice::Environment, "https://x.io", vars).unwrap_err();
        assert!(e.starts_with("invalid proxy in `https_proxy`: a proxy URL starts with"), "{e}");
        assert!(!e.contains("s3cr3t"), "{e}");
    }

    #[test]
    fn credentials_are_not_shown() {
        assert_eq!(without_credentials("socks5://u:s3cr3t@h:1080"), "socks5://h:1080");
        assert_eq!(without_credentials("http://u:p@ss@h:3128/"), "http://h:3128/");
        assert_eq!(without_credentials("socks5://h:1080"), "socks5://h:1080");
        assert_eq!(without_credentials("nonsense"), "nonsense");
    }
}
