//! One connection to the fake server: the commands `on_email` sends —
//! CAPABILITY, STARTTLS, LOGIN, AUTHENTICATE XOAUTH2, SELECT, UID SEARCH,
//! UID FETCH, UID STORE, UID MOVE, UID COPY, UID EXPUNGE, EXPUNGE, NOOP,
//! LOGOUT — answered as RFC 3501 (and 6851 for MOVE, 4315 for UID
//! EXPUNGE) say, and as Gmail answers a refused OAuth 2.0 token.

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::{Arc, Mutex};

use super::store::{Fault, State};
use super::wire::{Conn, base64_decode, read_line, uid_set, words};
use super::{Config, Mode, PASSWORD, USER};

/// Gmail's answer to a token it refuses: a challenge, before its `NO`.
const REFUSED_TOKEN: &str =
    "eyJzdGF0dXMiOiI0MDAiLCJzY2hlbWVzIjoiQmVhcmVyIiwic2NvcGUiOiJodHRwczovL21haWwuZ29vZ2xlLmNvbS8ifQ==";

pub(crate) struct Session {
    pub conn: Conn,
    pub state: Arc<Mutex<State>>,
    pub config: Arc<Config>,
    pub tls: Option<Arc<rustls::ServerConfig>>,
    logged_in: bool,
    selected: Option<String>,
}

/// What a command leads to.
enum Next {
    Go,
    Close,
}

impl Session {
    pub(crate) fn new(
        conn: Conn,
        state: Arc<Mutex<State>>,
        config: Arc<Config>,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> Session {
        Session { conn, state, config, tls, logged_in: false, selected: None }
    }

    pub(crate) fn run(mut self) {
        if self.send("* OK fake IMAP ready").is_err() {
            return;
        }
        while let Some(line) = read_line(&mut self.conn) {
            let fault = {
                let mut state = self.state.lock().unwrap();
                let (_, rest) = line.split_once(' ').unwrap_or((&line, ""));
                state.commands.push((self.conn.is_tls(), rest.to_string()));
                match state.fault.as_mut() {
                    Some((0 | 1, fault)) => {
                        let fault = *fault;
                        state.fault = None;
                        Some(fault)
                    }
                    Some((n, _)) => {
                        *n -= 1;
                        None
                    }
                    None => None,
                }
            };
            match fault {
                Some(Fault::Drop) => return,
                // read on, answering nothing, until the client gives up
                Some(Fault::Stall) => {
                    while read_line(&mut self.conn).is_some() {}
                    return;
                }
                None => {}
            }
            match self.command(&line) {
                Ok(Next::Go) => {}
                Ok(Next::Close) | Err(_) => return,
            }
        }
    }

    fn send(&mut self, line: &str) -> std::io::Result<()> {
        self.conn.write_all(format!("{line}\r\n").as_bytes())?;
        self.conn.flush()
    }

    fn capabilities(&self) -> String {
        let mut caps = vec!["IMAP4rev1"];
        if self.config.rev2 {
            caps.push("IMAP4rev2");
        }
        let clear = !self.conn.is_tls();
        if clear && self.config.mode == Mode::StartTls {
            caps.extend(["STARTTLS", "LOGINDISABLED"]);
        }
        if self.config.login_disabled {
            caps.push("LOGINDISABLED");
        }
        if self.config.token.is_some() {
            caps.push("AUTH=XOAUTH2");
        }
        if self.config.move_command {
            caps.push("MOVE");
        }
        if self.config.uidplus {
            caps.push("UIDPLUS");
        }
        caps.join(" ")
    }

    fn command(&mut self, line: &str) -> std::io::Result<Next> {
        let words = words(line);
        let tag = words.first().cloned().unwrap_or_default();
        let name = words.get(1).map(|w| w.to_ascii_uppercase()).unwrap_or_default();
        let args = &words[words.len().min(2)..];
        match name.as_str() {
            "CAPABILITY" => {
                let caps = self.capabilities();
                self.send(&format!("* CAPABILITY {caps}"))?;
                self.send(&format!("{tag} OK CAPABILITY completed"))?;
            }
            "NOOP" => self.send(&format!("{tag} OK NOOP completed"))?,
            "LOGOUT" => {
                self.send("* BYE logging out")?;
                self.send(&format!("{tag} OK LOGOUT completed"))?;
                return Ok(Next::Close);
            }
            "STARTTLS" => return self.starttls(&tag),
            "LOGIN" => self.login(&tag, args)?,
            "AUTHENTICATE" => self.authenticate(&tag, args)?,
            _ if !self.logged_in => self.send(&format!("{tag} BAD log in first"))?,
            "SELECT" | "EXAMINE" => self.select(&tag, args)?,
            _ if self.selected.is_none() => self.send(&format!("{tag} BAD select a folder first"))?,
            "UID" => self.uid(&tag, args)?,
            "EXPUNGE" => {
                self.expunge(None)?;
                self.send(&format!("{tag} OK EXPUNGE completed"))?;
            }
            _ => self.send(&format!("{tag} BAD unknown command"))?,
        }
        Ok(Next::Go)
    }

    fn starttls(&mut self, tag: &str) -> std::io::Result<Next> {
        let (Some(config), Conn::Plain(_), Mode::StartTls) = (self.tls.clone(), &self.conn, self.config.mode) else {
            self.send(&format!("{tag} BAD STARTTLS is not offered"))?;
            return Ok(Next::Go);
        };
        self.send(&format!("{tag} OK begin TLS negotiation now"))?;
        let Conn::Plain(tcp) = std::mem::replace(&mut self.conn, Conn::Closed) else { unreachable!() };
        let connection = rustls::ServerConnection::new(config).map_err(std::io::Error::other)?;
        self.conn = Conn::Tls(Box::new(rustls::StreamOwned::new(connection, tcp)));
        Ok(Next::Go)
    }

    fn login(&mut self, tag: &str, args: &[String]) -> std::io::Result<()> {
        if self.capabilities().split(' ').any(|c| c == "LOGINDISABLED") {
            return self.send(&format!("{tag} NO [PRIVACYREQUIRED] LOGIN is disabled"));
        }
        if args.len() == 2 && args[0] == USER && args[1] == PASSWORD {
            self.logged_in = true;
            self.send(&format!("{tag} OK [CAPABILITY {}] logged in", self.capabilities()))
        } else {
            self.send(&format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)"))
        }
    }

    fn authenticate(&mut self, tag: &str, args: &[String]) -> std::io::Result<()> {
        if !args.first().is_some_and(|m| m.eq_ignore_ascii_case("XOAUTH2")) {
            return self.send(&format!("{tag} NO unsupported mechanism"));
        }
        self.send("+ ")?;
        let answer = read_line(&mut self.conn).unwrap_or_default();
        let expected = self.config.token.as_ref().map(|t| format!("user={USER}\x01auth=Bearer {t}\x01\x01"));
        let given = base64_decode(answer.trim()).map(|b| String::from_utf8_lossy(&b).into_owned());
        if expected.is_some() && given == expected {
            self.logged_in = true;
            return self.send(&format!("{tag} OK SASL authentication succeeded"));
        }
        self.send(&format!("+ {REFUSED_TOKEN}"))?;
        // the client's empty answer
        read_line(&mut self.conn);
        self.send(&format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)"))
    }

    fn select(&mut self, tag: &str, args: &[String]) -> std::io::Result<()> {
        let name = args.first().cloned().unwrap_or_default();
        let found = {
            let state = self.state.lock().unwrap();
            state.folders.get(&name).map(|f| (f.messages.len(), f.uid_validity, f.next_uid))
        };
        let Some((exists, validity, next)) = found else {
            self.selected = None;
            return self.send(&format!("{tag} NO [NONEXISTENT] Unknown Mailbox: {name} (Failure)"));
        };
        self.selected = Some(name);
        self.send("* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)")?;
        self.send(&format!("* {exists} EXISTS"))?;
        self.send("* 0 RECENT")?;
        self.send(&format!("* OK [UIDVALIDITY {validity}] UIDs valid"))?;
        self.send(&format!("* OK [UIDNEXT {next}] Predicted next UID"))?;
        self.send(&format!("{tag} OK [READ-WRITE] SELECT completed"))
    }

    fn uid(&mut self, tag: &str, args: &[String]) -> std::io::Result<()> {
        let sub = args.first().map(|s| s.to_ascii_uppercase()).unwrap_or_default();
        let rest = &args[args.len().min(1)..];
        match sub.as_str() {
            "SEARCH" => self.search(rest)?,
            "FETCH" => self.fetch(rest)?,
            "STORE" => self.store(rest)?,
            "COPY" => {
                if let Some(refused) = self.copy(rest, false)? {
                    return self.send(&format!("{tag} {refused}"));
                }
            }
            "MOVE" if self.config.move_command || self.config.rev2 => {
                if let Some(refused) = self.copy(rest, true)? {
                    return self.send(&format!("{tag} {refused}"));
                }
            }
            "EXPUNGE" if self.config.uidplus || self.config.rev2 => {
                let set = rest.first().cloned().unwrap_or_default();
                self.expunge(Some(&set))?;
            }
            _ => return self.send(&format!("{tag} BAD unknown UID command")),
        }
        self.send(&format!("{tag} OK UID {sub} completed"))
    }

    fn search(&mut self, criteria: &[String]) -> std::io::Result<()> {
        let found: Vec<String> = {
            let mut state = self.state.lock().unwrap();
            let folder = state.folder(self.selected.as_ref().expect("selected"));
            folder
                .messages
                .iter()
                .filter(|m| {
                    criteria.iter().all(|c| match c.to_ascii_uppercase().as_str() {
                        "UNSEEN" => !m.has("\\Seen"),
                        "SEEN" => m.has("\\Seen"),
                        "UNDELETED" => !m.has("\\Deleted"),
                        "DELETED" => m.has("\\Deleted"),
                        "FLAGGED" => m.has("\\Flagged"),
                        _ => true,
                    })
                })
                .map(|m| m.uid.to_string())
                .collect()
        };
        let mut line = String::from("* SEARCH");
        for uid in found {
            line.push(' ');
            line.push_str(&uid);
        }
        self.send(&line)
    }

    fn fetch(&mut self, args: &[String]) -> std::io::Result<()> {
        let (set, items) = (args.first().cloned().unwrap_or_default(), args[1..].join(" ").to_ascii_uppercase());
        let mut out: Vec<u8> = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            let folder = state.folder(self.selected.as_ref().expect("selected"));
            for uid in uid_set(&set, &folder.uids()) {
                let (seq, message) = folder.find(uid).expect("listed");
                let mut parts = format!("UID {uid}");
                if items.contains("RFC822.SIZE") {
                    parts.push_str(&format!(" RFC822.SIZE {}", message.raw.len()));
                }
                out.extend_from_slice(format!("* {seq} FETCH ({parts}").as_bytes());
                if items.contains("BODY[]") || items.contains("BODY.PEEK[]") {
                    // `BODY[]` marks the message seen, `BODY.PEEK[]` does not
                    if !items.contains("BODY.PEEK[]") {
                        message.flags.insert("\\Seen".into());
                    }
                    out.extend_from_slice(format!(" BODY[] {{{}}}\r\n", message.raw.len()).as_bytes());
                    out.extend_from_slice(&message.raw);
                }
                out.extend_from_slice(b")\r\n");
            }
        }
        self.conn.write_all(&out)?;
        self.conn.flush()
    }

    fn store(&mut self, args: &[String]) -> std::io::Result<()> {
        let [set, operation, flags, ..] = args else { return Ok(()) };
        let flags: BTreeSet<String> = flags.trim_matches(['(', ')']).split_whitespace().map(capitalized).collect();
        let operation = operation.to_ascii_uppercase();
        let mut lines = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            let folder = state.folder(self.selected.as_ref().expect("selected"));
            for uid in uid_set(set, &folder.uids()) {
                let (seq, message) = folder.find(uid).expect("listed");
                match operation.trim_end_matches(".SILENT") {
                    "+FLAGS" => message.flags.extend(flags.iter().cloned()),
                    "-FLAGS" => message.flags.retain(|f| !flags.contains(f)),
                    _ => message.flags = flags.clone(),
                }
                if !operation.ends_with(".SILENT") {
                    let now: Vec<&str> = message.flags.iter().map(String::as_str).collect();
                    lines.push(format!("* {seq} FETCH (UID {uid} FLAGS ({}))", now.join(" ")));
                }
            }
        }
        for line in lines {
            self.send(&line)?;
        }
        Ok(())
    }

    /// `UID COPY` or `UID MOVE`: a refusal for the tagged answer, if any.
    fn copy(&mut self, args: &[String], moving: bool) -> std::io::Result<Option<String>> {
        let (Some(set), Some(target)) = (args.first(), args.get(1)) else { return Ok(Some("BAD arguments".into())) };
        let mut lines = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            if !state.folders.contains_key(target) {
                return Ok(Some(format!("NO [TRYCREATE] no folder {target}")));
            }
            let source = self.selected.clone().expect("selected");
            let uids = uid_set(set, &state.folder(&source).uids());
            let (mut from, mut to) = (Vec::new(), Vec::new());
            for uid in uids {
                let folder = state.folder(&source);
                let copied = if moving {
                    let (seq, message) = folder.remove(uid).expect("listed");
                    lines.push(format!("* {seq} EXPUNGE"));
                    message
                } else {
                    folder.find(uid).expect("listed").1.clone()
                };
                let added = state.folder(target).add(copied.raw, copied.flags);
                from.push(uid.to_string());
                to.push(added.to_string());
            }
            let validity = state.folder(target).uid_validity;
            if !from.is_empty() {
                lines.insert(0, format!("* OK [COPYUID {validity} {} {}]", from.join(","), to.join(",")));
            }
        }
        for line in lines {
            self.send(&line)?;
        }
        Ok(None)
    }

    /// Removes the deleted messages (among `set`, if given).
    fn expunge(&mut self, set: Option<&str>) -> std::io::Result<()> {
        let mut lines = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            let folder = state.folder(self.selected.as_ref().expect("selected"));
            let deleted: Vec<u32> = folder.messages.iter().filter(|m| m.has("\\Deleted")).map(|m| m.uid).collect();
            let chosen = set.map_or(deleted.clone(), |s| uid_set(s, &deleted));
            for uid in chosen {
                let (seq, _) = folder.remove(uid).expect("listed");
                lines.push(format!("* {seq} EXPUNGE"));
            }
        }
        for line in lines {
            self.send(&line)?;
        }
        Ok(())
    }
}

/// `\seen` → `\Seen`: flags compare without case.
fn capitalized(flag: &str) -> String {
    match flag.to_ascii_lowercase().as_str() {
        "\\seen" => "\\Seen".into(),
        "\\flagged" => "\\Flagged".into(),
        "\\deleted" => "\\Deleted".into(),
        "\\answered" => "\\Answered".into(),
        "\\draft" => "\\Draft".into(),
        _ => flag.to_string(),
    }
}
