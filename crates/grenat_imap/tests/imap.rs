//! Connections against the in-process server: implicit TLS, STARTTLS and
//! clear text on this machine; logging in with a password or a token;
//! unseen messages fetched in bounded batches and left unseen; flags and
//! moves (MOVE, COPY + UID EXPUNGE, COPY + EXPUNGE); folders in UTF-7;
//! UIDVALIDITY; reconnections and timeouts; secrets kept out of errors.

use std::time::Duration;

use grenat_imap::fake::{Config, FakeImap, Mode, PASSWORD};
use grenat_imap::{ErrorKind, Fetched, Inbox, Login, Mailbox, MailboxUrl, Message, Options, Trust};

fn message(subject: &str) -> Vec<u8> {
    format!("From: ada@acme.com\r\nTo: support@acme.com\r\nSubject: {subject}\r\n\r\nBody of {subject}.\r\n")
        .into_bytes()
}

fn options(server: &FakeImap) -> Options {
    Options { trust: Trust::default().with(server.authority()), timeout: Duration::from_secs(10), ..Options::default() }
}

fn url(server: &FakeImap, scheme: &str, folder: &str) -> MailboxUrl {
    MailboxUrl::parse(&server.url(scheme, folder)).unwrap()
}

fn open(server: &FakeImap, scheme: &str) -> Mailbox {
    Mailbox::open(&url(server, scheme, "INBOX"), Login::Password, &options(server)).unwrap_or_else(|e| panic!("{e}"))
}

fn subjects(fetched: &[Fetched]) -> Vec<String> {
    fetched
        .iter()
        .map(|f| match f {
            Fetched::Message { raw, .. } => Message::parse(raw).unwrap().subject,
            Fetched::TooLarge { uid, size } => format!("#{uid} too large ({size})"),
        })
        .collect()
}

/// The commands the server received, without the TLS marker.
fn commands(server: &FakeImap) -> Vec<String> {
    server.commands().into_iter().map(|(_, c)| c).collect()
}

#[test]
fn implicit_tls_reads_the_unseen_messages_and_leaves_them_unseen() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("first"));
    server.deliver_with("INBOX", &message("already read"), &["\\Seen"]);
    server.deliver_with("INBOX", &message("deleted"), &["\\Deleted"]);
    server.deliver("INBOX", &message("second"));
    let mut mailbox = open(&server, "imaps");
    assert_eq!(mailbox.uid_validity(), server.uid_validity("INBOX"));
    let unseen = mailbox.unseen().unwrap();
    assert_eq!(unseen, [1, 4]);
    assert_eq!(subjects(&mailbox.fetch(&unseen).unwrap()), ["first", "second"]);
    // BODY.PEEK[]: nothing was marked seen
    assert!(server.messages("INBOX").iter().filter(|m| m.has("\\Seen")).all(|m| m.uid == 2));
    assert_eq!(mailbox.unseen().unwrap(), [1, 4]);
    mailbox.logout();
    // everything went over TLS, LOGIN included
    let sent = server.commands();
    assert!(sent.iter().all(|(tls, _)| *tls), "{sent:?}");
    assert!(sent.iter().any(|(_, c)| c == "UID FETCH 1,4 (UID BODY.PEEK[])"), "{sent:?}");
    assert!(sent.iter().any(|(_, c)| c.starts_with("LOGIN ")));
    assert_eq!(sent.last().unwrap().1, "LOGOUT");
}

#[test]
fn starttls_protects_the_connection_before_logging_in() {
    let server = FakeImap::start(Config::new(Mode::StartTls));
    server.deliver("INBOX", &message("over starttls"));
    let mut mailbox = open(&server, "imap");
    let unseen = mailbox.unseen().unwrap();
    assert_eq!(subjects(&mailbox.fetch(&unseen).unwrap()), ["over starttls"]);
    let sent = server.commands();
    // in clear text: the capabilities and STARTTLS, nothing else
    let clear: Vec<&str> = sent.iter().filter(|(tls, _)| !tls).map(|(_, c)| c.as_str()).collect();
    assert_eq!(clear, ["CAPABILITY", "STARTTLS"]);
    // over TLS: the capabilities asked again, then the login
    let protected: Vec<&str> = sent.iter().filter(|(tls, _)| *tls).map(|(_, c)| c.as_str()).take(2).collect();
    assert_eq!(protected[0], "CAPABILITY");
    assert!(protected[1].starts_with("LOGIN "), "{protected:?}");
}

#[test]
fn a_server_without_starttls_never_gets_the_password() {
    let server = FakeImap::start(Config::new(Mode::Plain));
    let e = Mailbox::open(&url(&server, "imap", "INBOX"), Login::Password, &options(&server)).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Insecure, "{e}");
    assert!(e.message().contains("offers no STARTTLS"), "{e}");
    assert_eq!(commands(&server), ["CAPABILITY"]);
}

#[test]
fn clear_text_only_on_this_machine_and_only_when_asked() {
    let server = FakeImap::start(Config::new(Mode::Plain));
    server.deliver("INBOX", &message("local"));
    let local = url(&server, "imap", "INBOX").without_tls().unwrap();
    let mut mailbox = Mailbox::open(&local, Login::Password, &options(&server)).unwrap();
    assert_eq!(mailbox.unseen().unwrap(), [1]);
    assert!(server.commands().iter().all(|(tls, _)| !tls));
    // a remote host is refused before anything is sent
    let remote = MailboxUrl::parse("imap://u:s3cret@imap.example.com/INBOX").unwrap().without_tls().unwrap_err();
    assert_eq!(remote.kind(), ErrorKind::Insecure);
    assert!(remote.message().contains("imap.example.com") && !remote.message().contains("s3cret"), "{remote}");
}

#[test]
fn an_unknown_certificate_is_refused() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let trusting_nothing_extra = Options { timeout: Duration::from_secs(10), ..Options::default() };
    let e = Mailbox::open(&url(&server, "imaps", "INBOX"), Login::Password, &trusting_nothing_extra).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Tls, "{e}");
    assert!(server.commands().is_empty());
    // the same over STARTTLS: refused before LOGIN
    let server = FakeImap::start(Config::new(Mode::StartTls));
    let e = Mailbox::open(&url(&server, "imap", "INBOX"), Login::Password, &trusting_nothing_extra).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Tls, "{e}");
    assert_eq!(commands(&server), ["CAPABILITY", "STARTTLS"]);
}

#[test]
fn a_wrong_password_is_refused_and_never_shown() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let wrong =
        MailboxUrl::parse(&format!("imaps://support%40acme.com:hunter2@127.0.0.1:{}/INBOX", server.port())).unwrap();
    let e = Mailbox::open(&wrong, Login::Password, &options(&server)).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth, "{e}");
    assert!(e.message().contains("logging in on 127.0.0.1") && e.message().contains("Invalid credentials"), "{e}");
    // neither the password nor the user: logs and events name the host
    for hidden in ["hunter2", "support@acme.com", "support%40acme.com"] {
        assert!(!e.message().contains(hidden) && !format!("{e:?}").contains(hidden), "{e:?}");
    }
    assert!(!format!("{wrong:?}").contains("hunter2"));
}

#[test]
fn login_disabled_means_no_password_is_sent() {
    let server = FakeImap::start(Config { login_disabled: true, ..Config::new(Mode::Implicit) });
    let e = Mailbox::open(&url(&server, "imaps", "INBOX"), Login::Password, &options(&server)).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth, "{e}");
    assert!(e.message().contains("LOGINDISABLED"), "{e}");
    assert_eq!(commands(&server), ["CAPABILITY"]);
    assert!(!e.message().contains(PASSWORD));
}

#[test]
fn an_oauth2_token_logs_in_and_a_bad_one_is_never_shown() {
    let server = FakeImap::start(Config { token: Some("ya29.good-token".into()), ..Config::new(Mode::Implicit) });
    server.deliver("INBOX", &message("with a token"));
    let mut inbox = Inbox::new(url(&server, "imaps", "INBOX"), options(&server));
    inbox.set_token(Some("ya29.good-token".into()));
    assert_eq!(inbox.with(|m| m.unseen()).unwrap(), [1]);
    assert!(commands(&server).iter().any(|c| c == "AUTHENTICATE XOAUTH2"));
    assert!(!commands(&server).iter().any(|c| c.starts_with("LOGIN")));

    let mut inbox = Inbox::new(url(&server, "imaps", "INBOX"), options(&server));
    inbox.set_token(Some("ya29.expired".into()));
    let e = inbox.with(|m| m.unseen()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Auth, "{e}");
    assert!(!e.message().contains("ya29.expired") && !format!("{e:?}").contains("ya29.expired"), "{e:?}");
    assert!(format!("{:?}", Login::OAuth2("ya29.expired")).contains("[hidden]"));
}

#[test]
fn a_server_without_xoauth2_is_sent_no_token() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let e = Mailbox::open(&url(&server, "imaps", "INBOX"), Login::OAuth2("ya29.t"), &options(&server)).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Auth, "{e}");
    assert!(e.message().contains("AUTH=XOAUTH2") && !e.message().contains("ya29.t"), "{e}");
    assert_eq!(commands(&server), ["CAPABILITY"]);
}

#[test]
fn flags_seen_and_flagged() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let (a, b) = (server.deliver("INBOX", &message("a")), server.deliver("INBOX", &message("b")));
    let mut mailbox = open(&server, "imaps");
    mailbox.mark_seen(a).unwrap();
    mailbox.mark_flagged(b).unwrap();
    let stored = server.messages("INBOX");
    assert!(stored[0].has("\\Seen") && !stored[0].has("\\Flagged"));
    assert!(stored[1].has("\\Seen") && stored[1].has("\\Flagged"));
    assert!(mailbox.unseen().unwrap().is_empty());
    assert!(commands(&server).iter().any(|c| c == "UID STORE 2 +FLAGS.SILENT (\\Seen \\Flagged)"));
}

/// Moves message `b` to `Done` on a server configured by `config`; the
/// commands that did it.
fn moved_with(config: Config) -> Vec<String> {
    let server = FakeImap::start(config);
    server.create("Done");
    server.deliver_with("INBOX", &message("kept, deleted by its owner"), &["\\Deleted"]);
    let b = server.deliver("INBOX", &message("b"));
    server.deliver("INBOX", &message("c"));
    let mut mailbox = open(&server, "imaps");
    mailbox.move_to(b, "Done").unwrap();
    let done = server.messages("Done");
    assert_eq!(done.len(), 1);
    // moved as it was: seen only by whoever reads it there
    assert!(!done[0].has("\\Seen") && !done[0].has("\\Deleted"), "{:?}", done[0].flags);
    assert_eq!(Message::parse(&done[0].raw).unwrap().subject, "b");
    assert!(!server.messages("INBOX").iter().any(|m| m.uid == b));
    let sent = commands(&server);
    let at = sent.iter().position(|c| c.starts_with("UID MOVE 2") || c.starts_with("UID COPY 2")).unwrap();
    sent[at..].iter().filter(|c| *c != "LOGOUT").cloned().collect()
}

#[test]
fn moves_with_move_when_offered() {
    assert_eq!(moved_with(Config::new(Mode::Implicit)), ["UID MOVE 2 \"Done\""]);
    // IMAP4rev2 has MOVE without announcing it
    let rev2 = Config { rev2: true, move_command: false, uidplus: false, ..Config::new(Mode::Implicit) };
    assert_eq!(moved_with(rev2), ["UID MOVE 2 \"Done\""]);
}

#[test]
fn moves_with_copy_and_uid_expunge_under_uidplus() {
    let config = Config { move_command: false, ..Config::new(Mode::Implicit) };
    assert_eq!(moved_with(config), ["UID COPY 2 \"Done\"", "UID STORE 2 +FLAGS.SILENT (\\Deleted)", "UID EXPUNGE 2"]);
}

#[test]
fn moves_with_copy_and_expunge_otherwise() {
    let config = Config { move_command: false, uidplus: false, ..Config::new(Mode::Implicit) };
    let sent = moved_with(config);
    assert_eq!(sent.last().unwrap(), "EXPUNGE");
    assert!(sent.contains(&"UID COPY 2 \"Done\"".to_string()), "{sent:?}");
}

#[test]
fn copy_gets_its_folder_quoted() {
    let server = FakeImap::start(Config { move_command: false, ..Config::new(Mode::Implicit) });
    let folder = r#"Done "today" \ é"#;
    server.create(folder);
    let uid = server.deliver("INBOX", &message("copied"));
    open(&server, "imaps").move_to(uid, folder).unwrap();
    assert_eq!(server.messages(folder).len(), 1);
    let sent = commands(&server);
    assert!(sent.contains(&r#"UID COPY 1 "Done \"today\" \\ &AOk-""#.to_string()), "{sent:?}");
}

#[test]
fn moving_to_a_missing_folder_is_refused() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let uid = server.deliver("INBOX", &message("a"));
    let e = open(&server, "imaps").move_to(uid, "Nowhere").unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Refused, "{e}");
    assert!(e.message().contains("Nowhere"), "{e}");
    // left where it was, as it was: unseen, for the caller to decide
    let inbox = server.messages("INBOX");
    assert_eq!(inbox.len(), 1);
    assert!(inbox[0].flags.is_empty(), "{:?}", inbox[0].flags);
    // the same without MOVE
    let server = FakeImap::start(Config { move_command: false, ..Config::new(Mode::Implicit) });
    let uid = server.deliver("INBOX", &message("a"));
    assert_eq!(open(&server, "imaps").move_to(uid, "Nowhere").unwrap_err().kind(), ErrorKind::Refused);
    assert!(server.messages("INBOX")[0].flags.is_empty());
}

#[test]
fn folders_are_named_in_modified_utf7() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.create("Boîte/Clients 台北");
    server.create("Traité");
    let uid = server.deliver("Boîte/Clients 台北", &message("from Taipei"));
    let folder = MailboxUrl::parse(&server.url("imaps", "Bo%C3%AEte/Clients%20%E5%8F%B0%E5%8C%97")).unwrap();
    assert_eq!(folder.folder, "Boîte/Clients 台北");
    let mut mailbox = Mailbox::open(&folder, Login::Password, &options(&server)).unwrap();
    assert_eq!(subjects(&mailbox.fetch(&[uid]).unwrap()), ["from Taipei"]);
    mailbox.move_to(uid, "Traité").unwrap();
    assert_eq!(server.messages("Traité").len(), 1);
    let sent = commands(&server);
    assert!(sent.contains(&"SELECT \"Bo&AO4-te/Clients &U,BTFw-\"".to_string()), "{sent:?}");
    assert!(sent.contains(&"UID MOVE 1 \"Trait&AOk-\"".to_string()), "{sent:?}");
    // a folder that does not exist
    let missing = MailboxUrl::parse(&server.url("imaps", "Archive")).unwrap();
    let e = Mailbox::open(&missing, Login::Password, &options(&server)).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Refused, "{e}");
    assert!(e.message().contains("`Archive`"), "{e}");
}

#[test]
fn fetches_in_bounded_batches_and_skips_what_is_too_large() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let mut big = message("big");
    big.extend(std::iter::repeat_n(b'x', 5000));
    for i in 1..=5 {
        if i == 3 {
            server.deliver("INBOX", &big);
        } else {
            server.deliver("INBOX", &message(&format!("m{i}")));
        }
    }
    let small = Options { max_message_size: 1000, batch: 2, ..options(&server) };
    let mut mailbox = Mailbox::open(&url(&server, "imaps", "INBOX"), Login::Password, &small).unwrap();
    let uids = mailbox.unseen().unwrap();
    let fetched = mailbox.fetch(&uids).unwrap();
    let big_size = big.len() as u64;
    assert_eq!(subjects(&fetched), ["m1", "m2", &format!("#3 too large ({big_size})"), "m4", "m5"]);
    let bodies: Vec<String> = commands(&server).into_iter().filter(|c| c.contains("BODY.PEEK")).collect();
    assert_eq!(bodies, ["UID FETCH 1,2 (UID BODY.PEEK[])", "UID FETCH 4,5 (UID BODY.PEEK[])"]);
    // a message gone is left out
    assert_eq!(subjects(&mailbox.fetch(&[2, 99]).unwrap()), ["m2"]);
    assert!(mailbox.fetch(&[]).unwrap().is_empty());
}

#[test]
fn a_broken_connection_is_opened_again_once() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("a"));
    let mut inbox = Inbox::new(url(&server, "imaps", "INBOX"), options(&server));
    assert_eq!(inbox.with(|m| m.unseen()).unwrap(), [1]);
    assert_eq!(server.connections(), 1);
    // the next command breaks the connection: a new one does the work
    server.drop_at(1);
    assert_eq!(inbox.with(|m| m.unseen()).unwrap(), [1]);
    assert_eq!(server.connections(), 2);
    // a failure that is not the connection's is not tried again
    let e = inbox.with(|m| m.move_to(1, "Nowhere")).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Refused);
    assert_eq!(server.connections(), 2);
    inbox.close();
    assert_eq!(commands(&server).last().unwrap(), "LOGOUT");
}

#[test]
fn a_connection_that_breaks_again_is_an_error() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    let mut inbox = Inbox::new(url(&server, "imaps", "INBOX"), options(&server));
    let mut attempts = 0;
    let e = inbox
        .with(|_| {
            attempts += 1;
            Err::<(), _>(grenat_imap::Error::new(ErrorKind::Connect, "broken"))
        })
        .unwrap_err();
    assert_eq!((e.kind(), attempts), (ErrorKind::Connect, 2));
    assert_eq!(server.connections(), 2);
}

#[test]
fn a_silent_server_times_out_and_is_reconnected() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("a"));
    let quick = Options { timeout: Duration::from_millis(300), ..options(&server) };
    let mut mailbox = Mailbox::open(&url(&server, "imaps", "INBOX"), Login::Password, &quick).unwrap();
    server.stall_at(1);
    let e = mailbox.unseen().unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Timeout, "{e}");
    assert!(e.retryable());
    // through an inbox, the work is done on a new connection
    let mut inbox = Inbox::new(url(&server, "imaps", "INBOX"), quick);
    inbox.with(|m| m.noop()).unwrap();
    server.stall_at(1);
    assert_eq!(inbox.with(|m| m.unseen()).unwrap(), [1]);
    assert_eq!(server.connections(), 3);
}

#[test]
fn a_recreated_folder_is_not_mistaken_for_the_old_one() {
    let server = FakeImap::start(Config::new(Mode::Implicit));
    server.deliver("INBOX", &message("a"));
    let mut inbox = Inbox::new(url(&server, "imaps", "INBOX"), options(&server));
    let (validity, uids) = inbox.with(|m| Ok((m.uid_validity(), m.unseen()?))).unwrap();
    assert_eq!(uids, [1]);
    inbox.with_uids(validity, |m| m.mark_flagged(1)).unwrap();
    inbox.close();
    server.recreate("INBOX");
    server.deliver("INBOX", &message("another message, the same UID"));
    let e = inbox.with_uids(validity, |m| m.mark_seen(1)).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::UidValidity, "{e}");
    assert!(e.message().contains("127.0.0.1/INBOX"), "{e}");
    assert!(!server.messages("INBOX")[0].has("\\Seen"));
}

#[test]
fn an_unreachable_server_is_a_connection_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let url = MailboxUrl::parse(&format!("imaps://u:s3cret@127.0.0.1:{port}/INBOX")).unwrap();
    let e = Mailbox::open(&url, Login::Password, &Options::default()).err().unwrap();
    assert_eq!(e.kind(), ErrorKind::Connect, "{e}");
    assert!(e.message().contains(&format!("127.0.0.1:{port}")) && !e.message().contains("s3cret"), "{e}");
}
