// Copyright (c) 2026, Michael Grier.

use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};

use crate::{
    branch::Branch,
    encoding::{EncodingError, EncodingValidator},
    green::GreenPool,
    line_buffer_view::LineBufferView,
    line_source::LineSource,
};

/// A top-level owned line buffer that owns the `GreenPool`, a root `Branch`,
/// and an optional encoding validator applied to all edits.
///
/// `LineBuffer` is the primary entry point for end-users of this crate.
/// Branches can be forked from it for independent edit histories that share
/// the same interned string pool.
pub struct LineBuffer {
    pub(crate) pool: Arc<Mutex<GreenPool>>,
    pub(crate) root: Branch,
    pub(crate) validator: Option<Arc<dyn EncodingValidator>>,
}

impl LineBuffer {
    /// Creates an empty `LineBuffer` with a fresh pool and empty root branch.
    ///
    /// If `validator` is `Some`, it will be applied to every `insert_line` and
    /// `replace_line` call.
    pub fn new(validator: Option<Arc<dyn EncodingValidator>>) -> LineBuffer {
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        let root = Branch::new(Arc::clone(&pool));
        LineBuffer {
            pool,
            root,
            validator,
        }
    }

    /// Bulk-loads all lines from `src` into a new buffer.
    ///
    /// Uses [`LineSource::line_count_hint`] to pre-allocate capacity.  Each
    /// line is validated (if a validator is supplied) and interned.  Returns
    /// `Err` on the first validation failure; no partial state is retained.
    pub fn from_source(
        src: &dyn LineSource,
        validator: Option<Arc<dyn EncodingValidator>>,
    ) -> Result<LineBuffer, EncodingError> {
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        let mut root = Branch::new(Arc::clone(&pool));
        if let Some(hint) = src.line_count_hint() {
            root.lines.reserve(hint);
        }
        let mut error: Option<EncodingError> = None;
        src.for_each_line(&mut |line| {
            if error.is_some() {
                return;
            }
            if let Some(v) = validator.as_deref() {
                if let Err(e) = v.validate(line) {
                    error = Some(e);
                    return;
                }
            }
            let id = pool.lock().unwrap().intern(line);
            root.lines.push(id);
        });
        if let Some(e) = error {
            return Err(e);
        }
        Ok(LineBuffer {
            pool,
            root,
            validator,
        })
    }

    /// Returns the number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.root.line_count()
    }

    /// Returns the content of line `n`, or `None` if `n >= line_count()`.
    ///
    /// Returns an `Arc<str>` sharing the pool's heap allocation — no bytes
    /// are copied.
    pub fn get_line(&self, n: usize) -> Option<Arc<str>> {
        self.root.get_line(n)
    }

    /// Inserts `content` at position `n`, shifting subsequent lines down.
    ///
    /// The buffer's installed validator (if any) is applied first.  Returns
    /// `Err` without modifying state if validation fails.  Inserting at
    /// `n == line_count()` appends.
    pub fn insert_line(&mut self, n: usize, content: &str) -> Result<(), EncodingError> {
        self.root.insert_line(n, content, self.validator.as_deref())
    }

    /// Replaces the content of line `n` with `content`.
    ///
    /// The buffer's installed validator (if any) is applied first.  Returns
    /// `Err` without modifying state if validation fails or `n >= line_count()`.
    pub fn replace_line(&mut self, n: usize, content: &str) -> Result<(), EncodingError> {
        self.root
            .replace_line(n, content, self.validator.as_deref())
    }

    /// Deletes line `n`.  Returns `false` without modifying state if
    /// `n >= line_count()`.
    pub fn delete_line(&mut self, n: usize) -> bool {
        self.root.delete_line(n)
    }

    /// Undoes the most recent edit.  Returns `false` if there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.root.undo()
    }

    /// Replays the most recently undone edit.  Returns `false` if there is
    /// nothing to redo.
    pub fn redo(&mut self) -> bool {
        self.root.redo()
    }

    /// Returns a new `Branch` forked from the root branch.
    ///
    /// The returned branch shares the same `GreenPool` but starts with its own
    /// copy of the current line sequence and empty undo/redo stacks.  No
    /// validator is installed on the branch — callers pass validators explicitly
    /// to `Branch::insert_line` / `Branch::replace_line` if needed.
    pub fn branch(&self) -> Branch {
        self.root.fork()
    }

    /// Writes each line as UTF-8 bytes followed by `line_ending` bytes.
    ///
    /// No trailing line ending is written after the last line.  Writing to an
    /// empty buffer produces no output.
    pub fn write_lines<W: Write>(&self, mut writer: W, line_ending: &str) -> io::Result<()> {
        let last = self.root.lines.len().saturating_sub(1);
        for (i, &id) in self.root.lines.iter().enumerate() {
            // Hold the pool lock only for the content write; release it before
            // writing the separator so we never hold it across two writes and
            // we avoid allocating a copy of the line bytes.
            {
                let pool = self.pool.lock().unwrap();
                writer.write_all(pool.get(id).content().as_bytes())?;
            }
            if i < last {
                writer.write_all(line_ending.as_bytes())?;
            }
        }
        Ok(())
    }

    /// Returns an iterator over the content of each line in order.
    ///
    /// Each item is an `Arc<str>` sharing the pool's heap allocation — no
    /// bytes are copied.  The pool lock is acquired and released per item.
    pub fn iter_lines(&self) -> impl Iterator<Item = Arc<str>> + '_ {
        self.root.lines.iter().map(|&id| {
            let pool = self.pool.lock().unwrap();
            pool.get(id).content_arc()
        })
    }

    /// Creates a `LineBuffer` by splitting `s` on LF, CRLF, or bare CR.
    ///
    /// Terminators are stripped.  A trailing newline does not produce a spurious
    /// empty final line.  Each line is validated (if a validator is supplied)
    /// and interned; returns `Err` on the first validation failure.
    pub fn from_str(
        s: &str,
        validator: Option<Arc<dyn EncodingValidator>>,
    ) -> Result<LineBuffer, EncodingError> {
        let lines = split_on_line_endings(s);
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        let mut root = Branch::new(Arc::clone(&pool));
        root.lines.reserve(lines.len());
        for line in lines {
            if let Some(v) = validator.as_deref() {
                v.validate(line)?;
            }
            let id = pool.lock().unwrap().intern(line);
            root.lines.push(id);
        }
        Ok(LineBuffer {
            pool,
            root,
            validator,
        })
    }

    /// Creates a `LineBuffer` by reading all bytes from `reader`, UTF-8 decoding
    /// them, then splitting and interning lines as [`LineBuffer::from_str`] does.
    ///
    /// A UTF-8 decode failure yields an `io::Error` with kind `InvalidData`.
    /// Validation failures are likewise wrapped as `InvalidData` errors.
    pub fn from_reader<R: Read>(
        mut reader: R,
        validator: Option<Arc<dyn EncodingValidator>>,
    ) -> io::Result<LineBuffer> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        let s = std::str::from_utf8(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        LineBuffer::from_str(s, validator)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

// ── LineBufferView for LineBuffer ─────────────────────────────────────────────

impl LineBufferView for LineBuffer {
    fn line_count(&self) -> usize {
        self.root.line_count()
    }

    fn get_line(&self, n: usize) -> Option<Arc<str>> {
        self.root.get_line(n)
    }

    fn line_crc32(&self, n: usize) -> Option<u32> {
        self.root.line_crc32(n)
    }

    fn line_md5(&self, n: usize) -> Option<[u8; 16]> {
        self.root.line_md5(n)
    }

    fn line_sha256(&self, n: usize) -> Option<[u8; 32]> {
        self.root.line_sha256(n)
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Splits `s` on LF (`\n`), CRLF (`\r\n`), or bare CR (`\r`), returning the
/// lines without their terminators.  A trailing newline does **not** produce a
/// spurious empty final element.
fn split_on_line_endings(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(&s[start..i]);
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                lines.push(&s[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    let tail = &s[start..];
    if !tail.is_empty() {
        lines.push(tail);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::{AsciiValidator, Utf8Validator};

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn buf() -> LineBuffer {
        LineBuffer::new(None)
    }

    fn ascii_buf() -> LineBuffer {
        LineBuffer::new(Some(Arc::new(AsciiValidator)))
    }

    fn write_to_vec(lb: &LineBuffer, line_ending: &str) -> Vec<u8> {
        let mut out = Vec::new();
        lb.write_lines(&mut out, line_ending).unwrap();
        out
    }

    // ── ML-27: empty buffer ──────────────────────────────────────────────────

    #[test]
    fn empty_buffer_has_line_count_zero() {
        assert_eq!(buf().line_count(), 0);
    }

    #[test]
    fn empty_buffer_get_line_returns_none() {
        assert!(buf().get_line(0).is_none());
    }

    // ── ML-27: insert + retrieve roundtrip ──────────────────────────────────

    #[test]
    fn insert_and_retrieve_single_line() {
        let mut lb = buf();
        lb.insert_line(0, "hello").unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn insert_multiple_lines_roundtrip() {
        let mut lb = buf();
        let lines = ["alpha", "beta", "gamma", "delta", "epsilon"];
        for (i, &s) in lines.iter().enumerate() {
            lb.insert_line(i, s).unwrap();
        }
        assert_eq!(lb.line_count(), lines.len());
        for (i, &expected) in lines.iter().enumerate() {
            assert_eq!(lb.get_line(i).as_deref(), Some(expected));
        }
    }

    #[test]
    fn insert_at_beginning_shifts_lines() {
        let mut lb = buf();
        lb.insert_line(0, "second").unwrap();
        lb.insert_line(0, "first").unwrap();
        assert_eq!(lb.get_line(0).as_deref(), Some("first"));
        assert_eq!(lb.get_line(1).as_deref(), Some("second"));
    }

    #[test]
    fn insert_append_at_line_count_appends() {
        let mut lb = buf();
        lb.insert_line(0, "a").unwrap();
        lb.insert_line(lb.line_count(), "b").unwrap();
        assert_eq!(lb.get_line(1).as_deref(), Some("b"));
    }

    #[test]
    fn replace_line_changes_content() {
        let mut lb = buf();
        lb.insert_line(0, "original").unwrap();
        lb.replace_line(0, "updated").unwrap();
        assert_eq!(lb.get_line(0).as_deref(), Some("updated"));
    }

    #[test]
    fn delete_line_removes_it() {
        let mut lb = buf();
        lb.insert_line(0, "keep").unwrap();
        lb.insert_line(1, "remove").unwrap();
        lb.delete_line(1);
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("keep"));
    }

    // ── ML-27: get_line out-of-bounds ────────────────────────────────────────

    #[test]
    fn get_line_out_of_bounds_returns_none() {
        let mut lb = buf();
        lb.insert_line(0, "only").unwrap();
        assert!(lb.get_line(1).is_none());
        assert!(lb.get_line(100).is_none());
    }

    // ── ML-27: validator rejects invalid content ─────────────────────────────

    #[test]
    fn validator_rejects_non_ascii_insert() {
        let mut lb = ascii_buf();
        let result = lb.insert_line(0, "café");
        assert!(result.is_err());
    }

    #[test]
    fn validator_rejects_non_ascii_replace() {
        let mut lb = ascii_buf();
        lb.insert_line(0, "ok").unwrap();
        let result = lb.replace_line(0, "日本語");
        assert!(result.is_err());
    }

    #[test]
    fn validator_accepts_ascii_content() {
        let mut lb = ascii_buf();
        lb.insert_line(0, "plain ascii").unwrap();
        assert_eq!(lb.get_line(0).as_deref(), Some("plain ascii"));
    }

    #[test]
    fn validator_utf8_accepts_unicode() {
        let mut lb = LineBuffer::new(Some(Arc::new(Utf8Validator)));
        lb.insert_line(0, "日本語").unwrap();
        assert_eq!(lb.get_line(0).as_deref(), Some("日本語"));
    }

    // ── ML-27: buffer state unchanged after rejection ────────────────────────

    #[test]
    fn buffer_unchanged_after_insert_rejection() {
        let mut lb = ascii_buf();
        lb.insert_line(0, "good").unwrap();
        let _ = lb.insert_line(1, "bäd");
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("good"));
    }

    #[test]
    fn buffer_unchanged_after_replace_rejection() {
        let mut lb = ascii_buf();
        lb.insert_line(0, "original").unwrap();
        let _ = lb.replace_line(0, "ñoño");
        assert_eq!(lb.get_line(0).as_deref(), Some("original"));
    }

    // ── ML-27: branch independence ────────────────────────────────────────────

    #[test]
    fn branch_from_buffer_is_edit_independent() {
        let mut lb = buf();
        lb.insert_line(0, "line1").unwrap();
        let mut br = lb.branch();
        br.insert_line(1, "branch-only", None).unwrap();
        // Buffer unchanged
        assert_eq!(lb.line_count(), 1);
        // Branch has extra line
        assert_eq!(br.line_count(), 2);
    }

    #[test]
    fn edit_to_buffer_does_not_affect_branch() {
        let mut lb = buf();
        lb.insert_line(0, "shared").unwrap();
        let br = lb.branch();
        lb.insert_line(1, "buffer-only").unwrap();
        assert_eq!(br.line_count(), 1);
        assert_eq!(lb.line_count(), 2);
    }

    // ── ML-27: branch shares pool ─────────────────────────────────────────────

    #[test]
    fn branch_from_buffer_shares_pool() {
        let mut lb = buf();
        lb.insert_line(0, "shared content").unwrap();
        let br = lb.branch();
        // Both refer to the same Arc<Mutex<GreenPool>> — verify via pointer equality
        assert!(Arc::ptr_eq(&lb.pool, &br.pool));
    }

    // ── ML-27: undo/redo through LineBuffer API ───────────────────────────────

    #[test]
    fn undo_after_insert_removes_line() {
        let mut lb = buf();
        lb.insert_line(0, "a").unwrap();
        lb.insert_line(1, "b").unwrap();
        lb.undo();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("a"));
    }

    #[test]
    fn redo_after_undo_replays_edit() {
        let mut lb = buf();
        lb.insert_line(0, "a").unwrap();
        lb.insert_line(1, "b").unwrap();
        lb.undo();
        lb.redo();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(1).as_deref(), Some("b"));
    }

    #[test]
    fn undo_on_empty_buffer_returns_false() {
        let mut lb = buf();
        assert!(!lb.undo());
    }

    #[test]
    fn redo_on_fresh_buffer_returns_false() {
        let mut lb = buf();
        assert!(!lb.redo());
    }

    #[test]
    fn undo_after_replace_restores_original() {
        let mut lb = buf();
        lb.insert_line(0, "original").unwrap();
        lb.replace_line(0, "updated").unwrap();
        lb.undo();
        assert_eq!(lb.get_line(0).as_deref(), Some("original"));
    }

    #[test]
    fn multi_step_undo_redo_sequence() {
        let mut lb = buf();
        for i in 0..10 {
            lb.insert_line(i, &format!("line{}", i)).unwrap();
        }
        assert_eq!(lb.line_count(), 10);
        for _ in 0..5 {
            lb.undo();
        }
        assert_eq!(lb.line_count(), 5);
        for _ in 0..3 {
            lb.redo();
        }
        assert_eq!(lb.line_count(), 8);
    }

    // ── ML-27: iter_lines ordering ────────────────────────────────────────────

    #[test]
    fn iter_lines_empty_buffer_yields_nothing() {
        let lb = buf();
        assert_eq!(lb.iter_lines().count(), 0);
    }

    #[test]
    fn iter_lines_returns_lines_in_order() {
        let mut lb = buf();
        let lines = ["first", "second", "third", "fourth", "fifth"];
        for (i, &s) in lines.iter().enumerate() {
            lb.insert_line(i, s).unwrap();
        }
        let collected: Vec<Arc<str>> = lb.iter_lines().collect();
        for (i, &expected) in lines.iter().enumerate() {
            assert_eq!(&*collected[i], expected);
        }
    }

    #[test]
    fn iter_lines_matches_get_line() {
        let mut lb = buf();
        let lines = ["alpha", "beta", "gamma"];
        for (i, &s) in lines.iter().enumerate() {
            lb.insert_line(i, s).unwrap();
        }
        let iter_collected: Vec<String> = lb.iter_lines().map(|s| s.to_string()).collect();
        let get_collected: Vec<String> = (0..lb.line_count())
            .map(|i| lb.get_line(i).unwrap().to_string())
            .collect();
        assert_eq!(iter_collected, get_collected);
    }

    // ── ML-27: write_lines with various line endings ──────────────────────────

    #[test]
    fn write_lines_empty_buffer_produces_empty_output() {
        let lb = buf();
        let out = write_to_vec(&lb, "\n");
        assert!(out.is_empty());
    }

    #[test]
    fn write_lines_single_line_no_trailing_newline() {
        let mut lb = buf();
        lb.insert_line(0, "only line").unwrap();
        let out = write_to_vec(&lb, "\n");
        assert_eq!(out, b"only line");
    }

    #[test]
    fn write_lines_with_lf() {
        let mut lb = buf();
        lb.insert_line(0, "alpha").unwrap();
        lb.insert_line(1, "beta").unwrap();
        lb.insert_line(2, "gamma").unwrap();
        let out = write_to_vec(&lb, "\n");
        assert_eq!(out, b"alpha\nbeta\ngamma");
    }

    #[test]
    fn write_lines_with_crlf() {
        let mut lb = buf();
        lb.insert_line(0, "alpha").unwrap();
        lb.insert_line(1, "beta").unwrap();
        lb.insert_line(2, "gamma").unwrap();
        let out = write_to_vec(&lb, "\r\n");
        assert_eq!(out, b"alpha\r\nbeta\r\ngamma");
    }

    #[test]
    fn write_lines_with_cr() {
        let mut lb = buf();
        lb.insert_line(0, "alpha").unwrap();
        lb.insert_line(1, "beta").unwrap();
        lb.insert_line(2, "gamma").unwrap();
        let out = write_to_vec(&lb, "\r");
        assert_eq!(out, b"alpha\rbeta\rgamma");
    }

    #[test]
    fn write_lines_two_lines_no_trailing_separator() {
        let mut lb = buf();
        lb.insert_line(0, "line1").unwrap();
        lb.insert_line(1, "line2").unwrap();
        let out = write_to_vec(&lb, "\n");
        assert_eq!(out, b"line1\nline2");
    }

    #[test]
    fn write_lines_unicode_content_utf8() {
        let mut lb = buf();
        lb.insert_line(0, "日本語").unwrap();
        lb.insert_line(1, "emoji 🦆").unwrap();
        let out = write_to_vec(&lb, "\n");
        assert_eq!(out, "日本語\nemoji 🦆".as_bytes());
    }

    #[test]
    fn write_lines_empty_string_lines() {
        let mut lb = buf();
        lb.insert_line(0, "").unwrap();
        lb.insert_line(1, "").unwrap();
        lb.insert_line(2, "").unwrap();
        let out = write_to_vec(&lb, "\n");
        assert_eq!(out, b"\n\n");
    }

    // ── Extra coverage ────────────────────────────────────────────────────────

    #[test]
    fn delete_out_of_bounds_returns_false() {
        let mut lb = buf();
        lb.insert_line(0, "line").unwrap();
        assert!(!lb.delete_line(5));
        assert_eq!(lb.line_count(), 1);
    }

    #[test]
    fn replace_out_of_bounds_returns_error() {
        let mut lb = buf();
        assert!(lb.replace_line(0, "content").is_err());
    }

    #[test]
    fn ten_normal_insert_retrieve_pairs() {
        let mut lb = buf();
        let data = [
            "apple",
            "banana",
            "cherry",
            "date",
            "elderberry",
            "fig",
            "grape",
            "honeydew",
            "kiwi",
            "lemon",
        ];
        for (i, &s) in data.iter().enumerate() {
            lb.insert_line(i, s).unwrap();
        }
        assert_eq!(lb.line_count(), 10);
        for (i, &expected) in data.iter().enumerate() {
            assert_eq!(lb.get_line(i).as_deref(), Some(expected));
        }
    }

    // ── ML-38: from_str ───────────────────────────────────────────────────────

    #[test]
    fn from_str_empty_string_gives_zero_lines() {
        let lb = LineBuffer::from_str("", None).unwrap();
        assert_eq!(lb.line_count(), 0);
    }

    #[test]
    fn from_str_single_line_no_newline() {
        let lb = LineBuffer::from_str("hello", None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn from_str_trailing_lf_not_spurious() {
        let lb = LineBuffer::from_str("hello\n", None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn from_str_trailing_crlf_not_spurious() {
        let lb = LineBuffer::from_str("hello\r\n", None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn from_str_trailing_cr_not_spurious() {
        let lb = LineBuffer::from_str("hello\r", None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn from_str_two_lines_lf() {
        let lb = LineBuffer::from_str("alpha\nbeta", None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("alpha"));
        assert_eq!(lb.get_line(1).as_deref(), Some("beta"));
    }

    #[test]
    fn from_str_two_lines_crlf() {
        let lb = LineBuffer::from_str("alpha\r\nbeta", None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("alpha"));
        assert_eq!(lb.get_line(1).as_deref(), Some("beta"));
    }

    #[test]
    fn from_str_two_lines_bare_cr() {
        let lb = LineBuffer::from_str("alpha\rbeta", None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("alpha"));
        assert_eq!(lb.get_line(1).as_deref(), Some("beta"));
    }

    #[test]
    fn from_str_mixed_endings() {
        let lb = LineBuffer::from_str("a\nb\r\nc\rd", None).unwrap();
        assert_eq!(lb.line_count(), 4);
        assert_eq!(lb.get_line(0).as_deref(), Some("a"));
        assert_eq!(lb.get_line(1).as_deref(), Some("b"));
        assert_eq!(lb.get_line(2).as_deref(), Some("c"));
        assert_eq!(lb.get_line(3).as_deref(), Some("d"));
    }

    #[test]
    fn from_str_unicode_content() {
        let lb = LineBuffer::from_str("日本語\n🦆 emoji", None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("日本語"));
        assert_eq!(lb.get_line(1).as_deref(), Some("🦆 emoji"));
    }

    #[test]
    fn from_str_empty_line_in_middle() {
        let lb = LineBuffer::from_str("a\n\nb", None).unwrap();
        assert_eq!(lb.line_count(), 3);
        assert_eq!(lb.get_line(0).as_deref(), Some("a"));
        assert_eq!(lb.get_line(1).as_deref(), Some(""));
        assert_eq!(lb.get_line(2).as_deref(), Some("b"));
    }

    #[test]
    fn from_str_only_lf_is_one_empty_line() {
        let lb = LineBuffer::from_str("\n", None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some(""));
    }

    #[test]
    fn from_str_two_consecutive_lf_no_content() {
        // "\n\n" = each \n terminates one empty line; trailing suppression only
        // applies to the empty tail *after* the last terminator, not to
        // intermediate empty lines.  Consistent with write_lines: 3 empty lines
        // round-trip as "\n\n".
        let lb = LineBuffer::from_str("\n\n", None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some(""));
        assert_eq!(lb.get_line(1).as_deref(), Some(""));
    }

    #[test]
    fn from_str_ten_lines_roundtrip() {
        let input = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten";
        let lb = LineBuffer::from_str(input, None).unwrap();
        assert_eq!(lb.line_count(), 10);
        let expected = [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ];
        for (i, &e) in expected.iter().enumerate() {
            assert_eq!(lb.get_line(i).as_deref(), Some(e));
        }
    }

    #[test]
    fn from_str_validator_accepts_ascii() {
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let lb = LineBuffer::from_str("hello\nworld", v).unwrap();
        assert_eq!(lb.line_count(), 2);
    }

    #[test]
    fn from_str_validator_rejects_first_line() {
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let result = LineBuffer::from_str("café\nworld", v);
        assert!(result.is_err());
    }

    #[test]
    fn from_str_validator_rejects_second_line() {
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let result = LineBuffer::from_str("ok\ncafé", v);
        assert!(result.is_err());
    }

    #[test]
    fn from_str_no_validator_accepts_any_utf8() {
        let lb = LineBuffer::from_str("αβγ\n中文\n🎵", None).unwrap();
        assert_eq!(lb.line_count(), 3);
    }

    #[test]
    fn from_str_content_survives_roundtrip_via_write_lines() {
        let input = "line1\nline2\nline3\n";
        let lb = LineBuffer::from_str(input, None).unwrap();
        let out = write_to_vec(&lb, "\n");
        assert_eq!(out, b"line1\nline2\nline3");
    }

    // ── ML-37: from_reader ────────────────────────────────────────────────────

    #[test]
    fn from_reader_empty_reader_gives_zero_lines() {
        let lb = LineBuffer::from_reader(std::io::Cursor::new(b"" as &[u8]), None).unwrap();
        assert_eq!(lb.line_count(), 0);
    }

    #[test]
    fn from_reader_single_line_no_newline() {
        let lb = LineBuffer::from_reader(std::io::Cursor::new(b"hello" as &[u8]), None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn from_reader_lf_separated_lines() {
        let lb = LineBuffer::from_reader(std::io::Cursor::new(b"a\nb\nc" as &[u8]), None).unwrap();
        assert_eq!(lb.line_count(), 3);
        assert_eq!(lb.get_line(0).as_deref(), Some("a"));
        assert_eq!(lb.get_line(2).as_deref(), Some("c"));
    }

    #[test]
    fn from_reader_crlf_separated_lines() {
        let lb = LineBuffer::from_reader(std::io::Cursor::new(b"x\r\ny" as &[u8]), None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("x"));
        assert_eq!(lb.get_line(1).as_deref(), Some("y"));
    }

    #[test]
    fn from_reader_bare_cr_separated_lines() {
        let lb = LineBuffer::from_reader(std::io::Cursor::new(b"p\rq" as &[u8]), None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("p"));
        assert_eq!(lb.get_line(1).as_deref(), Some("q"));
    }

    #[test]
    fn from_reader_trailing_newline_not_spurious() {
        let lb = LineBuffer::from_reader(std::io::Cursor::new(b"done\n" as &[u8]), None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("done"));
    }

    #[test]
    fn from_reader_unicode_utf8_content() {
        let data = "日本語\n🦆".as_bytes();
        let lb = LineBuffer::from_reader(std::io::Cursor::new(data), None).unwrap();
        assert_eq!(lb.line_count(), 2);
        assert_eq!(lb.get_line(0).as_deref(), Some("日本語"));
        assert_eq!(lb.get_line(1).as_deref(), Some("🦆"));
    }

    #[test]
    fn from_reader_invalid_utf8_yields_io_error() {
        let bad: &[u8] = &[0x80, 0x81, 0x82];
        let result = LineBuffer::from_reader(std::io::Cursor::new(bad), None);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn from_reader_validator_rejection_yields_io_error() {
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let data = "good\ncafé\n".as_bytes();
        let result = LineBuffer::from_reader(std::io::Cursor::new(data), v);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn from_reader_no_validator_accepts_unicode() {
        let data = "αβγ\n中文".as_bytes();
        let lb = LineBuffer::from_reader(std::io::Cursor::new(data), None).unwrap();
        assert_eq!(lb.line_count(), 2);
    }

    #[test]
    fn from_reader_mixed_endings() {
        let data = b"a\nb\r\nc\rd" as &[u8];
        let lb = LineBuffer::from_reader(std::io::Cursor::new(data), None).unwrap();
        assert_eq!(lb.line_count(), 4);
        let collected: Vec<String> = lb.iter_lines().map(|s| s.to_string()).collect();
        assert_eq!(collected, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn from_reader_ten_lines_roundtrip() {
        let input = b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten" as &[u8];
        let lb = LineBuffer::from_reader(std::io::Cursor::new(input), None).unwrap();
        assert_eq!(lb.line_count(), 10);
        let expected = [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ];
        for (i, &e) in expected.iter().enumerate() {
            assert_eq!(lb.get_line(i).as_deref(), Some(e));
        }
    }

    #[test]
    fn from_reader_matches_from_str() {
        let s = "hello\nworld\nfoo\n";
        let via_str = LineBuffer::from_str(s, None).unwrap();
        let via_reader = LineBuffer::from_reader(std::io::Cursor::new(s.as_bytes()), None).unwrap();
        let str_lines: Vec<String> = via_str.iter_lines().map(|l| l.to_string()).collect();
        let reader_lines: Vec<String> = via_reader.iter_lines().map(|l| l.to_string()).collect();
        assert_eq!(str_lines, reader_lines);
    }

    // ── ML-39: from_source mock tests ─────────────────────────────────────────

    struct MockSource {
        lines: Vec<String>,
        hint: Option<usize>,
        hint_calls: std::cell::Cell<usize>,
        each_calls: std::cell::Cell<usize>,
    }

    impl MockSource {
        fn new(lines: &[&str], hint: Option<usize>) -> MockSource {
            MockSource {
                lines: lines.iter().map(|s| s.to_string()).collect(),
                hint,
                hint_calls: std::cell::Cell::new(0),
                each_calls: std::cell::Cell::new(0),
            }
        }
    }

    impl LineSource for MockSource {
        fn line_count_hint(&self) -> Option<usize> {
            self.hint_calls.set(self.hint_calls.get() + 1);
            self.hint
        }

        fn for_each_line(&self, f: &mut dyn FnMut(&str)) {
            self.each_calls.set(self.each_calls.get() + 1);
            for line in &self.lines {
                f(line);
            }
        }
    }

    #[test]
    fn from_source_mock_empty_source_gives_zero_lines() {
        let src = MockSource::new(&[], None);
        let lb = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(lb.line_count(), 0);
    }

    #[test]
    fn from_source_mock_single_line() {
        let src = MockSource::new(&["hello"], None);
        let lb = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(lb.line_count(), 1);
        assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn from_source_mock_five_lines() {
        let data = ["alpha", "beta", "gamma", "delta", "epsilon"];
        let src = MockSource::new(&data, Some(5));
        let lb = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(lb.line_count(), 5);
        for (i, &expected) in data.iter().enumerate() {
            assert_eq!(lb.get_line(i).as_deref(), Some(expected));
        }
    }

    #[test]
    fn from_source_mock_ten_lines() {
        let data = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
        let src = MockSource::new(&data, Some(10));
        let lb = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(lb.line_count(), 10);
        for (i, &e) in data.iter().enumerate() {
            assert_eq!(lb.get_line(i).as_deref(), Some(e));
        }
    }

    #[test]
    fn from_source_mock_unicode_lines() {
        let data = ["日本語", "emoji 🦆", "中文"];
        let src = MockSource::new(&data, Some(3));
        let lb = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(lb.line_count(), 3);
        assert_eq!(lb.get_line(0).as_deref(), Some("日本語"));
        assert_eq!(lb.get_line(1).as_deref(), Some("emoji 🦆"));
        assert_eq!(lb.get_line(2).as_deref(), Some("中文"));
    }

    #[test]
    fn from_source_mock_validator_accepts_all_ascii() {
        let data = ["one", "two", "three", "four", "five"];
        let src = MockSource::new(&data, Some(5));
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let lb = LineBuffer::from_source(&src, v).unwrap();
        assert_eq!(lb.line_count(), 5);
    }

    #[test]
    fn from_source_validator_rejects_line_5_of_10() {
        // Lines 0-4 are ASCII; line 4 (index 4, the 5th) is non-ASCII.
        let data = ["a", "b", "c", "d", "café", "f", "g", "h", "i", "j"];
        let src = MockSource::new(&data, Some(10));
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let result = LineBuffer::from_source(&src, v);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(&*err.line, "café");
    }

    #[test]
    fn from_source_validator_rejects_first_line() {
        let data = ["ñoño", "fine"];
        let src = MockSource::new(&data, None);
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let result = LineBuffer::from_source(&src, v);
        assert!(result.is_err());
    }

    #[test]
    fn from_source_validator_rejects_last_line() {
        let data = ["ok", "also ok", "ñoño"];
        let src = MockSource::new(&data, None);
        let v: Option<Arc<dyn EncodingValidator>> = Some(Arc::new(AsciiValidator));
        let result = LineBuffer::from_source(&src, v);
        assert!(result.is_err());
    }

    #[test]
    fn from_source_line_count_hint_is_called() {
        let src = MockSource::new(&["a", "b", "c"], Some(3));
        let _ = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(
            src.hint_calls.get(),
            1,
            "line_count_hint must be called exactly once"
        );
    }

    #[test]
    fn from_source_for_each_line_is_called() {
        let src = MockSource::new(&["x", "y"], None);
        let _ = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(
            src.each_calls.get(),
            1,
            "for_each_line must be called exactly once"
        );
    }

    #[test]
    fn from_source_hint_none_still_works() {
        // No hint: capacity not pre-allocated, but should still work correctly.
        let data = ["p", "q", "r", "s", "t"];
        let src = MockSource::new(&data, None);
        let lb = LineBuffer::from_source(&src, None).unwrap();
        assert_eq!(lb.line_count(), 5);
    }
}
