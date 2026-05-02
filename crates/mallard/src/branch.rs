// Copyright (c) 2026, Michael Grier.

//! Branch — a versioned, editable sequence of interned lines.
//!
//! A `Branch` holds a `Vec<GreenLineId>` representing the current ordered line
//! sequence, together with undo and redo stacks expressed as [`BranchEdit`]
//! values.  Multiple branches may share one [`GreenPool`] via
//! `Arc<Mutex<GreenPool>>`; pool interning ensures identical content is stored
//! only once across all branches.
//!
//! Edit operations (insert, replace, delete) are added in Milestones ML-14
//! through ML-19.

use std::sync::{Arc, Mutex};

use crate::{
    encoding::{EncodingError, EncodingValidator},
    green::{GreenLineId, GreenPool},
};

// ── BranchEdit discriminant constants ────────────────────────────────────────
//
// BREAKING CHANGE WARNING: changing any value here is a breaking change to
// the serialised undo/redo history format.  Do not renumber.

mod edit_tag {
    pub const INSERT: u8 = 0;
    pub const DELETE: u8 = 1;
    pub const REPLACE: u8 = 2;
}

// ── BranchEdit ────────────────────────────────────────────────────────────────

/// A single reversible edit applied to a [`Branch`].
///
/// The discriminant values are fixed protocol constants; changing any of them
/// is a breaking change to any persistent undo-history format.
#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchEdit {
    /// A line was inserted at position `at`.
    Insert { at: usize, id: GreenLineId } = edit_tag::INSERT,

    /// The line at position `at` was deleted (its id is stored for undo).
    Delete { at: usize, id: GreenLineId } = edit_tag::DELETE,

    /// The line at position `at` was replaced.
    Replace {
        at: usize,
        old_id: GreenLineId,
        new_id: GreenLineId,
    } = edit_tag::REPLACE,
}

// ── Branch ────────────────────────────────────────────────────────────────────

/// A versioned, editable sequence of interned lines backed by a shared
/// [`GreenPool`].
///
/// Branches may be forked cheaply (cloning the `lines` vec, sharing the pool).
/// Each branch tracks its own independent undo/redo history.
pub struct Branch {
    pub(crate) pool: Arc<Mutex<GreenPool>>,
    pub(crate) lines: Vec<GreenLineId>,
    pub(crate) undo_stack: Vec<BranchEdit>,
    pub(crate) redo_stack: Vec<BranchEdit>,
}

impl Branch {
    /// Creates a new, empty `Branch` backed by the given pool.
    pub fn new(pool: Arc<Mutex<GreenPool>>) -> Branch {
        Branch {
            pool,
            lines: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Returns the number of lines currently in this branch.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Returns the content of line `n` (0-based), or `None` if `n` is
    /// out of range.
    ///
    /// Returns an `Arc<str>` sharing the pool's heap allocation — no bytes
    /// are copied.  Acquires the pool lock only for the duration of the
    /// `Arc::clone`.
    pub fn get_line(&self, n: usize) -> Option<Arc<str>> {
        let id = *self.lines.get(n)?;
        let pool = self.pool.lock().expect("GreenPool lock poisoned");
        Some(pool.get(id).content_arc())
    }

    /// Inserts `content` as a new line at position `n` (0-based).
    ///
    /// If `validator` is `Some`, validation is performed first.  On rejection
    /// the branch state is unchanged and the error is returned.  Inserting at
    /// `n == line_count()` appends to the end.
    ///
    /// # Errors
    /// Returns `Err` if the validator rejects `content` or if `n > line_count()`.
    pub fn insert_line(
        &mut self,
        n: usize,
        content: &str,
        validator: Option<&dyn EncodingValidator>,
    ) -> Result<(), EncodingError> {
        if n > self.lines.len() {
            return Err(EncodingError::new(
                content,
                format!(
                    "insert_line: index {} out of range (line_count = {})",
                    n,
                    self.lines.len()
                ),
            ));
        }
        if let Some(v) = validator {
            v.validate(content)?;
        }
        let id = self
            .pool
            .lock()
            .expect("GreenPool lock poisoned")
            .intern(content);
        self.lines.insert(n, id);
        self.undo_stack.push(BranchEdit::Insert { at: n, id });
        self.redo_stack.clear();
        Ok(())
    }

    /// Replaces the content of line `n` (0-based) with `content`.
    ///
    /// If `validator` is `Some`, validation is performed first.  On rejection
    /// or if `n >= line_count()` the branch state is unchanged and `Err` is
    /// returned.
    pub fn replace_line(
        &mut self,
        n: usize,
        content: &str,
        validator: Option<&dyn EncodingValidator>,
    ) -> Result<(), EncodingError> {
        if n >= self.lines.len() {
            return Err(EncodingError::new(
                content,
                format!(
                    "replace_line: index {} out of range (line_count = {})",
                    n,
                    self.lines.len()
                ),
            ));
        }
        if let Some(v) = validator {
            v.validate(content)?;
        }
        let new_id = self
            .pool
            .lock()
            .expect("GreenPool lock poisoned")
            .intern(content);
        let old_id = self.lines[n];
        self.lines[n] = new_id;
        self.undo_stack.push(BranchEdit::Replace {
            at: n,
            old_id,
            new_id,
        });
        self.redo_stack.clear();
        Ok(())
    }

    /// Deletes the line at position `n` (0-based).
    ///
    /// Returns `false` without modifying state if `n >= line_count()`.
    /// On success pushes a `BranchEdit::Delete` onto the undo stack and clears
    /// the redo stack.
    pub fn delete_line(&mut self, n: usize) -> bool {
        if n >= self.lines.len() {
            return false;
        }
        let id = self.lines.remove(n);
        self.undo_stack.push(BranchEdit::Delete { at: n, id });
        self.redo_stack.clear();
        true
    }

    /// Undoes the most recent edit.
    ///
    /// Pops from the undo stack, reverses the operation on `lines`, and pushes
    /// the inverse edit onto the redo stack.  Returns `false` if the undo stack
    /// is empty.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo_stack.pop() else {
            return false;
        };
        match edit {
            BranchEdit::Insert { at, id } => {
                self.lines.remove(at);
                self.redo_stack.push(BranchEdit::Insert { at, id });
            }
            BranchEdit::Delete { at, id } => {
                self.lines.insert(at, id);
                self.redo_stack.push(BranchEdit::Delete { at, id });
            }
            BranchEdit::Replace { at, old_id, new_id } => {
                self.lines[at] = old_id;
                self.redo_stack
                    .push(BranchEdit::Replace { at, old_id, new_id });
            }
        }
        true
    }

    /// Replays the most recently undone edit.
    ///
    /// Pops from the redo stack, re-applies the operation on `lines`, and pushes
    /// the edit onto the undo stack.  Returns `false` if the redo stack is empty.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo_stack.pop() else {
            return false;
        };
        match edit {
            BranchEdit::Insert { at, id } => {
                self.lines.insert(at, id);
                self.undo_stack.push(BranchEdit::Insert { at, id });
            }
            BranchEdit::Delete { at, id } => {
                self.lines.remove(at);
                self.undo_stack.push(BranchEdit::Delete { at, id });
            }
            BranchEdit::Replace { at, old_id, new_id } => {
                self.lines[at] = new_id;
                self.undo_stack
                    .push(BranchEdit::Replace { at, old_id, new_id });
            }
        }
        true
    }

    /// Returns a new `Branch` that shares the same `GreenPool` but starts with
    /// a clone of this branch's current line sequence and empty undo/redo stacks.
    ///
    /// The forked branch is fully independent in its edit history: edits,
    /// undos, and redos on either branch do not affect the other.
    pub fn fork(&self) -> Branch {
        Branch {
            pool: Arc::clone(&self.pool),
            lines: self.lines.clone(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}

// ── LineBufferView for Branch ─────────────────────────────────────────────────

impl crate::line_buffer_view::LineBufferView for Branch {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn get_line(&self, n: usize) -> Option<Arc<str>> {
        let id = *self.lines.get(n)?;
        let pool = self.pool.lock().expect("GreenPool lock poisoned");
        Some(pool.get(id).content_arc())
    }

    fn line_crc32(&self, n: usize) -> Option<u32> {
        let id = *self.lines.get(n)?;
        let pool = self.pool.lock().expect("GreenPool lock poisoned");
        Some(pool.get(id).crc32())
    }

    fn line_md5(&self, n: usize) -> Option<[u8; 16]> {
        let id = *self.lines.get(n)?;
        let pool = self.pool.lock().expect("GreenPool lock poisoned");
        Some(pool.get(id).md5())
    }

    fn line_sha256(&self, n: usize) -> Option<[u8; 32]> {
        let id = *self.lines.get(n)?;
        let pool = self.pool.lock().expect("GreenPool lock poisoned");
        Some(pool.get(id).sha256())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_branch() -> Branch {
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        Branch::new(pool)
    }

    // Helper: intern a line directly via the shared pool.
    fn intern(branch: &Branch, s: &str) -> GreenLineId {
        branch.pool.lock().unwrap().intern(s)
    }

    // ── ML-13: line_count and get_line ───────────────────────────────────────

    #[test]
    fn new_branch_is_empty() {
        let b = make_branch();
        assert_eq!(b.line_count(), 0);
    }

    #[test]
    fn get_line_on_empty_branch_returns_none() {
        let b = make_branch();
        assert!(b.get_line(0).is_none());
    }

    #[test]
    fn get_line_out_of_bounds_returns_none() {
        let mut b = make_branch();
        let id = intern(&b, "only line");
        b.lines.push(id);
        assert!(b.get_line(1).is_none());
    }

    #[test]
    fn get_line_returns_correct_content() {
        let mut b = make_branch();
        let id = intern(&b, "hello");
        b.lines.push(id);
        assert_eq!(b.get_line(0).as_deref(), Some("hello"));
    }

    #[test]
    fn get_line_multiple_lines_correct_order() {
        let mut b = make_branch();
        for s in ["first", "second", "third"] {
            let id = intern(&b, s);
            b.lines.push(id);
        }
        assert_eq!(b.get_line(0).as_deref(), Some("first"));
        assert_eq!(b.get_line(1).as_deref(), Some("second"));
        assert_eq!(b.get_line(2).as_deref(), Some("third"));
        assert!(b.get_line(3).is_none());
    }

    #[test]
    fn line_count_grows_with_pushes() {
        let mut b = make_branch();
        for i in 0..10u32 {
            let id = intern(&b, &format!("line {}", i));
            b.lines.push(id);
            assert_eq!(b.line_count(), (i + 1) as usize);
        }
    }

    #[test]
    fn get_line_unicode_content() {
        let mut b = make_branch();
        for s in ["日本語", "🦆", "مرحبا", "e\u{0301}"] {
            let id = intern(&b, s);
            b.lines.push(id);
        }
        assert_eq!(b.get_line(0).as_deref(), Some("日本語"));
        assert_eq!(b.get_line(1).as_deref(), Some("🦆"));
        assert_eq!(b.get_line(2).as_deref(), Some("مرحبا"));
        assert_eq!(b.get_line(3).as_deref(), Some("e\u{0301}"));
    }

    // ── ML-11: BranchEdit structure ──────────────────────────────────────────

    #[test]
    fn branch_edit_insert_fields() {
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        let id = pool.lock().unwrap().intern("x");
        let edit = BranchEdit::Insert { at: 3, id };
        match edit {
            BranchEdit::Insert { at, id: _ } => assert_eq!(at, 3),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn branch_edit_delete_fields() {
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        let id = pool.lock().unwrap().intern("y");
        let edit = BranchEdit::Delete { at: 7, id };
        match edit {
            BranchEdit::Delete { at, id: _ } => assert_eq!(at, 7),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn branch_edit_replace_fields() {
        let mut p = GreenPool::new();
        let old_id = p.intern("old");
        let new_id = p.intern("new");
        let edit = BranchEdit::Replace {
            at: 2,
            old_id,
            new_id,
        };
        match edit {
            BranchEdit::Replace {
                at,
                old_id: _,
                new_id: _,
            } => assert_eq!(at, 2),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn branch_edit_clone_and_eq() {
        let mut p = GreenPool::new();
        let id = p.intern("z");
        let a = BranchEdit::Insert { at: 0, id };
        let b = a.clone();
        assert_eq!(a, b);
    }

    // ── ML-12: Branch construction ───────────────────────────────────────────

    #[test]
    fn branch_new_undo_redo_stacks_empty() {
        let b = make_branch();
        assert!(b.undo_stack.is_empty());
        assert!(b.redo_stack.is_empty());
    }

    #[test]
    fn two_branches_share_pool() {
        let pool = Arc::new(Mutex::new(GreenPool::new()));
        let b1 = Branch::new(Arc::clone(&pool));
        let b2 = Branch::new(Arc::clone(&pool));
        // Both see the same interned content.
        let id = pool.lock().unwrap().intern("shared");
        assert_eq!(b1.pool.lock().unwrap().get(id).content(), "shared");
        assert_eq!(b2.pool.lock().unwrap().get(id).content(), "shared");
    }

    // ── ML-14: insert_line ───────────────────────────────────────────────────

    #[test]
    fn insert_at_zero_into_empty() {
        let mut b = make_branch();
        b.insert_line(0, "first", None).unwrap();
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.get_line(0).as_deref(), Some("first"));
    }

    #[test]
    fn insert_appends_at_end() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        b.insert_line(1, "b", None).unwrap();
        b.insert_line(2, "c", None).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("a"));
        assert_eq!(b.get_line(1).as_deref(), Some("b"));
        assert_eq!(b.get_line(2).as_deref(), Some("c"));
    }

    #[test]
    fn insert_in_middle_shifts_rest() {
        let mut b = make_branch();
        b.insert_line(0, "first", None).unwrap();
        b.insert_line(1, "third", None).unwrap();
        b.insert_line(1, "second", None).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("first"));
        assert_eq!(b.get_line(1).as_deref(), Some("second"));
        assert_eq!(b.get_line(2).as_deref(), Some("third"));
    }

    #[test]
    fn insert_pushes_to_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "x", None).unwrap();
        assert_eq!(b.undo_stack.len(), 1);
        assert!(matches!(b.undo_stack[0], BranchEdit::Insert { at: 0, .. }));
    }

    #[test]
    fn insert_clears_redo_stack() {
        let mut b = make_branch();
        // Manually push a fake redo entry to confirm insert clears it.
        let id = intern(&b, "phantom");
        b.redo_stack.push(BranchEdit::Insert { at: 0, id });
        b.insert_line(0, "real", None).unwrap();
        assert!(b.redo_stack.is_empty());
    }

    #[test]
    fn insert_with_validator_accepted() {
        use crate::encoding::AsciiValidator;
        let mut b = make_branch();
        b.insert_line(0, "ascii only", Some(&AsciiValidator))
            .unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("ascii only"));
    }

    #[test]
    fn insert_with_validator_rejected_leaves_branch_unchanged() {
        use crate::encoding::AsciiValidator;
        let mut b = make_branch();
        b.insert_line(0, "initial", None).unwrap();
        let result = b.insert_line(1, "日本語", Some(&AsciiValidator));
        assert!(result.is_err());
        // State must be unchanged.
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.get_line(0).as_deref(), Some("initial"));
        // Undo stack must not have grown.
        assert_eq!(b.undo_stack.len(), 1);
    }

    #[test]
    fn insert_no_validator_accepts_unicode() {
        let mut b = make_branch();
        b.insert_line(0, "🦆日本語مرحبا", None).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("🦆日本語مرحبا"));
    }

    #[test]
    fn insert_empty_string_line() {
        let mut b = make_branch();
        b.insert_line(0, "", None).unwrap();
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.get_line(0).as_deref(), Some(""));
    }

    // ── ML-15: replace_line ──────────────────────────────────────────────────

    #[test]
    fn replace_middle_line_changes_content() {
        let mut b = make_branch();
        for s in ["a", "b", "c"] {
            b.insert_line(b.line_count(), s, None).unwrap();
        }
        b.replace_line(1, "B", None).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("a"));
        assert_eq!(b.get_line(1).as_deref(), Some("B"));
        assert_eq!(b.get_line(2).as_deref(), Some("c"));
        assert_eq!(b.line_count(), 3);
    }

    #[test]
    fn replace_first_line() {
        let mut b = make_branch();
        b.insert_line(0, "old", None).unwrap();
        b.replace_line(0, "new", None).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("new"));
    }

    #[test]
    fn replace_last_line() {
        let mut b = make_branch();
        for s in ["x", "y", "z"] {
            b.insert_line(b.line_count(), s, None).unwrap();
        }
        b.replace_line(2, "Z", None).unwrap();
        assert_eq!(b.get_line(2).as_deref(), Some("Z"));
        assert_eq!(b.line_count(), 3);
    }

    #[test]
    fn replace_out_of_bounds_returns_err_without_state_change() {
        let mut b = make_branch();
        b.insert_line(0, "only", None).unwrap();
        let before_undo_len = b.undo_stack.len();
        let result = b.replace_line(1, "oob", None);
        assert!(result.is_err());
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.get_line(0).as_deref(), Some("only"));
        assert_eq!(b.undo_stack.len(), before_undo_len);
    }

    #[test]
    fn replace_on_empty_branch_returns_err() {
        let mut b = make_branch();
        assert!(b.replace_line(0, "x", None).is_err());
    }

    #[test]
    fn replace_pushes_replace_edit_to_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "old", None).unwrap();
        b.replace_line(0, "new", None).unwrap();
        assert_eq!(b.undo_stack.len(), 2);
        assert!(matches!(b.undo_stack[1], BranchEdit::Replace { at: 0, .. }));
    }

    #[test]
    fn replace_clears_redo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        let id = intern(&b, "phantom_redo");
        b.redo_stack.push(BranchEdit::Insert { at: 0, id });
        b.replace_line(0, "b", None).unwrap();
        assert!(b.redo_stack.is_empty());
    }

    #[test]
    fn replace_with_validator_accepted() {
        use crate::encoding::AsciiValidator;
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        b.replace_line(0, "world", Some(&AsciiValidator)).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("world"));
    }

    #[test]
    fn replace_with_validator_rejected_leaves_branch_unchanged() {
        use crate::encoding::AsciiValidator;
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        let undo_len = b.undo_stack.len();
        let result = b.replace_line(0, "日本語", Some(&AsciiValidator));
        assert!(result.is_err());
        assert_eq!(b.get_line(0).as_deref(), Some("hello"));
        assert_eq!(b.undo_stack.len(), undo_len);
    }

    #[test]
    fn replace_with_unicode_no_validator() {
        let mut b = make_branch();
        b.insert_line(0, "ascii", None).unwrap();
        b.replace_line(0, "🦆日本語", None).unwrap();
        assert_eq!(b.get_line(0).as_deref(), Some("🦆日本語"));
    }

    #[test]
    fn replace_records_correct_old_and_new_ids() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        let old_id = b.lines[0];
        b.replace_line(0, "replacement", None).unwrap();
        let new_id = b.lines[0];
        assert_ne!(old_id, new_id);
        match b.undo_stack.last().unwrap() {
            BranchEdit::Replace {
                at,
                old_id: rec_old,
                new_id: rec_new,
            } => {
                assert_eq!(*at, 0);
                assert_eq!(*rec_old, old_id);
                assert_eq!(*rec_new, new_id);
            }
            _ => panic!("expected Replace edit on undo stack"),
        }
    }

    // ── ML-16: delete_line ───────────────────────────────────────────────────

    #[test]
    fn delete_first_line() {
        let mut b = make_branch();
        for s in ["a", "b", "c"] {
            b.insert_line(b.line_count(), s, None).unwrap();
        }
        assert!(b.delete_line(0));
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.get_line(0).as_deref(), Some("b"));
        assert_eq!(b.get_line(1).as_deref(), Some("c"));
    }

    #[test]
    fn delete_last_line() {
        let mut b = make_branch();
        for s in ["a", "b", "c"] {
            b.insert_line(b.line_count(), s, None).unwrap();
        }
        assert!(b.delete_line(2));
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.get_line(0).as_deref(), Some("a"));
        assert_eq!(b.get_line(1).as_deref(), Some("b"));
    }

    #[test]
    fn delete_middle_line() {
        let mut b = make_branch();
        for s in ["a", "b", "c"] {
            b.insert_line(b.line_count(), s, None).unwrap();
        }
        assert!(b.delete_line(1));
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.get_line(0).as_deref(), Some("a"));
        assert_eq!(b.get_line(1).as_deref(), Some("c"));
    }

    #[test]
    fn delete_out_of_bounds_returns_false() {
        let mut b = make_branch();
        b.insert_line(0, "only", None).unwrap();
        assert!(!b.delete_line(1));
        assert_eq!(b.line_count(), 1);
    }

    #[test]
    fn delete_on_empty_branch_returns_false() {
        let mut b = make_branch();
        assert!(!b.delete_line(0));
        assert_eq!(b.line_count(), 0);
    }

    #[test]
    fn delete_pushes_delete_edit_to_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "x", None).unwrap();
        let id = b.lines[0];
        b.delete_line(0);
        assert!(matches!(
            b.undo_stack.last().unwrap(),
            BranchEdit::Delete { at: 0, id: stored_id } if *stored_id == id
        ));
    }

    #[test]
    fn delete_clears_redo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        let id = intern(&b, "phantom");
        b.redo_stack.push(BranchEdit::Insert { at: 0, id });
        b.delete_line(0);
        assert!(b.redo_stack.is_empty());
    }

    #[test]
    fn delete_out_of_bounds_does_not_touch_undo_or_redo() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        let undo_len = b.undo_stack.len();
        assert!(!b.delete_line(5));
        assert_eq!(b.undo_stack.len(), undo_len);
        assert!(b.redo_stack.is_empty());
    }

    #[test]
    fn delete_sole_line_leaves_empty() {
        let mut b = make_branch();
        b.insert_line(0, "alone", None).unwrap();
        assert!(b.delete_line(0));
        assert_eq!(b.line_count(), 0);
        assert!(b.get_line(0).is_none());
    }

    // ML-17: undo()

    #[test]
    fn undo_on_empty_stack_returns_false() {
        let mut b = make_branch();
        assert!(!b.undo());
    }

    #[test]
    fn undo_after_insert_removes_line() {
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        assert_eq!(b.line_count(), 1);
        assert!(b.undo());
        assert_eq!(b.line_count(), 0);
    }

    #[test]
    fn undo_after_insert_pushes_to_redo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        let id = b.lines[0];
        b.undo();
        assert!(matches!(
            b.redo_stack.last().unwrap(),
            BranchEdit::Insert { at: 0, id: rid } if *rid == id
        ));
    }

    #[test]
    fn undo_after_insert_clears_undo_entry() {
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        assert_eq!(b.undo_stack.len(), 1);
        b.undo();
        assert!(b.undo_stack.is_empty());
    }

    #[test]
    fn undo_after_replace_restores_original_content() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        b.replace_line(0, "modified", None).unwrap();
        b.undo();
        let id = b.lines[0];
        let pool = b.pool.lock().unwrap();
        assert_eq!(pool.get(id).content(), "original");
    }

    #[test]
    fn undo_after_replace_pushes_replace_to_redo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        let old_id = b.lines[0];
        b.replace_line(0, "modified", None).unwrap();
        let new_id = b.lines[0];
        b.undo(); // clears the replace edit
                  // undo of the replace: redo_stack should hold Replace { old_id, new_id }
        assert!(matches!(
            b.redo_stack.last().unwrap(),
            BranchEdit::Replace { at: 0, old_id: oid, new_id: nid }
                if *oid == old_id && *nid == new_id
        ));
    }

    #[test]
    fn undo_after_delete_restores_line() {
        let mut b = make_branch();
        b.insert_line(0, "line0", None).unwrap();
        b.insert_line(1, "line1", None).unwrap();
        let id0 = b.lines[0];
        b.delete_line(0);
        assert_eq!(b.line_count(), 1);
        b.undo();
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.lines[0], id0);
    }

    #[test]
    fn undo_after_delete_pushes_delete_to_redo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "alpha", None).unwrap();
        let id = b.lines[0];
        b.delete_line(0);
        b.undo();
        assert!(matches!(
            b.redo_stack.last().unwrap(),
            BranchEdit::Delete { at: 0, id: rid } if *rid == id
        ));
    }

    #[test]
    fn undo_multiple_inserts_in_reverse_order() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        b.insert_line(1, "b", None).unwrap();
        b.insert_line(2, "c", None).unwrap();
        assert_eq!(b.line_count(), 3);
        assert!(b.undo());
        assert_eq!(b.line_count(), 2);
        assert!(b.undo());
        assert_eq!(b.line_count(), 1);
        assert!(b.undo());
        assert_eq!(b.line_count(), 0);
        assert!(!b.undo()); // stack empty
    }

    #[test]
    fn undo_returns_true_when_edit_is_applied() {
        let mut b = make_branch();
        b.insert_line(0, "x", None).unwrap();
        assert!(b.undo());
    }

    // ML-18: redo()

    #[test]
    fn redo_on_empty_stack_returns_false() {
        let mut b = make_branch();
        assert!(!b.redo());
    }

    #[test]
    fn redo_after_undo_insert_restores_line() {
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        b.undo();
        assert_eq!(b.line_count(), 0);
        assert!(b.redo());
        assert_eq!(b.line_count(), 1);
    }

    #[test]
    fn redo_after_undo_insert_pushes_to_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "hello", None).unwrap();
        let id = b.lines[0];
        b.undo();
        b.redo();
        assert!(matches!(
            b.undo_stack.last().unwrap(),
            BranchEdit::Insert { at: 0, id: uid } if *uid == id
        ));
    }

    #[test]
    fn redo_after_undo_replace_restores_modified_content() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        b.replace_line(0, "modified", None).unwrap();
        b.undo(); // back to "original"
        b.redo(); // forward to "modified"
        let id = b.lines[0];
        let pool = b.pool.lock().unwrap();
        assert_eq!(pool.get(id).content(), "modified");
    }

    #[test]
    fn redo_after_undo_replace_pushes_replace_to_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        let old_id = b.lines[0];
        b.replace_line(0, "modified", None).unwrap();
        let new_id = b.lines[0];
        b.undo();
        b.redo();
        assert!(matches!(
            b.undo_stack.last().unwrap(),
            BranchEdit::Replace { at: 0, old_id: oid, new_id: nid }
                if *oid == old_id && *nid == new_id
        ));
    }

    #[test]
    fn redo_after_undo_delete_removes_line_again() {
        let mut b = make_branch();
        b.insert_line(0, "alpha", None).unwrap();
        b.insert_line(1, "beta", None).unwrap();
        b.delete_line(0);
        b.undo(); // "alpha" restored at 0
        assert_eq!(b.line_count(), 2);
        b.redo(); // "alpha" deleted again
        assert_eq!(b.line_count(), 1);
    }

    #[test]
    fn redo_after_undo_delete_pushes_delete_to_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "alpha", None).unwrap();
        let id = b.lines[0];
        b.delete_line(0);
        b.undo();
        b.redo();
        assert!(matches!(
            b.undo_stack.last().unwrap(),
            BranchEdit::Delete { at: 0, id: uid } if *uid == id
        ));
    }

    #[test]
    fn redo_clears_when_new_edit_is_made() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        b.undo();
        // redo stack is now non-empty
        assert!(!b.redo_stack.is_empty());
        // a new edit (insert) must clear redo on the *new* insert
        b.insert_line(0, "b", None).unwrap();
        assert!(b.redo_stack.is_empty());
    }

    #[test]
    fn undo_redo_sequence_of_ten_edits() {
        let mut b = make_branch();
        let contents: Vec<&str> = vec![
            "line0", "line1", "line2", "line3", "line4", "line5", "line6", "line7", "line8",
            "line9",
        ];
        for (i, s) in contents.iter().enumerate() {
            b.insert_line(i, s, None).unwrap();
        }
        assert_eq!(b.line_count(), 10);
        // undo all 10
        for i in (1..=10).rev() {
            assert!(b.undo());
            assert_eq!(b.line_count(), i - 1);
        }
        assert!(!b.undo());
        // redo all 10
        for i in 1..=10 {
            assert!(b.redo());
            assert_eq!(b.line_count(), i);
        }
        assert!(!b.redo());
    }

    #[test]
    fn redo_returns_true_when_edit_is_applied() {
        let mut b = make_branch();
        b.insert_line(0, "x", None).unwrap();
        b.undo();
        assert!(b.redo());
    }

    // ML-19: fork()

    #[test]
    fn fork_shares_pool_with_parent() {
        let b = make_branch();
        let f = b.fork();
        assert!(Arc::ptr_eq(&b.pool, &f.pool));
    }

    #[test]
    fn fork_copies_lines_from_parent() {
        let mut b = make_branch();
        b.insert_line(0, "alpha", None).unwrap();
        b.insert_line(1, "beta", None).unwrap();
        let f = b.fork();
        assert_eq!(f.line_count(), 2);
        assert_eq!(f.lines[0], b.lines[0]);
        assert_eq!(f.lines[1], b.lines[1]);
    }

    #[test]
    fn fork_starts_with_empty_undo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "x", None).unwrap();
        let f = b.fork();
        assert!(f.undo_stack.is_empty());
    }

    #[test]
    fn fork_starts_with_empty_redo_stack() {
        let mut b = make_branch();
        b.insert_line(0, "x", None).unwrap();
        b.undo();
        let f = b.fork();
        assert!(f.redo_stack.is_empty());
    }

    #[test]
    fn fork_of_empty_branch_is_empty() {
        let b = make_branch();
        let f = b.fork();
        assert_eq!(f.line_count(), 0);
    }

    #[test]
    fn edits_on_fork_do_not_affect_parent() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        let mut f = b.fork();
        f.insert_line(1, "fork-only", None).unwrap();
        assert_eq!(b.line_count(), 1);
        assert_eq!(f.line_count(), 2);
    }

    #[test]
    fn edits_on_parent_do_not_affect_fork() {
        let mut b = make_branch();
        b.insert_line(0, "original", None).unwrap();
        let f = b.fork();
        b.insert_line(1, "parent-only", None).unwrap();
        assert_eq!(b.line_count(), 2);
        assert_eq!(f.line_count(), 1);
    }

    #[test]
    fn undo_on_parent_does_not_affect_fork() {
        let mut b = make_branch();
        b.insert_line(0, "a", None).unwrap();
        b.insert_line(1, "b", None).unwrap();
        let f = b.fork();
        b.undo();
        assert_eq!(b.line_count(), 1);
        assert_eq!(f.line_count(), 2);
    }

    #[test]
    fn fork_interned_content_accessible_via_shared_pool() {
        let mut b = make_branch();
        b.insert_line(0, "shared-content", None).unwrap();
        let id = b.lines[0];
        let f = b.fork();
        let pool = f.pool.lock().unwrap();
        assert_eq!(pool.get(id).content(), "shared-content");
    }

    #[test]
    fn fork_of_fork_is_independent() {
        let mut b = make_branch();
        b.insert_line(0, "root", None).unwrap();
        let mut f1 = b.fork();
        f1.insert_line(1, "f1-line", None).unwrap();
        let f2 = f1.fork();
        // f2 sees root + f1-line
        assert_eq!(f2.line_count(), 2);
        // further edit to f1 doesn't affect f2
        f1.insert_line(2, "f1-extra", None).unwrap();
        assert_eq!(f1.line_count(), 3);
        assert_eq!(f2.line_count(), 2);
        assert!(Arc::ptr_eq(&b.pool, &f2.pool));
    }

    #[test]
    fn forked_branch_can_undo_its_own_edits() {
        let mut b = make_branch();
        b.insert_line(0, "base", None).unwrap();
        let mut f = b.fork();
        f.insert_line(1, "fork-edit", None).unwrap();
        assert_eq!(f.line_count(), 2);
        assert!(f.undo());
        assert_eq!(f.line_count(), 1);
        // parent untouched
        assert_eq!(b.line_count(), 1);
    }

    // ── ML-20: Milestone 3 comprehensive tests ───────────────────────────────

    /// 20+ sequential inserts preserve correct ordering.
    #[test]
    fn twenty_five_sequential_inserts_correct_order() {
        let mut b = make_branch();
        let n = 25usize;
        for i in 0..n {
            b.insert_line(i, &format!("line-{}", i), None).unwrap();
        }
        assert_eq!(b.line_count(), n);
        for i in 0..n {
            assert_eq!(
                b.get_line(i).as_deref(),
                Some(format!("line-{}", i).as_str()),
                "line {} mismatch",
                i
            );
        }
    }

    /// Multi-operation undo/redo: a sequence of inserts, replaces, and deletes
    /// can all be undone and redone in order.
    #[test]
    fn mixed_operations_undo_redo_sequence() {
        let mut b = make_branch();

        // Build state: insert 5 lines.
        for i in 0..5usize {
            b.insert_line(i, &format!("L{}", i), None).unwrap();
        }
        // Replace line 2.
        b.replace_line(2, "L2-replaced", None).unwrap();
        // Delete line 4.
        b.delete_line(4);
        // Insert a new line at the end.
        b.insert_line(b.line_count(), "L-new", None).unwrap();

        // Snapshot after: insert×5, replace, delete, insert = [L0, L1, L2-replaced, L3, L-new]
        assert_eq!(b.line_count(), 5);
        assert_eq!(b.get_line(0).as_deref(), Some("L0"));
        assert_eq!(b.get_line(2).as_deref(), Some("L2-replaced"));
        assert_eq!(b.get_line(4).as_deref(), Some("L-new"));

        // Undo the insert of L-new.
        assert!(b.undo());
        assert_eq!(b.line_count(), 4);
        assert!(b.get_line(4).is_none());

        // Undo the delete of L4.
        assert!(b.undo());
        assert_eq!(b.line_count(), 5);

        // Undo the replace of L2.
        assert!(b.undo());
        assert_eq!(b.get_line(2).as_deref(), Some("L2"));

        // Undo the 5 original inserts.
        for i in (0..5).rev() {
            assert!(b.undo(), "undo insert {} failed", i);
        }
        assert_eq!(b.line_count(), 0);
        assert!(!b.undo()); // stack empty

        // Redo everything.
        for _ in 0..5 {
            assert!(b.redo());
        }
        assert_eq!(b.line_count(), 5);

        assert!(b.redo()); // redo replace
        assert_eq!(b.get_line(2).as_deref(), Some("L2-replaced"));

        assert!(b.redo()); // redo delete of L4
        assert_eq!(b.line_count(), 4);

        assert!(b.redo()); // redo insert of L-new
        assert_eq!(b.line_count(), 5);
        assert_eq!(b.get_line(4).as_deref(), Some("L-new"));

        assert!(!b.redo()); // redo stack empty
    }

    // ── ML-43: LineBufferView trait tests ─────────────────────────────────────

    use crate::line_buffer_view::LineBufferView;

    fn branch_with_lines(lines: &[&str]) -> Branch {
        let mut b = make_branch();
        for (i, &s) in lines.iter().enumerate() {
            b.insert_line(i, s, None).unwrap();
        }
        b
    }

    // CRC32, MD5, SHA-256 for "hello" — universally accepted test vectors.
    // CRC32: IEEE 802.3 polynomial (same as zlib / crc32fast).
    // MD5 and SHA-256: standard NIST vectors.
    const HELLO_CRC32: u32 = 0x3610_a686;
    const HELLO_MD5: [u8; 16] = [
        0x5d, 0x41, 0x40, 0x2a, 0xbc, 0x4b, 0x2a, 0x76, 0xb9, 0x71, 0x9d, 0x91, 0x10, 0x17, 0xc5,
        0x92,
    ];
    const HELLO_SHA256: [u8; 32] = [
        0x2c, 0xf2, 0x4d, 0xba, 0x5f, 0xb0, 0xa3, 0x0e, 0x26, 0xe8, 0x3b, 0x2a, 0xc5, 0xb9, 0xe2,
        0x9e, 0x1b, 0x16, 0x1e, 0x5c, 0x1f, 0xa7, 0x42, 0x5e, 0x73, 0x04, 0x33, 0x62, 0x93, 0x8b,
        0x98, 0x24,
    ];

    #[test]
    fn view_crc32_known_value_hello() {
        let b = branch_with_lines(&["hello"]);
        assert_eq!(b.line_crc32(0), Some(HELLO_CRC32));
    }

    #[test]
    fn view_md5_known_value_hello() {
        let b = branch_with_lines(&["hello"]);
        assert_eq!(b.line_md5(0), Some(HELLO_MD5));
    }

    #[test]
    fn view_sha256_known_value_hello() {
        let b = branch_with_lines(&["hello"]);
        assert_eq!(b.line_sha256(0), Some(HELLO_SHA256));
    }

    #[test]
    fn view_out_of_bounds_get_line_returns_none() {
        let b = branch_with_lines(&["only"]);
        assert!(b.get_line(1).is_none());
    }

    #[test]
    fn view_out_of_bounds_crc32_returns_none() {
        let b = branch_with_lines(&["only"]);
        assert!(b.line_crc32(1).is_none());
    }

    #[test]
    fn view_out_of_bounds_md5_returns_none() {
        let b = branch_with_lines(&["only"]);
        assert!(b.line_md5(1).is_none());
    }

    #[test]
    fn view_out_of_bounds_sha256_returns_none() {
        let b = branch_with_lines(&["only"]);
        assert!(b.line_sha256(1).is_none());
    }

    #[test]
    fn view_out_of_bounds_all_methods_on_empty_branch() {
        let b = make_branch();
        assert!(b.get_line(0).is_none());
        assert!(b.line_crc32(0).is_none());
        assert!(b.line_md5(0).is_none());
        assert!(b.line_sha256(0).is_none());
    }

    #[test]
    fn view_via_dyn_ref_trait_object() {
        let b = branch_with_lines(&["alpha", "beta"]);
        let view: &dyn LineBufferView = &b;
        assert_eq!(view.line_count(), 2);
        assert_eq!(view.get_line(0).as_deref(), Some("alpha"));
        assert_eq!(view.get_line(1).as_deref(), Some("beta"));
        assert!(view.line_crc32(0).is_some());
    }

    #[test]
    fn view_via_box_dyn_trait_object() {
        let b = branch_with_lines(&["gamma"]);
        let view: Box<dyn LineBufferView> = Box::new(b);
        assert_eq!(view.line_count(), 1);
        assert_eq!(view.get_line(0).as_deref(), Some("gamma"));
        assert!(view.line_crc32(0).is_some());
    }

    #[test]
    fn view_hash_consistent_between_greenline_and_trait() {
        let b = branch_with_lines(&["consistent"]);
        let id = b.lines[0];
        let pool = b.pool.lock().unwrap();
        let gl = pool.get(id);
        let expected_crc32 = gl.crc32();
        let expected_md5 = gl.md5();
        let expected_sha256 = gl.sha256();
        drop(pool);

        assert_eq!(b.line_crc32(0), Some(expected_crc32));
        assert_eq!(b.line_md5(0), Some(expected_md5));
        assert_eq!(b.line_sha256(0), Some(expected_sha256));
    }

    #[test]
    fn view_ten_diverse_lines_all_accessible() {
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
        let b = branch_with_lines(&data);
        let view: &dyn LineBufferView = &b;
        assert_eq!(view.line_count(), 10);
        for (i, &expected) in data.iter().enumerate() {
            assert_eq!(view.get_line(i).as_deref(), Some(expected));
            assert!(view.line_crc32(i).is_some());
            assert!(view.line_md5(i).is_some());
            assert!(view.line_sha256(i).is_some());
        }
    }

    #[test]
    fn view_ten_diverse_lines_hashes_are_unique_per_line() {
        let data = [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
        ];
        let b = branch_with_lines(&data);
        // All CRC32 values should be mutually distinct (lines are distinct)
        let crcs: Vec<u32> = (0..10).map(|i| b.line_crc32(i).unwrap()).collect();
        let mut sorted = crcs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 10, "expected distinct CRC32 per unique line");
    }

    #[test]
    fn view_forked_branch_reflects_fork_content_independently() {
        let mut parent = branch_with_lines(&["shared-a", "shared-b"]);
        let mut child = parent.fork();

        // Modify parent after fork
        parent.insert_line(2, "parent-only", None).unwrap();
        // Modify child differently
        child.insert_line(0, "child-only", None).unwrap();

        // Parent view
        assert_eq!(parent.line_count(), 3);
        assert_eq!(parent.get_line(2).as_deref(), Some("parent-only"));
        assert!(parent.line_crc32(2).is_some());

        // Child view sees child-only at 0, originals shifted
        assert_eq!(child.line_count(), 3);
        assert_eq!(child.get_line(0).as_deref(), Some("child-only"));
        assert_eq!(child.get_line(1).as_deref(), Some("shared-a"));

        // Trait object view of child
        let child_view: &dyn LineBufferView = &child;
        assert_eq!(child_view.get_line(0).as_deref(), Some("child-only"));
        assert!(child_view.line_crc32(0).is_some());
    }

    #[test]
    fn view_line_buffer_via_trait_object() {
        use crate::line_buffer::LineBuffer;
        let mut lb = LineBuffer::new(None);
        lb.insert_line(0, "hello").unwrap();
        lb.insert_line(1, "world").unwrap();
        let view: &dyn LineBufferView = &lb;
        assert_eq!(view.line_count(), 2);
        assert_eq!(view.get_line(0).as_deref(), Some("hello"));
        assert_eq!(view.line_crc32(0), Some(HELLO_CRC32));
        assert_eq!(view.line_md5(0), Some(HELLO_MD5));
        assert_eq!(view.line_sha256(0), Some(HELLO_SHA256));
    }

    #[test]
    fn view_line_buffer_out_of_bounds_via_trait() {
        use crate::line_buffer::LineBuffer;
        let lb = LineBuffer::new(None);
        let view: &dyn LineBufferView = &lb;
        assert!(view.get_line(0).is_none());
        assert!(view.line_crc32(0).is_none());
        assert!(view.line_md5(0).is_none());
        assert!(view.line_sha256(0).is_none());
    }
}
