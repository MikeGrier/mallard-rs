// Copyright (c) 2026, Michael Grier.

//! The [`LineBufferView`] trait — a read-only window into a line sequence.
//!
//! `LineBufferView` provides a uniform interface for diff, merge, and search
//! algorithms that need to inspect line content and content hashes without
//! holding a mutable reference.  Any type that holds an ordered sequence of
//! interned lines can implement this trait; the two primary implementors are
//! [`crate::Branch`] and (via delegation) [`crate::LineBuffer`].
//!
//! The hash accessors (`line_crc32`, `line_md5`, `line_sha256`) return `None`
//! for out-of-bounds indices so that callers do not need separate bounds checks
//! when iterating over two sequences of different lengths.

use std::sync::Arc;

/// A read-only view of an ordered sequence of lines with content-hash access.
///
/// Implementors expose line content and precomputed hashes (CRC32, MD5,
/// SHA-256) without requiring mutable access.  Hash values are derived from
/// the interned [`crate::green::GreenLine`] entries and are computed lazily
/// and cached on first access.
///
/// # Intended use
///
/// `LineBufferView` is the primary interface for algorithms that operate on
/// line sequences without modifying them — for example:
///
/// - **Diff** algorithms that compare two sequences line-by-line using cheap
///   CRC32 equality tests before falling back to full content comparison.
/// - **Merge** algorithms that need read access to a base and two derived
///   sequences simultaneously.
/// - **Search** algorithms that hash lines to build an index over a buffer.
///
/// The trait is object-safe so that implementations can be passed as
/// `&dyn LineBufferView` across module boundaries.
pub trait LineBufferView {
    /// Returns the number of lines in the sequence.
    fn line_count(&self) -> usize;

    /// Returns the content of line `n` (0-based), or `None` if `n >= line_count()`.
    ///
    /// Returns an `Arc<str>` that shares the pool's heap allocation — no bytes
    /// are copied.  The caller may hold the handle indefinitely; the string
    /// data remains alive as long as any `Arc` to it exists.
    fn get_line(&self, n: usize) -> Option<Arc<str>>;

    /// Returns the CRC32 of line `n`, or `None` if `n >= line_count()`.
    fn line_crc32(&self, n: usize) -> Option<u32>;

    /// Returns the MD5 digest of line `n`, or `None` if `n >= line_count()`.
    fn line_md5(&self, n: usize) -> Option<[u8; 16]>;

    /// Returns the SHA-256 digest of line `n`, or `None` if `n >= line_count()`.
    fn line_sha256(&self, n: usize) -> Option<[u8; 32]>;
}
