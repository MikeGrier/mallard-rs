// Copyright (c) 2026, Michael Grier.
//! ML-45: Integration test -- AsciiValidator end-to-end.
//!
//! Verifies that a `LineBuffer` created with `AsciiValidator`:
//!   - accepts 10+ valid ASCII edits (insert, replace, delete)
//!   - rejects 5+ edits with non-ASCII content with the correct error
//!   - leaves buffer state unchanged after every rejection
//!   - undo/redo work correctly around rejections

use std::sync::Arc;

use mallard::{AsciiValidator, EncodingError, EncodingValidator, LineBuffer};

// ── Helper ───────────────────────────────────────────────────────────────────

fn ascii_buf(s: &str) -> LineBuffer {
    let v: Arc<dyn EncodingValidator> = Arc::new(AsciiValidator);
    LineBuffer::from_str(s, Some(v)).expect("ASCII-only input must succeed")
}

fn collect(lb: &LineBuffer) -> Vec<String> {
    lb.iter_lines().map(|s| s.to_string()).collect()
}

/// Assert that an edit was rejected, that the error's `line` field matches
/// `expected_line`, and that the error description mentions "U+".
fn assert_rejected(result: Result<(), EncodingError>, expected_line: &str) {
    let err = result.expect_err("expected rejection but got Ok");
    assert_eq!(&*err.line, expected_line);
    assert!(
        err.description.contains("U+"),
        "description should contain 'U+', got: {}",
        err.description
    );
}

// ── Test 1: from_str rejects non-ASCII content up front ──────────────────────

#[test]
fn from_str_rejects_non_ascii() {
    let v: Arc<dyn EncodingValidator> = Arc::new(AsciiValidator);
    // Contains U+1F986 duck emoji
    let result = LineBuffer::from_str("hello\n\u{1F986}\nworld\n", Some(v));
    let err = result.err().expect("should reject non-ASCII in from_str");
    assert_eq!(&*err.line, "\u{1F986}");
    assert!(err.description.contains("U+1F986"));
}

// ── Test 2: from_str accepts all-ASCII content ────────────────────────────────

#[test]
fn from_str_accepts_ascii() {
    let lb = ascii_buf("hello\nworld\nfoo\n");
    assert_eq!(lb.line_count(), 3);
    assert_eq!(lb.get_line(0).as_deref(), Some("hello"));
}

// ── Test 3: 10+ successful ASCII inserts ──────────────────────────────────────

#[test]
fn ascii_inserts_succeed() {
    let mut lb = ascii_buf("start\n");
    assert_eq!(lb.line_count(), 1);

    let lines = [
        "line-A", "line-B", "line-C", "line-D", "line-E", "line-F", "line-G", "line-H", "line-I",
        "line-J",
    ];
    for (i, &content) in lines.iter().enumerate() {
        lb.insert_line(i + 1, content)
            .expect("ASCII insert must succeed");
    }
    assert_eq!(lb.line_count(), 11);
    assert_eq!(lb.get_line(0).as_deref(), Some("start"));
    for (i, &content) in lines.iter().enumerate() {
        assert_eq!(lb.get_line(i + 1).as_deref(), Some(content));
    }
}

// ── Test 4: 10+ successful ASCII replaces ─────────────────────────────────────

#[test]
fn ascii_replaces_succeed() {
    let mut lb = ascii_buf("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
    assert_eq!(lb.line_count(), 10);

    let replacements = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
    for (i, &r) in replacements.iter().enumerate() {
        lb.replace_line(i, r).expect("ASCII replace must succeed");
    }
    for (i, &r) in replacements.iter().enumerate() {
        assert_eq!(lb.get_line(i).as_deref(), Some(r));
    }
}

// ── Test 5: non-ASCII insert rejected, state unchanged ───────────────────────

#[test]
fn non_ascii_insert_rejected() {
    let mut lb = ascii_buf("alpha\nbeta\ngamma\n");
    let before = collect(&lb);

    // Reject U+00E9 (é), U+4E2D (中), U+1F600 (😀), U+03B1 (α), U+00C0 (À)
    let bad_inputs = [
        "caf\u{00E9}",              // café - Latin Extended
        "\u{4E2D}\u{6587}",         // 中文 - CJK
        "hi \u{1F600}",             // hi 😀 - emoji
        "\u{03B1}\u{03B2}\u{03B3}", // αβγ - Greek
        "\u{00C0}ngstr\u{00F6}m",   // Ångström - Latin Extended
    ];
    for &bad in &bad_inputs {
        assert_rejected(lb.insert_line(1, bad), bad);
        assert_eq!(
            collect(&lb),
            before,
            "state must be unchanged after rejection of {:?}",
            bad
        );
    }
    assert_eq!(lb.line_count(), 3);
}

// ── Test 6: non-ASCII replace rejected, state unchanged ──────────────────────

#[test]
fn non_ascii_replace_rejected() {
    let mut lb = ascii_buf("line0\nline1\nline2\n");

    // Try to replace line 1 with non-ASCII content 5+ times.
    let bad_inputs = [
        "\u{1F986}",        // 🦆 duck emoji
        "\u{65E5}\u{672C}", // 日本
        "\u{D55C}",         // 한
        "\u{00B5}",         // µ (micro sign, U+00B5)
        "\u{2603}",         // ☃ snowman
        "\u{00FF}",         // ÿ (y with diaeresis)
    ];
    for &bad in &bad_inputs {
        assert_rejected(lb.replace_line(1, bad), bad);
        // Line 1 must still be "line1" after each rejection.
        assert_eq!(
            lb.get_line(1).as_deref(),
            Some("line1"),
            "line 1 must be unchanged after rejection of {:?}",
            bad
        );
        assert_eq!(lb.line_count(), 3);
    }
}

// ── Test 7: undo after valid ASCII edit, then rejected edit, then redo ────────

#[test]
fn undo_redo_around_rejections() {
    let mut lb = ascii_buf("x\ny\nz\n");

    // Valid edit.
    lb.insert_line(3, "w").unwrap();
    assert_eq!(lb.line_count(), 4);

    // Rejected edit -- does not affect undo stack.
    assert_rejected(lb.insert_line(4, "\u{1F600}"), "\u{1F600}");
    assert_eq!(lb.line_count(), 4); // still 4

    // Undo the valid insert.
    assert!(lb.undo());
    assert_eq!(lb.line_count(), 3);
    assert_eq!(collect(&lb), vec!["x", "y", "z"]);

    // Redo re-applies the valid insert.
    assert!(lb.redo());
    assert_eq!(lb.line_count(), 4);
    assert_eq!(lb.get_line(3).as_deref(), Some("w"));
}

// ── Test 8: undo/redo chain interleaved with multiple rejections ──────────────

#[test]
fn undo_redo_chain_with_rejections() {
    let mut lb = ascii_buf("p\nq\nr\n");

    // Build up valid history.
    lb.replace_line(0, "P").unwrap(); // step 1
    lb.insert_line(3, "s").unwrap(); // step 2
    lb.delete_line(1); // step 3 (removes "q")
    assert_eq!(collect(&lb), vec!["P", "r", "s"]);

    // Intersperse rejections -- none should change state or undo stack.
    assert_rejected(lb.replace_line(0, "\u{00C9}P"), "\u{00C9}P");
    assert_rejected(lb.insert_line(1, "\u{4E2D}"), "\u{4E2D}");
    assert_eq!(collect(&lb), vec!["P", "r", "s"]);

    // Undo step 3 (delete).
    lb.undo();
    assert_eq!(collect(&lb), vec!["P", "q", "r", "s"]);

    // Another rejection in the middle of undoing.
    assert_rejected(lb.replace_line(2, "\u{1F986}"), "\u{1F986}");
    assert_eq!(collect(&lb), vec!["P", "q", "r", "s"]);

    // Undo step 2 (insert "s").
    lb.undo();
    assert_eq!(collect(&lb), vec!["P", "q", "r"]);

    // Undo step 1 (replace P -> p).
    lb.undo();
    assert_eq!(collect(&lb), vec!["p", "q", "r"]);

    // Redo all three.
    lb.redo();
    lb.redo();
    lb.redo();
    assert_eq!(collect(&lb), vec!["P", "r", "s"]);
}

// ── Test 9: replace at boundary indices rejected cleanly ─────────────────────

#[test]
fn non_ascii_replace_boundary_indices() {
    let mut lb = ascii_buf("first\nlast\n");

    // Replace first line -- rejected.
    assert_rejected(lb.replace_line(0, "\u{03A9}mega"), "\u{03A9}mega"); // Ωmega
    assert_eq!(lb.get_line(0).as_deref(), Some("first"));

    // Replace last line -- rejected.
    assert_rejected(lb.replace_line(1, "caf\u{00E9}"), "caf\u{00E9}");
    assert_eq!(lb.get_line(1).as_deref(), Some("last"));

    assert_eq!(lb.line_count(), 2);
}

// ── Test 10: mixed valid/invalid edits preserve correct state ─────────────────

#[test]
fn mixed_valid_invalid_edits_preserve_state() {
    let mut lb = ascii_buf("a\nb\nc\nd\ne\n");

    lb.replace_line(0, "A").unwrap(); // ok
    assert_rejected(lb.replace_line(1, "\u{1F4A9}"), "\u{1F4A9}"); // rejected
    lb.replace_line(1, "B").unwrap(); // ok
    assert_rejected(lb.insert_line(2, "\u{2603}"), "\u{2603}"); // rejected
    lb.insert_line(2, "BB").unwrap(); // ok
    lb.delete_line(5); // ok (removes "e")
    assert_rejected(lb.replace_line(0, "\u{00DF}"), "\u{00DF}"); // rejected

    // Expected: [A, B, BB, c, d]
    assert_eq!(collect(&lb), vec!["A", "B", "BB", "c", "d"]);
    assert_eq!(lb.line_count(), 5);

    // Undo the delete, two replaces, and one insert.
    lb.undo(); // undo delete "e"
    assert_eq!(lb.line_count(), 6);
    lb.undo(); // undo insert "BB"
    lb.undo(); // undo replace "B"
    lb.undo(); // undo replace "A"
    assert_eq!(collect(&lb), vec!["a", "b", "c", "d", "e"]);
}

// ── Test 11: from_str validator carried through, from_reader too ──────────────

#[test]
fn from_reader_with_ascii_validator_rejects_non_ascii() {
    let v: Arc<dyn EncodingValidator> = Arc::new(AsciiValidator);
    // Input with a non-ASCII byte sequence (UTF-8 for U+00E9 é = 0xC3 0xA9).
    let input = b"hello\ncaf\xC3\xA9\nworld\n" as &[u8];
    let result = LineBuffer::from_reader(std::io::Cursor::new(input), Some(v));
    assert!(
        result.is_err(),
        "from_reader must propagate AsciiValidator rejection"
    );
}

#[test]
fn from_reader_with_ascii_validator_accepts_ascii() {
    let v: Arc<dyn EncodingValidator> = Arc::new(AsciiValidator);
    let input = b"hello\nworld\nfoo\n" as &[u8];
    let mut lb = LineBuffer::from_reader(std::io::Cursor::new(input), Some(v))
        .expect("pure ASCII must succeed");
    assert_eq!(lb.line_count(), 3);
    lb.insert_line(3, "bar").unwrap();
    assert_rejected(lb.insert_line(4, "\u{00E9}"), "\u{00E9}");
    assert_eq!(lb.line_count(), 4);
}

// ── Test 12: error description identifies offending character ─────────────────

#[test]
fn error_description_identifies_character() {
    let mut lb = ascii_buf("abc\n");

    // Insert a line with U+4E2D (中, CJK Unified Ideograph).
    let err = lb.insert_line(1, "\u{4E2D}").err().unwrap();
    assert!(
        err.description.contains("U+4E2D"),
        "description should name U+4E2D, got: {}",
        err.description
    );

    // Replace with U+1F986 (🦆, U+1F986).
    let err = lb.replace_line(0, "hi \u{1F986}").err().unwrap();
    assert!(
        err.description.contains("U+1F986"),
        "description should name U+1F986, got: {}",
        err.description
    );
}
