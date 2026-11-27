//! Encoded pictures kept across conversions (`ConvertOptions::picture_cache`).
//!
//! Decoding, resampling and encoding pictures is most of the time a
//! conversion with large pictures takes. Converting an edited document again
//! finds the same pictures with the same settings: the cache hands back what
//! the last conversion encoded. Entries are keyed by the picture's content
//! and every setting that changes its encoding, hold encoded data only (never
//! the source), and are bounded by size, least recently used first out.
//! One cache serves every thread; only conversions that turn it on use it.
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::{Arc, Mutex};

use super::images::RgbEncoding;
use crate::compression::Compression;
use crate::fast_hash::FxHasher;

/// Encoded data kept at most (64 MiB).
const BUDGET: usize = 64 << 20;

/// A picture encoded for the PDF.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Encoded {
    /// A JPEG source encoded again.
    Jpeg {
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// A black-and-white picture at 1 bit per sample, Flate.
    Bilevel {
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// RGB as JPEG or Flate, with its Flate alpha when not opaque.
    Rgb {
        jpeg: bool,
        data: Vec<u8>,
        width: u32,
        height: u32,
        alpha: Option<Vec<u8>>,
    },
    /// A source that could not be decoded (or encoded).
    Undecodable,
}

impl Encoded {
    fn size(&self) -> usize {
        match self {
            Encoded::Jpeg { data, .. } | Encoded::Bilevel { data, .. } => data.len(),
            Encoded::Rgb { data, alpha, .. } => data.len() + alpha.as_ref().map_or(0, Vec::len),
            Encoded::Undecodable => 0,
        }
    }
}

/// A picture's content: its length and two independent 64-bit hashes of
/// its bytes (SipHash and Fx).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct Digest {
    len: usize,
    sip: u64,
    fx: u64,
}

impl Digest {
    pub(super) fn of(data: &[u8]) -> Digest {
        let mut sip = std::hash::DefaultHasher::new();
        sip.write(data);
        let mut fx = FxHasher::default();
        fx.write(data);
        Digest {
            len: data.len(),
            sip: sip.finish(),
            fx: fx.finish(),
        }
    }
}

/// Everything a picture's encoding depends on.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct PictureKey {
    pub(super) digest: Digest,
    pub(super) format: u8,
    pub(super) target: Option<(u32, u32)>,
    pub(super) soft_edge: Option<(u32, u32)>,
    pub(super) lum: Option<(u32, u32)>,
    pub(super) recolor: Option<([u8; 3], [u8; 3])>,
    pub(super) alpha: Option<u32>,
    pub(super) fast_filter: bool,
    pub(super) rgb: RgbEncoding,
    pub(super) compression: Compression,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<PictureKey, (Arc<Encoded>, u64)>,
    bytes: usize,
    clock: u64,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The picture `key` names, encoded once by `encode` and then kept.
pub(super) fn get_or_insert(key: PictureKey, encode: impl FnOnce() -> Encoded) -> Arc<Encoded> {
    {
        let mut guard = CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cache = guard.get_or_insert_with(Cache::default);
        cache.clock += 1;
        let now = cache.clock;
        if let Some((encoded, used)) = cache.entries.get_mut(&key) {
            *used = now;
            return Arc::clone(encoded);
        }
    }
    // Encoded outside the lock: other threads keep converting meanwhile.
    let encoded = Arc::new(encode());
    let size = encoded.size();
    if size > BUDGET {
        return encoded;
    }
    let mut guard = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cache = guard.get_or_insert_with(Cache::default);
    while cache.bytes + size > BUDGET {
        let Some(oldest) = cache
            .entries
            .iter()
            .min_by_key(|(_, (_, used))| *used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        if let Some((evicted, _)) = cache.entries.remove(&oldest) {
            cache.bytes -= evicted.size();
        }
    }
    cache.clock += 1;
    let now = cache.clock;
    if let Some((previous, _)) = cache.entries.insert(key, (Arc::clone(&encoded), now)) {
        cache.bytes -= previous.size();
    }
    cache.bytes += size;
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> PictureKey {
        PictureKey {
            digest: Digest::of(&[byte; 16]),
            format: 1,
            target: None,
            soft_edge: None,
            lum: None,
            recolor: None,
            alpha: None,
            fast_filter: false,
            rgb: RgbEncoding::Smaller,
            compression: Compression::Smallest,
        }
    }

    #[test]
    fn encodes_once_per_key_and_tells_contents_apart() {
        let encode = |data: Vec<u8>| Encoded::Jpeg {
            data,
            width: 1,
            height: 1,
        };
        let mut calls = 0;
        let first = get_or_insert(key(200), || {
            calls += 1;
            encode(vec![1])
        });
        let again = get_or_insert(key(200), || {
            calls += 1;
            encode(vec![2])
        });
        assert_eq!(calls, 1);
        assert!(Arc::ptr_eq(&first, &again));
        let other = get_or_insert(key(201), || encode(vec![3]));
        assert_eq!(*other, encode(vec![3]));
        assert_ne!(Digest::of(&[1, 2]), Digest::of(&[2, 1]));
    }
}
