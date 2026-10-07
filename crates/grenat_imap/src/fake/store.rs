//! The fake server's mail: folders (by their name on the wire), messages
//! with their UID and flags, and the commands received.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub uid: u32,
    pub flags: BTreeSet<String>,
    pub raw: Vec<u8>,
}

impl Stored {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f.eq_ignore_ascii_case(flag))
    }
}

pub(crate) struct Folder {
    pub uid_validity: u32,
    pub next_uid: u32,
    pub messages: Vec<Stored>,
}

impl Folder {
    pub(crate) fn new(uid_validity: u32) -> Folder {
        Folder { uid_validity, next_uid: 1, messages: Vec::new() }
    }

    pub(crate) fn add(&mut self, raw: Vec<u8>, flags: BTreeSet<String>) -> u32 {
        let uid = self.next_uid;
        self.next_uid += 1;
        self.messages.push(Stored { uid, flags, raw });
        uid
    }

    pub(crate) fn uids(&self) -> Vec<u32> {
        self.messages.iter().map(|m| m.uid).collect()
    }

    /// The message's sequence number (1-based) and the message.
    pub(crate) fn find(&mut self, uid: u32) -> Option<(usize, &mut Stored)> {
        self.messages.iter_mut().enumerate().find(|(_, m)| m.uid == uid).map(|(i, m)| (i + 1, m))
    }

    /// Removes the message; its sequence number before, for `EXPUNGE`.
    pub(crate) fn remove(&mut self, uid: u32) -> Option<(usize, Stored)> {
        let i = self.messages.iter().position(|m| m.uid == uid)?;
        Some((i + 1, self.messages.remove(i)))
    }
}

/// A connection that misbehaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Closed, unanswered.
    Drop,
    /// Left open, unanswered.
    Stall,
}

#[derive(Default)]
pub(crate) struct State {
    pub folders: BTreeMap<String, Folder>,
    /// Each command line received (without its tag), and whether it came over TLS.
    pub commands: Vec<(bool, String)>,
    /// Connections accepted so far.
    pub connections: usize,
    /// What befalls the connection when this many more commands have come (once).
    pub fault: Option<(usize, Fault)>,
    pub next_validity: u32,
}

impl State {
    pub(crate) fn folder(&mut self, name: &str) -> &mut Folder {
        if !self.folders.contains_key(name) {
            self.next_validity += 1;
            let validity = 1000 + self.next_validity;
            self.folders.insert(name.to_string(), Folder::new(validity));
        }
        self.folders.get_mut(name).expect("inserted above")
    }
}
