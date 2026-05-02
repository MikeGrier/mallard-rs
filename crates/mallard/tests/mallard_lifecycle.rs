// Copyright (c) 2026, Michael Grier.
//! ML-44: Integration test suite — >=10 distinct lifecycle scenarios.
//!
//! Each test exercises a full lifecycle: load via `from_str`, multi-step edits,
//! undo/redo, fork, independent fork edits, write-back, and output comparison.

use mallard::{Branch, LineBuffer, LineBufferView};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn collect(lb: &LineBuffer) -> Vec<String> {
    lb.iter_lines().map(|s| s.to_string()).collect()
}

/// Collect all lines from a `Branch` as owned `String` values.
/// `Branch` has no `iter_lines`; iterate via `get_line`.
fn collect_branch(b: &Branch) -> Vec<String> {
    (0..b.line_count())
        .map(|i| b.get_line(i).unwrap().to_string())
        .collect()
}

fn write_lf(lb: &LineBuffer) -> String {
    let mut out = Vec::new();
    lb.write_lines(&mut out, "\n").unwrap();
    String::from_utf8(out).unwrap()
}

fn write_crlf(lb: &LineBuffer) -> String {
    let mut out = Vec::new();
    lb.write_lines(&mut out, "\r\n").unwrap();
    String::from_utf8(out).unwrap()
}

// ── Scenario 1: insert + undo + redo + write ──────────────────────────────────

#[test]
fn scenario_insert_undo_redo_write() {
    let mut lb = LineBuffer::from_str("alpha\nbeta\ngamma\n", None).unwrap();
    assert_eq!(lb.line_count(), 3);

    lb.insert_line(3, "delta").unwrap();
    lb.insert_line(4, "epsilon").unwrap();
    assert_eq!(lb.line_count(), 5);

    lb.undo(); // removes epsilon
    assert_eq!(lb.line_count(), 4);
    assert_eq!(lb.get_line(3).as_deref(), Some("delta"));

    lb.undo(); // removes delta
    assert_eq!(lb.line_count(), 3);

    lb.redo(); // re-adds delta
    assert_eq!(lb.line_count(), 4);
    assert_eq!(lb.get_line(3).as_deref(), Some("delta"));

    let out = write_lf(&lb);
    assert_eq!(out, "alpha\nbeta\ngamma\ndelta");
}

// ── Scenario 2: replace + undo + verify original restored ────────────────────

#[test]
fn scenario_replace_undo_restores_original() {
    let mut lb = LineBuffer::from_str("line1\nline2\nline3\n", None).unwrap();

    lb.replace_line(1, "REPLACED").unwrap();
    assert_eq!(lb.get_line(1).as_deref(), Some("REPLACED"));

    lb.undo();
    assert_eq!(lb.get_line(1).as_deref(), Some("line2"));
    assert_eq!(lb.line_count(), 3);
    assert_eq!(write_lf(&lb), "line1\nline2\nline3");
}

// ── Scenario 3: delete + undo + redo ─────────────────────────────────────────

#[test]
fn scenario_delete_undo_redo() {
    let mut lb = LineBuffer::from_str("a\nb\nc\nd\ne\n", None).unwrap();
    assert_eq!(lb.line_count(), 5);

    lb.delete_line(2); // removes "c"
    assert_eq!(lb.line_count(), 4);
    assert_eq!(lb.get_line(2).as_deref(), Some("d"));

    lb.undo();
    assert_eq!(lb.line_count(), 5);
    assert_eq!(lb.get_line(2).as_deref(), Some("c"));

    lb.redo();
    assert_eq!(lb.line_count(), 4);
    assert_eq!(collect(&lb), vec!["a", "b", "d", "e"]);
}

// ── Scenario 4: fork + independent edits + write both ────────────────────────

#[test]
fn scenario_fork_independent_edits_write_both() {
    let mut base = LineBuffer::from_str("base1\nbase2\nbase3\n", None).unwrap();
    let mut fork = base.branch();

    base.insert_line(3, "base-extra").unwrap();
    fork.insert_line(0, "fork-prefix", None).unwrap();

    // base: 4 lines, fork: 4 lines, different content
    assert_eq!(base.line_count(), 4);
    assert_eq!(write_lf(&base), "base1\nbase2\nbase3\nbase-extra");

    let fork_lines = collect_branch(&fork);
    assert_eq!(fork_lines, vec!["fork-prefix", "base1", "base2", "base3"]);
}

// ── Scenario 5: multi-step insert/replace/delete then undo chain ──────────────

#[test]
fn scenario_multi_step_full_undo_chain() {
    let mut lb = LineBuffer::from_str("x\ny\nz\n", None).unwrap();
    lb.insert_line(3, "w").unwrap(); // [x,y,z,w]
    lb.replace_line(0, "X").unwrap(); // [X,y,z,w]
    lb.delete_line(2); // [X,y,w]
    assert_eq!(lb.line_count(), 3);
    assert_eq!(collect(&lb), vec!["X", "y", "w"]);

    lb.undo(); // undo delete -> [X,y,z,w]
    assert_eq!(lb.line_count(), 4);
    lb.undo(); // undo replace -> [x,y,z,w]
    assert_eq!(lb.get_line(0).as_deref(), Some("x"));
    lb.undo(); // undo insert -> [x,y,z]
    assert_eq!(lb.line_count(), 3);
    assert_eq!(collect(&lb), vec!["x", "y", "z"]);
}

// ── Scenario 6: fork after edits, fork reflects parent's committed state ──────

#[test]
fn scenario_fork_after_edits_reflects_parent_state() {
    let mut lb = LineBuffer::from_str("p\nq\nr\n", None).unwrap();
    lb.replace_line(1, "Q").unwrap();
    lb.insert_line(3, "s").unwrap();

    let fork = lb.branch();
    assert_eq!(fork.line_count(), 4);
    let fork_lines = collect_branch(&fork);
    assert_eq!(fork_lines, vec!["p", "Q", "r", "s"]);

    // Parent undo does not affect fork
    let mut lb = lb;
    lb.undo();
    assert_eq!(lb.line_count(), 3);
    assert_eq!(fork.line_count(), 4); // fork unchanged
}

// ── Scenario 7: write LF vs CRLF produces different byte sequences ────────────

#[test]
fn scenario_write_lf_vs_crlf_differ() {
    let lb = LineBuffer::from_str("one\ntwo\nthree\n", None).unwrap();
    let lf_out = write_lf(&lb);
    let crlf_out = write_crlf(&lb);
    assert_eq!(lf_out, "one\ntwo\nthree");
    assert_eq!(crlf_out, "one\r\ntwo\r\nthree");
    assert_ne!(lf_out, crlf_out);
}

// ── Scenario 8: empty lines interleaved ──────────────────────────────────────

#[test]
fn scenario_empty_lines_interleaved() {
    let mut lb = LineBuffer::from_str("a\n\nb\n\nc\n", None).unwrap();
    assert_eq!(lb.line_count(), 5);
    assert_eq!(lb.get_line(1).as_deref(), Some(""));
    assert_eq!(lb.get_line(3).as_deref(), Some(""));

    lb.delete_line(3); // remove second empty line
    assert_eq!(lb.line_count(), 4);
    assert_eq!(collect(&lb), vec!["a", "", "b", "c"]);

    lb.undo();
    assert_eq!(lb.line_count(), 5);
    assert_eq!(collect(&lb), vec!["a", "", "b", "", "c"]);
}

// ── Scenario 9: unicode content roundtrip ─────────────────────────────────────

#[test]
fn scenario_unicode_roundtrip() {
    // Use explicit unicode escapes to avoid any source-encoding ambiguity.
    let japanese = "\u{65E5}\u{672C}\u{8A9E}"; // 日本語
    let chinese = "\u{4E2D}\u{6587}"; // 中文
    let korean = "\u{D55C}\u{AD6D}\u{C5B4}"; // 한국어
    let emoji = "\u{1F986}"; // 🦆
    let greek = "\u{03B1}\u{03B2}\u{03B3}"; // αβγ
    let input = format!("{japanese}\n{chinese}\n{korean}\n{emoji}\n{greek}\n");

    let mut lb = LineBuffer::from_str(&input, None).unwrap();
    assert_eq!(lb.line_count(), 5);
    assert_eq!(lb.get_line(0).as_deref(), Some(japanese));
    assert_eq!(lb.get_line(1).as_deref(), Some(chinese));
    assert_eq!(lb.get_line(2).as_deref(), Some(korean));
    assert_eq!(lb.get_line(3).as_deref(), Some(emoji));
    assert_eq!(lb.get_line(4).as_deref(), Some(greek));

    // Replace emoji line and verify undo restores original.
    lb.replace_line(3, "duck").unwrap();
    assert_eq!(lb.get_line(3).as_deref(), Some("duck"));

    lb.undo();
    assert_eq!(lb.get_line(3).as_deref(), Some(emoji));

    // Roundtrip write.
    let out = write_lf(&lb);
    let expected = format!("{japanese}\n{chinese}\n{korean}\n{emoji}\n{greek}");
    assert_eq!(out, expected);
}

// ── Scenario 10: LineBufferView trait through full lifecycle ──────────────────

#[test]
fn scenario_view_trait_through_full_lifecycle() {
    let mut lb = LineBuffer::from_str("one\ntwo\nthree\n", None).unwrap();
    lb.insert_line(3, "four").unwrap();
    lb.replace_line(0, "ONE").unwrap();

    let view: &dyn LineBufferView = &lb;
    assert_eq!(view.line_count(), 4);
    assert_eq!(view.get_line(0).as_deref(), Some("ONE"));
    assert_eq!(view.get_line(3).as_deref(), Some("four"));
    assert!(view.line_crc32(0).is_some());
    assert!(view.line_sha256(3).is_some());

    // CRC32 of "ONE" must differ from CRC32 of "one"
    let crc_one = view.line_crc32(0).unwrap();

    lb.undo(); // reverts replace
    let view2: &dyn LineBufferView = &lb;
    let crc_one_lower = view2.line_crc32(0).unwrap();
    assert_ne!(crc_one, crc_one_lower);
}

// ── Scenario 11: from_reader + edits + fork + write ──────────────────────────

#[test]
fn scenario_from_reader_edit_fork_write() {
    let input = b"r1\nr2\nr3\nr4\nr5\n" as &[u8];
    let mut lb = LineBuffer::from_reader(std::io::Cursor::new(input), None).unwrap();
    assert_eq!(lb.line_count(), 5);

    lb.replace_line(2, "R3-edited").unwrap();
    lb.insert_line(5, "r6").unwrap();

    let fork = lb.branch();

    lb.delete_line(0); // parent removes r1
    let parent_out = write_lf(&lb);
    assert_eq!(parent_out, "r2\nR3-edited\nr4\nr5\nr6");

    let fork_lines = collect_branch(&fork);
    assert_eq!(fork_lines, vec!["r1", "r2", "R3-edited", "r4", "r5", "r6"]);
}

// ── Scenario 12: redo becomes unavailable after a new edit ───────────────────

#[test]
fn scenario_redo_cleared_by_new_edit() {
    let mut lb = LineBuffer::from_str("a\nb\nc\n", None).unwrap();
    lb.insert_line(3, "d").unwrap();
    lb.undo(); // can redo "d"

    // New edit clears redo
    lb.insert_line(3, "e").unwrap();
    assert!(!lb.redo()); // redo stack cleared

    assert_eq!(lb.line_count(), 4);
    assert_eq!(lb.get_line(3).as_deref(), Some("e"));
    assert_eq!(collect(&lb), vec!["a", "b", "c", "e"]);
}

// ── Scenario 13: two forks from same parent diverge independently ─────────────

#[test]
fn scenario_two_forks_diverge_independently() {
    let base = LineBuffer::from_str("base\n", None).unwrap();
    let mut fork_a = base.branch();
    let mut fork_b = base.branch();

    fork_a.insert_line(1, "fork-a-line", None).unwrap();
    fork_b.insert_line(1, "fork-b-line", None).unwrap();
    fork_b.insert_line(2, "fork-b-extra", None).unwrap();

    assert_eq!(fork_a.line_count(), 2);
    assert_eq!(fork_b.line_count(), 3);

    let a_lines = collect_branch(&fork_a);
    let b_lines = collect_branch(&fork_b);
    assert_eq!(a_lines, vec!["base", "fork-a-line"]);
    assert_eq!(b_lines, vec!["base", "fork-b-line", "fork-b-extra"]);
}

// ── Scenario 14: load -> full delete cycle -> rebuild -> write ────────────────

#[test]
fn scenario_delete_all_then_rebuild() {
    let mut lb = LineBuffer::from_str("x\ny\nz\n", None).unwrap();
    lb.delete_line(2);
    lb.delete_line(1);
    lb.delete_line(0);
    assert_eq!(lb.line_count(), 0);

    lb.insert_line(0, "rebuilt-a").unwrap();
    lb.insert_line(1, "rebuilt-b").unwrap();
    assert_eq!(lb.line_count(), 2);
    assert_eq!(write_lf(&lb), "rebuilt-a\nrebuilt-b");

    // Undo rebuild
    lb.undo();
    lb.undo();
    assert_eq!(lb.line_count(), 0);

    // Undo deletes
    lb.undo();
    lb.undo();
    lb.undo();
    assert_eq!(lb.line_count(), 3);
    assert_eq!(collect(&lb), vec!["x", "y", "z"]);
}
