//! **Where a database URL connects**: the network hosts it reaches, the
//! machine's own aside — what `Db.connect(url)` and `database url` must be
//! granted `net("host")` for. Nothing here knows the Grenat language: the
//! checker reads a literal URL with it, the runtime any other, alike.
//!
//! - SQLite (`sqlite://app.db`, `sqlite::memory:`) is a file: no host.
//! - PostgreSQL (`postgres://` or `postgresql://`) reaches the hosts of its
//!   authority (`user:password@host:port`, several separated by commas, an
//!   IPv6 address in brackets, percent-encoded) and of its `host=` and
//!   `hostaddr=` parameters. Local ones are not the network: `localhost`,
//!   `127.0.0.1`, `::1`, a Unix socket's directory (`/var/run/postgresql`,
//!   `%2Fvar%2Frun%2Fpostgresql`), and no host at all (the default socket).
//! - Any other URL connects nowhere (the database layer refuses it).
//!
//! The credentials a URL holds are never part of what is returned.

/// The hosts on the network `url` connects to, in order, each once; empty
/// for a local database.
pub fn remote_hosts(url: &str) -> Vec<String> {
    let Some(rest) = url.strip_prefix("postgres://").or_else(|| url.strip_prefix("postgresql://")) else {
        return Vec::new();
    };
    let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));
    let authority = before_query.split('/').next().unwrap_or_default();
    let address = authority.rsplit_once('@').map_or(authority, |(_, address)| address);
    let mut hosts: Vec<String> = Vec::new();
    let mut add = |host: String| {
        if !is_local(&host) && !hosts.contains(&host) {
            hosts.push(host);
        }
    };
    for part in address.split(',') {
        add(authority_host(&decode(part)));
    }
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        if key == "host" || key == "hostaddr" {
            for host in decode(value).split(',') {
                add(bare(host.trim()).to_string());
            }
        }
    }
    hosts
}

/// Whether `host` is this machine: a loopback name or address, a Unix
/// socket's directory, or none (the default socket).
pub fn is_local(host: &str) -> bool {
    let host = bare(host);
    host.is_empty()
        || host.starts_with('/')
        || host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
}

/// The host of `host:port`, `[v6]:port` or a socket path.
fn authority_host(address: &str) -> String {
    if address.starts_with('/') {
        return address.to_string();
    }
    if let Some(bracketed) = address.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or_default().to_string();
    }
    address.split(':').next().unwrap_or_default().to_string()
}

/// An IPv6 address without its brackets.
fn bare(host: &str) -> &str {
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host)
}

/// Percent-decoded (`%2Fvar%2Frun` → `/var/run`); an invalid escape is kept.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) =
                (bytes.get(i + 1).and_then(|b| hex(*b)), bytes.get(i + 2).and_then(|b| hex(*b)))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_and_other_urls_reach_no_host() {
        for url in ["sqlite://app.db", "sqlite::memory:", "sqlite:///var/db/x.db", "mysql://db.acme.io/x", ""] {
            assert!(remote_hosts(url).is_empty(), "{url}");
        }
    }

    #[test]
    fn a_postgres_url_reaches_its_host_without_its_credentials() {
        assert_eq!(remote_hosts("postgres://app:s3cr3t@db.acme.io:5432/app"), ["db.acme.io"]);
        assert_eq!(remote_hosts("postgresql://db.acme.io/app?sslmode=require"), ["db.acme.io"]);
        assert_eq!(remote_hosts("postgres://db.acme.io"), ["db.acme.io"]);
        // a password holding `@`, percent-encoded or not
        assert_eq!(remote_hosts("postgres://app:p%40ss@db.acme.io/app"), ["db.acme.io"]);
        assert_eq!(remote_hosts("postgres://app:p@ss@db.acme.io/app"), ["db.acme.io"]);
        assert_eq!(remote_hosts("postgres://u@[2001:db8::1]:5432/app"), ["2001:db8::1"]);
        assert_eq!(remote_hosts("postgres://u@10.0.0.5/app"), ["10.0.0.5"]);
    }

    #[test]
    fn every_host_is_reached() {
        assert_eq!(
            remote_hosts("postgres://u@a.acme.io:5432,b.acme.io:5433,a.acme.io/app"),
            ["a.acme.io", "b.acme.io"]
        );
        assert_eq!(remote_hosts("postgres://u@localhost/app?host=db.acme.io"), ["db.acme.io"]);
        assert_eq!(remote_hosts("postgres:///app?hostaddr=10.0.0.5&sslmode=require"), ["10.0.0.5"]);
        assert_eq!(remote_hosts("postgres:///app?host=a.acme.io,%2Ftmp,b.acme.io"), ["a.acme.io", "b.acme.io"]);
    }

    #[test]
    fn local_databases_are_not_the_network() {
        for url in [
            "postgres://mehdifarsi@localhost:15432/postgres",
            "postgres://u:p@LOCALHOST/app",
            "postgres://u@127.0.0.1:5432/app",
            "postgres://u@[::1]:5432/app",
            "postgres:///app",
            "postgres://u@/app",
            "postgres://u@%2Fvar%2Frun%2Fpostgresql/app",
            "postgres:///app?host=/var/run/postgresql",
            "postgres://u@localhost/app?host=%2Ftmp&hostaddr=127.0.0.1",
        ] {
            assert!(remote_hosts(url).is_empty(), "{url}: {:?}", remote_hosts(url));
        }
        assert!(is_local("[::1]") && is_local("/tmp") && is_local(""));
        assert!(!is_local("127.0.0.2") && !is_local("0.0.0.0") && !is_local("db.local.acme.io"));
    }
}
