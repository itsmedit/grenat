//! Fetching in bounded batches: the sizes first (`RFC822.SIZE`, cheap),
//! then the messages within the cap, a few per `UID FETCH`, a batch never
//! holding more than the cap in total — so that memory stays bounded
//! whatever the folder holds, and a message too large is reported rather
//! than downloaded.

/// A message asked for, as the server gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// Its RFC 5322 bytes, left unseen (`BODY.PEEK[]`).
    Message { uid: u32, raw: Vec<u8> },
    /// Larger than the cap ([`crate::Options::max_message_size`]): not downloaded.
    TooLarge { uid: u32, size: u64 },
}

impl Fetched {
    pub fn uid(&self) -> u32 {
        match self {
            Fetched::Message { uid, .. } | Fetched::TooLarge { uid, .. } => *uid,
        }
    }
}

/// What to do with messages of known size (`(uid, size)`, in order): the
/// ones over `cap`, and the batches of the others — at most `per_batch`
/// messages and `cap` octets each, in the order given.
pub(crate) fn plan(sizes: &[(u32, u64)], cap: u64, per_batch: usize) -> (Vec<(u32, u64)>, Vec<Vec<u32>>) {
    let per_batch = per_batch.max(1);
    let mut too_large = Vec::new();
    let mut batches: Vec<Vec<u32>> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut total = 0u64;
    for &(uid, size) in sizes {
        if size > cap {
            too_large.push((uid, size));
            continue;
        }
        if !current.is_empty() && (current.len() >= per_batch || total + size > cap) {
            batches.push(std::mem::take(&mut current));
            total = 0;
        }
        current.push(uid);
        total += size;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    (too_large, batches)
}

/// `1,5,7`: a UID set.
pub(crate) fn uid_set(uids: &[u32]) -> String {
    uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_are_bounded_by_count_and_size() {
        let sizes = [(1, 10), (2, 10), (3, 10), (4, 95), (5, 101), (6, 5)];
        let (too_large, batches) = plan(&sizes, 100, 2);
        assert_eq!(too_large, [(5, 101)]);
        assert_eq!(batches, [vec![1, 2], vec![3], vec![4, 6]]);
        // a message of exactly the cap is fetched, alone
        assert_eq!(plan(&[(1, 1), (2, 100)], 100, 20).1, [vec![1], vec![2]]);
        assert_eq!(plan(&[], 100, 20), (vec![], vec![]));
        // a batch of zero means one
        assert_eq!(plan(&[(1, 1), (2, 1)], 100, 0).1, [vec![1], vec![2]]);
    }

    #[test]
    fn uid_sets() {
        assert_eq!(uid_set(&[1, 5, 7]), "1,5,7");
        assert_eq!(Fetched::TooLarge { uid: 3, size: 9 }.uid(), 3);
    }
}
