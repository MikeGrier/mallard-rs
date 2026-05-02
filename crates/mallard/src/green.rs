// Copyright (c) 2026, Michael Grier.

//! Core primitives for the green (immutable, shared) side of mallard's
//! green/red versioning model.
//!
//! A `GreenPool` interns line content as `GreenLine` values identified by
//! opaque `GreenLineId` indices. The pool is append-only; entries are never
//! removed or modified once inserted.

use std::{cell::Cell, collections::HashMap, sync::Arc};

use md5::Md5;
use sha2::{Digest, Sha256};

// ── GreenLineId ───────────────────────────────────────────────────────────────

/// Opaque index into a [`GreenPool`].
///
/// `GreenLineId` values are meaningful only within the pool that produced them.
/// Using an id from one pool to index into a different pool is undefined
/// behaviour and will either panic or silently return the wrong line.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct GreenLineId(u32);

// ── LineHash ──────────────────────────────────────────────────────────────────

/// Content hashes for a single interned line.
///
/// `crc32` is computed eagerly at construction time. `md5` and `sha256` are
/// computed on first access and cached via interior mutability so that callers
/// pay only for the digests they actually need.
pub struct LineHash {
    crc32: u32,
    md5: Cell<Option<[u8; 16]>>,
    sha256: Cell<Option<[u8; 32]>>,
}

impl LineHash {
    pub(crate) fn new(content: &str) -> LineHash {
        let crc32 = crc32fast::hash(content.as_bytes());
        LineHash {
            crc32,
            md5: Cell::new(None),
            sha256: Cell::new(None),
        }
    }

    /// Returns the CRC32 of the line content. Always available without
    /// additional computation.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    // Internal helpers called by GreenLine, which owns the content.
    // MD5 and SHA-256 are not exposed directly on LineHash because computing
    // them requires the line content, which is owned by GreenLine.  Call
    // GreenLine::md5() and GreenLine::sha256() instead.
    pub(crate) fn md5_with_content(&self, content: &str) -> [u8; 16] {
        if let Some(v) = self.md5.get() {
            return v;
        }
        let digest: [u8; 16] = Md5::digest(content.as_bytes()).into();
        self.md5.set(Some(digest));
        digest
    }

    pub(crate) fn sha256_with_content(&self, content: &str) -> [u8; 32] {
        if let Some(v) = self.sha256.get() {
            return v;
        }
        let digest: [u8; 32] = Sha256::digest(content.as_bytes()).into();
        self.sha256.set(Some(digest));
        digest
    }
}

// ── GreenLine ─────────────────────────────────────────────────────────────────

/// An immutable, interned line of text together with its content hashes.
pub struct GreenLine {
    content: Arc<str>,
    hash: LineHash,
}

impl GreenLine {
    /// Creates a new `GreenLine`, computing the CRC32 hash eagerly.
    pub fn new(s: &str) -> GreenLine {
        let content: Arc<str> = Arc::from(s);
        let hash = LineHash::new(&content);
        GreenLine { content, hash }
    }

    /// Returns the line content as a `&str`.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns a shared handle to the interned content.
    ///
    /// This is a zero-cost atomic reference-count increment — no bytes are
    /// copied.  The returned `Arc<str>` shares the same heap allocation that
    /// the pool already owns.  Changing any value in the pool is a breaking
    /// change; this accessor is the preferred way to expose content to callers
    /// that need to hold a reference beyond the lock scope.
    pub fn content_arc(&self) -> Arc<str> {
        Arc::clone(&self.content)
    }

    /// Returns a reference to the content hashes for this line.
    ///
    /// Use `line.hashes().crc32()` for the free CRC32.  Call `md5()` and
    /// `sha256()` via the forwarding methods on `GreenLine` itself (not on the
    /// returned `LineHash`) so that the content is available for lazy
    /// computation.
    pub fn hashes(&self) -> &LineHash {
        &self.hash
    }

    /// Returns the CRC32 of this line's content.
    pub fn crc32(&self) -> u32 {
        self.hash.crc32()
    }

    /// Returns the MD5 digest, computing and caching it on first call.
    pub fn md5(&self) -> [u8; 16] {
        self.hash.md5_with_content(&self.content)
    }

    /// Returns the SHA-256 digest, computing and caching it on first call.
    pub fn sha256(&self) -> [u8; 32] {
        self.hash.sha256_with_content(&self.content)
    }
}

// ── GreenPool ─────────────────────────────────────────────────────────────────

/// Append-only interned store of [`GreenLine`] values.
///
/// Interning the same content twice returns the same [`GreenLineId`]. The pool
/// never removes entries; `GreenLineId` values remain stable for the lifetime
/// of the pool.
pub struct GreenPool {
    lines: Vec<GreenLine>,
    /// Keyed by `Arc<str>` so the pool shares a single heap allocation between
    /// the `GreenLine` entry and the look-up index; `Arc<str>: Borrow<str>`
    /// allows queries via `&str` without additional allocation.
    index: HashMap<Arc<str>, GreenLineId>,
}

impl GreenPool {
    /// Creates a new, empty `GreenPool`.
    pub fn new() -> GreenPool {
        GreenPool {
            lines: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Interns `s`, returning an existing `GreenLineId` if the content has
    /// been seen before, or inserting a new `GreenLine` and returning its id.
    ///
    /// Each unique string is heap-allocated exactly once; the `GreenLine` entry
    /// and the look-up index share the same `Arc<str>` allocation via a cheap
    /// atomic reference-count increment.
    pub fn intern(&mut self, s: &str) -> GreenLineId {
        if let Some(&id) = self.index.get(s) {
            return id;
        }
        let id = GreenLineId(self.lines.len() as u32);
        // One heap allocation for the string data; both the GreenLine and the
        // index key share it via Arc, so no second copy is made.
        let content: Arc<str> = Arc::from(s);
        let hash = LineHash::new(&content);
        self.index.insert(Arc::clone(&content), id); // atomic refcount only
        self.lines.push(GreenLine { content, hash });
        id
    }

    /// Returns a reference to the `GreenLine` at the given id.
    ///
    /// # Panics
    /// Panics if `id` was produced by a different `GreenPool`.
    pub fn get(&self, id: GreenLineId) -> &GreenLine {
        &self.lines[id.0 as usize]
    }

    /// Returns the number of distinct lines currently in the pool.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Returns `true` if the pool contains no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

impl Default for GreenPool {
    fn default() -> Self {
        GreenPool::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ML-6: Milestone 1 unit tests ─────────────────────────────────────────

    #[test]
    fn intern_same_content_returns_same_id() {
        let mut pool = GreenPool::new();
        let a = pool.intern("hello");
        let b = pool.intern("hello");
        assert_eq!(a, b);
    }

    #[test]
    fn intern_distinct_content_returns_different_ids() {
        let mut pool = GreenPool::new();
        let a = pool.intern("foo");
        let b = pool.intern("bar");
        assert_ne!(a, b);
    }

    #[test]
    fn crc32_known_value() {
        // CRC32 of "hello" is 0x3610a686 (verified against crc32fast).
        let line = GreenLine::new("hello");
        assert_eq!(line.crc32(), crc32fast::hash(b"hello"));
    }

    #[test]
    fn crc32_known_reference_value() {
        // Cross-check against the well-known CRC32/ISO-HDLC value for "123456789".
        let line = GreenLine::new("123456789");
        assert_eq!(line.crc32(), 0xCBF4_3926);
    }

    #[test]
    fn md5_correct_and_cached() {
        let line = GreenLine::new("hello");
        let first = line.md5();
        let second = line.md5();
        // Both calls must return the same bytes.
        assert_eq!(first, second);
        // Known MD5 of "hello": 5d41402abc4b2a76b9719d911017c592
        let expected: [u8; 16] = [
            0x5d, 0x41, 0x40, 0x2a, 0xbc, 0x4b, 0x2a, 0x76, 0xb9, 0x71, 0x9d, 0x91, 0x10, 0x17,
            0xc5, 0x92,
        ];
        assert_eq!(first, expected);
    }

    #[test]
    fn sha256_correct_and_cached() {
        let line = GreenLine::new("hello");
        let first = line.sha256();
        let second = line.sha256();
        assert_eq!(first, second);
        // Known SHA-256 of "hello":
        // 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let expected: [u8; 32] = [
            0x2c, 0xf2, 0x4d, 0xba, 0x5f, 0xb0, 0xa3, 0x0e, 0x26, 0xe8, 0x3b, 0x2a, 0xc5, 0xb9,
            0xe2, 0x9e, 0x1b, 0x16, 0x1e, 0x5c, 0x1f, 0xa7, 0x42, 0x5e, 0x73, 0x04, 0x33, 0x62,
            0x93, 0x8b, 0x98, 0x24,
        ];
        assert_eq!(first, expected);
    }

    #[test]
    fn pool_500_diverse_strings_all_unique_ids() {
        let mut pool = GreenPool::new();
        let mut ids = Vec::with_capacity(512);
        for i in 0u32..512 {
            let s = format!("line-content-{}", i);
            ids.push(pool.intern(&s));
        }
        // All IDs must be distinct.
        let unique: std::collections::HashSet<GreenLineId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 512);
    }

    #[test]
    fn intern_empty_string() {
        let mut pool = GreenPool::new();
        let id = pool.intern("");
        assert_eq!(pool.get(id).content(), "");
        // Idempotent.
        let id2 = pool.intern("");
        assert_eq!(id, id2);
    }

    #[test]
    fn intern_unicode_emoji() {
        let mut pool = GreenPool::new();
        let id = pool.intern("Hello 🦆🦆🦆");
        assert_eq!(pool.get(id).content(), "Hello 🦆🦆🦆");
    }

    #[test]
    fn intern_unicode_cjk() {
        let mut pool = GreenPool::new();
        let id = pool.intern("日本語テスト");
        assert_eq!(pool.get(id).content(), "日本語テスト");
    }

    #[test]
    fn intern_unicode_rtl() {
        let mut pool = GreenPool::new();
        let id = pool.intern("مرحبا بالعالم");
        assert_eq!(pool.get(id).content(), "مرحبا بالعالم");
    }

    #[test]
    fn intern_unicode_combining_chars() {
        // Combining diacritics: e + combining acute accent = é (two code points)
        let id = GreenPool::new().intern("e\u{0301}");
        let _ = id; // Just ensure it doesn't panic.
    }

    #[test]
    fn intern_line_exceeding_64kb() {
        let big: String = "x".repeat(70_000);
        let mut pool = GreenPool::new();
        let id = pool.intern(&big);
        assert_eq!(pool.get(id).content().len(), 70_000);
    }

    #[test]
    fn get_returns_exact_original_content() {
        let mut pool = GreenPool::new();
        let s = "  leading and trailing spaces  ";
        let id = pool.intern(s);
        assert_eq!(pool.get(id).content(), s);
    }

    #[test]
    fn intern_is_idempotent_under_repeated_calls() {
        let mut pool = GreenPool::new();
        let s = "repeat me";
        let ids: Vec<GreenLineId> = (0..20).map(|_| pool.intern(s)).collect();
        assert!(ids.iter().all(|&id| id == ids[0]));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn crc32_different_for_different_content() {
        let a = GreenLine::new("apple");
        let b = GreenLine::new("orange");
        assert_ne!(a.crc32(), b.crc32());
    }

    #[test]
    fn md5_different_for_different_content() {
        let a = GreenLine::new("apple");
        let b = GreenLine::new("orange");
        assert_ne!(a.md5(), b.md5());
    }

    #[test]
    fn sha256_different_for_different_content() {
        let a = GreenLine::new("apple");
        let b = GreenLine::new("orange");
        assert_ne!(a.sha256(), b.sha256());
    }

    #[test]
    fn pool_is_empty_initially() {
        let pool = GreenPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn pool_len_grows_correctly() {
        let mut pool = GreenPool::new();
        pool.intern("a");
        pool.intern("b");
        pool.intern("a"); // duplicate — should not grow
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn md5_empty_string() {
        // MD5("") = d41d8cd98f00b204e9800998ecf8427e
        let line = GreenLine::new("");
        let expected: [u8; 16] = [
            0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
            0x42, 0x7e,
        ];
        assert_eq!(line.md5(), expected);
    }

    #[test]
    fn sha256_empty_string() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let line = GreenLine::new("");
        let expected: [u8; 32] = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        assert_eq!(line.sha256(), expected);
    }
}
