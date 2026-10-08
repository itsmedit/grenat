//! Trace and span identifiers (W3C Trace Context): 16 and 8 random bytes,
//! never all zero, written as lowercase hex.
//!
//! The bytes come from a SplitMix64 sequence seeded once per process by
//! the standard library's random hasher keys and the clock: unique and
//! unpredictable enough for tracing, which needs no cryptography.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// A trace's identifier: 16 bytes, not all zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId(pub [u8; 16]);

/// A span's identifier: 8 bytes, not all zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId(pub [u8; 8]);

impl TraceId {
    pub fn random() -> TraceId {
        loop {
            let mut bytes = [0; 16];
            bytes[..8].copy_from_slice(&next().to_be_bytes());
            bytes[8..].copy_from_slice(&next().to_be_bytes());
            if bytes != [0; 16] {
                return TraceId(bytes);
            }
        }
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// 32 lowercase hex digits, not all zero.
    pub fn parse(text: &str) -> Option<TraceId> {
        let bytes: [u8; 16] = unhex(text)?.try_into().ok()?;
        (bytes != [0; 16]).then_some(TraceId(bytes))
    }
}

impl SpanId {
    pub fn random() -> SpanId {
        loop {
            let bytes = next().to_be_bytes();
            if bytes != [0; 8] {
                return SpanId(bytes);
            }
        }
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// 16 lowercase hex digits, not all zero.
    pub fn parse(text: &str) -> Option<SpanId> {
        let bytes: [u8; 8] = unhex(text)?.try_into().ok()?;
        (bytes != [0; 8]).then_some(SpanId(bytes))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase hex only, as W3C Trace Context requires.
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

/// The next number of the process's sequence.
fn next() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = *SEED.get_or_init(|| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),
        );
        hasher.write_u32(std::process::id());
        hasher.finish()
    });
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    splitmix64(seed.wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_hex_unique_and_never_zero() {
        let a = TraceId::random();
        let b = TraceId::random();
        assert_ne!(a, b);
        assert_eq!(a.hex().len(), 32);
        assert_eq!(TraceId::parse(&a.hex()), Some(a));
        let s = SpanId::random();
        assert_eq!(s.hex().len(), 16);
        assert_eq!(SpanId::parse(&s.hex()), Some(s));
        let many: std::collections::HashSet<_> = (0..10_000).map(|_| SpanId::random()).collect();
        assert_eq!(many.len(), 10_000);
    }

    #[test]
    fn parsing_refuses_zero_uppercase_and_odd_lengths() {
        assert_eq!(TraceId::parse(&"0".repeat(32)), None);
        assert_eq!(SpanId::parse(&"0".repeat(16)), None);
        assert_eq!(SpanId::parse("00F067AA0BA902B7"), None);
        assert_eq!(SpanId::parse("00f067aa0ba902b"), None);
        assert!(SpanId::parse("00f067aa0ba902b7").is_some());
    }
}
