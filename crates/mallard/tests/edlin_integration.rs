// Copyright (c) 2026, Michael Grier.

//! Integration tests for edlin Phase 1 (ML-34).
//!
//! Each test writes a command script to a temp file, invokes the `edlin` binary
//! via `std::process::Command`, and asserts the output file or stdout/stderr.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a unique path in the system temp directory.
fn unique_path(suffix: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "edlin_test_{pid}_{id}_{suffix}",
        pid = std::process::id()
    ))
}

/// Write command lines (one per entry) to a temp file and return its path.
fn write_script(cmds: &[&str]) -> PathBuf {
    let path = unique_path("script.txt");
    std::fs::write(&path, cmds.join("\n")).expect("write script");
    path
}

/// Run edlin with the given argument list; return (stdout, stderr).
fn edlin(args: &[&str]) -> (String, String) {
    let bin = env!("CARGO_BIN_EXE_medlin");
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("invoke edlin");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (stdout, stderr)
}

/// Convenience: run edlin with --input-file script and --output out.
fn run(script: &Path, output: &Path, extra: &[&str]) -> (String, String) {
    let script_str = script.to_str().expect("script path");
    let output_str = output.to_str().expect("output path");
    let mut args = vec!["--input-file", script_str, "--output", output_str];
    args.extend_from_slice(extra);
    edlin(&args)
}

fn read_output(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|err| {
        panic!("failed to read output file {}: {err}", path.display())
    })
}

// ── 1: empty buffer save produces empty file ─────────────────────────────────

#[test]
fn empty_buffer_save_produces_empty_file() {
    let script = write_script(&["s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"");
}

// ── 2: insert 5 lines then save ──────────────────────────────────────────────

#[test]
fn insert_5_lines_then_save() {
    let script = write_script(&[
        "i 1 alpha",
        "i 2 beta",
        "i 3 gamma",
        "i 4 delta",
        "i 5 epsilon",
        "s",
        "q",
    ]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"alpha\nbeta\ngamma\ndelta\nepsilon");
}

// ── 3: insert then delete then save ──────────────────────────────────────────

#[test]
fn insert_then_delete_then_save() {
    let script = write_script(&["i 1 keep", "i 2 remove", "i 3 also_keep", "d 2", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"keep\nalso_keep");
}

// ── 4: insert then undo then save ────────────────────────────────────────────

#[test]
fn insert_then_undo_then_save() {
    let script = write_script(&["i 1 first", "i 2 second", "u", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"first");
}

// ── 5: replace a line then save ──────────────────────────────────────────────

#[test]
fn replace_line_then_save() {
    let script = write_script(&["i 1 original", "r 1 updated", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"updated");
}

// ── 6: multi-step undo to verify state ───────────────────────────────────────

#[test]
fn multi_step_undo_verifies_state() {
    let script = write_script(&["i 1 a", "i 2 b", "i 3 c", "i 4 d", "u", "u", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"a\nb");
}

// ── 7: p N prints correct line to stdout ─────────────────────────────────────

#[test]
fn p_prints_correct_line_to_stdout() {
    let script = write_script(&["i 1 hello", "i 2 world", "p 2", "q"]);
    let output = unique_path("out.txt");
    let (stdout, _) = run(&script, &output, &[]);
    assert!(stdout.contains("world"), "stdout was: {:?}", stdout);
    assert!(!stdout.contains("hello"), "stdout was: {:?}", stdout);
}

// ── 8: l lists all lines with correct numbers ────────────────────────────────

#[test]
fn l_lists_all_lines_with_correct_numbers() {
    let script = write_script(&["i 1 foo", "i 2 bar", "l", "q"]);
    let output = unique_path("out.txt");
    let (stdout, _) = run(&script, &output, &[]);
    assert!(stdout.contains("1"), "stdout: {:?}", stdout);
    assert!(stdout.contains("foo"), "stdout: {:?}", stdout);
    assert!(stdout.contains("2"), "stdout: {:?}", stdout);
    assert!(stdout.contains("bar"), "stdout: {:?}", stdout);
}

// ── 9: unknown command is tolerated ──────────────────────────────────────────

#[test]
fn unknown_command_is_tolerated() {
    let script = write_script(&["i 1 line", "zzz_unknown", "s", "q"]);
    let output = unique_path("out.txt");
    let (_stdout, stderr) = run(&script, &output, &[]);
    assert!(stderr.contains("unrecognised"), "stderr was: {:?}", stderr);
    // Buffer was still saved correctly.
    assert_eq!(read_output(&output), b"line");
}

// ── 10: --line-ending crlf produces CRLF-separated output ────────────────────

#[test]
fn line_ending_crlf_produces_crlf_output() {
    let script = write_script(&["i 1 alpha", "i 2 beta", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &["--line-ending", "crlf"]);
    assert_eq!(read_output(&output), b"alpha\r\nbeta");
}

// ── 11: --line-ending cr produces CR-separated output ────────────────────────

#[test]
fn line_ending_cr_produces_cr_output() {
    let script = write_script(&["i 1 alpha", "i 2 beta", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &["--line-ending", "cr"]);
    assert_eq!(read_output(&output), b"alpha\rbeta");
}

// ── 12: --file loading an existing file matches original content ──────────────

#[test]
fn file_loading_existing_file_matches_original() {
    let src = unique_path("src.txt");
    std::fs::write(&src, b"line1\nline2\nline3").expect("write src");

    let script = write_script(&["l", "q"]);
    let output = unique_path("out.txt");

    let script_str = script.to_str().expect("script path");
    let output_str = output.to_str().expect("output path");
    let src_str = src.to_str().expect("src path");
    let (stdout, _) = edlin(&[
        "--file",
        src_str,
        "--input-file",
        script_str,
        "--output",
        output_str,
    ]);
    assert!(stdout.contains("line1"), "stdout: {:?}", stdout);
    assert!(stdout.contains("line2"), "stdout: {:?}", stdout);
    assert!(stdout.contains("line3"), "stdout: {:?}", stdout);
}

// ── 13: loading a non-existent --file starts empty ───────────────────────────

#[test]
fn file_loading_nonexistent_starts_empty() {
    let nonexistent = unique_path("does_not_exist.txt");
    let script = write_script(&["s", "q"]);
    let output = unique_path("out.txt");

    let script_str = script.to_str().expect("script path");
    let output_str = output.to_str().expect("output path");
    let ne_str = nonexistent.to_str().expect("nonexistent path");
    edlin(&[
        "--file",
        ne_str,
        "--input-file",
        script_str,
        "--output",
        output_str,
    ]);
    assert_eq!(read_output(&output), b"");
}

// ── 14: save to --output path distinct from --file ───────────────────────────

#[test]
fn save_to_output_path_distinct_from_file() {
    let src = unique_path("src.txt");
    std::fs::write(&src, b"original").expect("write src");

    let script = write_script(&["i 1 extra", "s", "q"]);
    let output = unique_path("out.txt");

    let script_str = script.to_str().expect("script path");
    let output_str = output.to_str().expect("output path");
    let src_str = src.to_str().expect("src path");
    edlin(&[
        "--file",
        src_str,
        "--output",
        output_str,
        "--input-file",
        script_str,
    ]);

    // Original --file is untouched.
    assert_eq!(std::fs::read(&src).unwrap(), b"original");
    // --output receives the edited content.
    let out_bytes = read_output(&output);
    assert!(
        out_bytes.windows(5).any(|w| w == b"extra"),
        "output: {:?}",
        String::from_utf8_lossy(&out_bytes)
    );
}

// ── 15: redo after undo restores line ────────────────────────────────────────

#[test]
fn redo_after_undo_restores_line() {
    let script = write_script(&["i 1 first", "i 2 second", "u", "e", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), b"first\nsecond");
}

// ── 16: delete out-of-range is tolerated ─────────────────────────────────────

#[test]
fn delete_out_of_range_is_tolerated() {
    let script = write_script(&["i 1 only", "d 99", "s", "q"]);
    let output = unique_path("out.txt");
    let (_stdout, stderr) = run(&script, &output, &[]);
    assert!(stderr.contains("out of range"), "stderr: {:?}", stderr);
    assert_eq!(read_output(&output), b"only");
}

// ── 17: insert Unicode content round-trips correctly ─────────────────────────

#[test]
fn insert_unicode_content_roundtrip() {
    let script = write_script(&["i 1 日本語", "i 2 emoji 🦆", "s", "q"]);
    let output = unique_path("out.txt");
    run(&script, &output, &[]);
    assert_eq!(read_output(&output), "日本語\nemoji 🦆".as_bytes());
}

// ── 18: l on empty buffer prints marker ──────────────────────────────────────

#[test]
fn l_on_empty_buffer_prints_empty_marker() {
    let script = write_script(&["l", "q"]);
    let output = unique_path("out.txt");
    let (stdout, _) = run(&script, &output, &[]);
    assert!(stdout.contains("(empty)"), "stdout: {:?}", stdout);
}

// ── 19: loading file with CRLF line endings splits correctly ─────────────────

#[test]
fn file_loading_crlf_splits_correctly() {
    let src = unique_path("crlf_src.txt");
    std::fs::write(&src, b"first\r\nsecond\r\nthird").expect("write src");

    let script = write_script(&["s", "q"]);
    let output = unique_path("out.txt");

    let script_str = script.to_str().expect("script path");
    let output_str = output.to_str().expect("output path");
    let src_str = src.to_str().expect("src path");
    edlin(&[
        "--file",
        src_str,
        "--input-file",
        script_str,
        "--output",
        output_str,
        "--line-ending",
        "lf",
    ]);
    // Loaded with CRLF, saved with LF — terminators stripped on load.
    assert_eq!(read_output(&output), b"first\nsecond\nthird");
}

// ── 20: --force-encoding utf-8 CRLF roundtrip with edit ─────────────────────
//
// ML-55: load a UTF-8 CRLF file, replace one line, save with --force-encoding
// utf-8 --line-ending crlf; verify both lines have CRLF terminators and the
// replaced content is correct.

#[test]
fn force_encoding_utf8_crlf_roundtrip_with_edit() {
    let src = unique_path("src_utf8_crlf.txt");
    std::fs::write(&src, b"line1\r\nline2\r\n").expect("write src");

    let script = write_script(&["r 2 replaced", "s", "q"]);
    let output = unique_path("out_utf8_crlf.txt");

    edlin(&[
        "--file",
        src.to_str().unwrap(),
        "--input-file",
        script.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--force-encoding",
        "utf-8",
        "--line-ending",
        "crlf",
    ]);

    // The encoding save path writes a terminator after every line, including
    // the last.  Line 1 unchanged; line 2 replaced.
    assert_eq!(read_output(&output), b"line1\r\nreplaced\r\n");
}

// ── 21: --force-encoding utf-8 CRLF idempotency ─────────────────────────────
//
// ML-55: after a force-encoding save the output can be reloaded and re-saved
// without changing any bytes (idempotency).  Verified for UTF-8 here; see
// test 22 for the Windows-1252 variant.

#[test]
fn force_encoding_utf8_crlf_idempotency() {
    let src = unique_path("src_utf8_crlf_idem.txt");
    std::fs::write(&src, b"alpha\r\nbeta\r\n").expect("write src");

    // First pass — replace line 2.
    let script1 = write_script(&["r 2 gamma", "s", "q"]);
    let output1 = unique_path("out_utf8_crlf_idem1.txt");
    edlin(&[
        "--file",
        src.to_str().unwrap(),
        "--input-file",
        script1.to_str().unwrap(),
        "--output",
        output1.to_str().unwrap(),
        "--force-encoding",
        "utf-8",
        "--line-ending",
        "crlf",
    ]);
    let after_edit = read_output(&output1);
    assert_eq!(after_edit, b"alpha\r\ngamma\r\n", "first-pass output wrong");

    // Second pass (idempotency) — load the first output, make no edits, save.
    let script2 = write_script(&["s", "q"]);
    let output2 = unique_path("out_utf8_crlf_idem2.txt");
    edlin(&[
        "--file",
        output1.to_str().unwrap(),
        "--input-file",
        script2.to_str().unwrap(),
        "--output",
        output2.to_str().unwrap(),
        "--force-encoding",
        "utf-8",
        "--line-ending",
        "crlf",
    ]);
    let after_reload = read_output(&output2);
    assert_eq!(
        after_reload, after_edit,
        "idempotency pass output differs from first-pass"
    );
}

// ── 22: --force-encoding windows-1252 CRLF roundtrip + idempotency ──────────
//
// ML-55: load a Windows-1252 CRLF file containing Latin-1 characters, replace
// one line with other Latin-1 content, save with --force-encoding windows-1252
// --line-ending crlf; verify the output bytes are correct Windows-1252.
// Then reload the output and save again to verify idempotency.
//
// Encoding table (Windows-1252):
//   é = 0xE9   ï = 0xEF

#[test]
fn force_encoding_windows_1252_crlf_roundtrip_and_idempotency() {
    // Source: "café\r\nnaïve\r\n" in Windows-1252 bytes.
    let src = unique_path("src_w1252_crlf.txt");
    std::fs::write(&src, b"caf\xe9\r\nnai\xefve\r\n").expect("write src");

    // First pass: replace line 2 with "résumé" (also representable in Windows-1252).
    // The script is written as UTF-8; edlin reads it as UTF-8, stores the
    // UTF-8 string, and the ForcedEncodingValidator accepts it because both
    // é (U+00E9 → 0xE9) characters exist in Windows-1252.
    let script1 = write_script(&["r 2 r\u{e9}sum\u{e9}", "s", "q"]);
    let output1 = unique_path("out_w1252_crlf.txt");

    edlin(&[
        "--file",
        src.to_str().unwrap(),
        "--input-file",
        script1.to_str().unwrap(),
        "--output",
        output1.to_str().unwrap(),
        "--force-encoding",
        "windows-1252",
        "--line-ending",
        "crlf",
    ]);

    // Expected Windows-1252 bytes: "café\r\nrésumé\r\n"
    //   r=0x72  é=0xE9  s=0x73  u=0x75  m=0x6D  é=0xE9
    let expected: &[u8] = b"caf\xe9\r\nr\xe9sum\xe9\r\n";
    assert_eq!(
        read_output(&output1),
        expected,
        "first-pass output mismatch"
    );

    // Second pass (idempotency): reload the saved output, no edits, save again.
    // The trailing CRLF is absorbed by str::lines() (no spurious empty line),
    // so the resulting LineBuffer has the same two lines and the output is
    // byte-for-byte identical.
    let script2 = write_script(&["s", "q"]);
    let output2 = unique_path("out_w1252_crlf_idem.txt");

    edlin(&[
        "--file",
        output1.to_str().unwrap(),
        "--input-file",
        script2.to_str().unwrap(),
        "--output",
        output2.to_str().unwrap(),
        "--force-encoding",
        "windows-1252",
        "--line-ending",
        "crlf",
    ]);

    assert_eq!(
        read_output(&output2),
        expected,
        "idempotency-pass output differs from first-pass"
    );
}

// ── 23: --force-encoding windows-1252: ASCII-only lines, LF save ─────────────
//
// ML-56: Insert ASCII-only lines into a new buffer with --force-encoding
// windows-1252 and save; verify the output is valid Windows-1252 bytes
// (ASCII is identical in both UTF-8 and Windows-1252).

#[test]
fn force_encoding_windows_1252_ascii_only_save() {
    let script = write_script(&["i 1 hello", "i 2 world", "i 3 foo bar", "s", "q"]);
    let output = unique_path("out_w1252_ascii.txt");

    edlin(&[
        "--input-file",
        script.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--force-encoding",
        "windows-1252",
        "--line-ending",
        "lf",
    ]);

    // All ASCII: Windows-1252 bytes are identical to UTF-8.
    // The encoding save path writes a terminator after every line.
    assert_eq!(read_output(&output), b"hello\nworld\nfoo bar\n");
}

// ── 24: --force-encoding windows-1252: Latin-1 characters are accepted ────────
//
// ML-56: Characters such as é, ï, ñ exist in Windows-1252 and must
// be accepted by the validator and saved as their correct byte values.

#[test]
fn force_encoding_windows_1252_latin1_accepted() {
    // Insert "café" (é = U+00E9 → 0xE9 in Windows-1252).
    let script = write_script(&["i 1 caf\u{e9}", "s", "q"]);
    let output = unique_path("out_w1252_latin1.txt");

    edlin(&[
        "--input-file",
        script.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--force-encoding",
        "windows-1252",
        "--line-ending",
        "lf",
    ]);

    // "café\n" encoded as Windows-1252: 63 61 66 E9 0A
    assert_eq!(read_output(&output), b"caf\xe9\n");
}

// ── 25: --force-encoding windows-1252: non-Latin-1 insert is rejected ─────────
//
// ML-56: Attempting to insert a character outside Windows-1252 (e.g. Japanese
// or emoji) must be rejected before saving.  The buffer is not written.

#[test]
fn force_encoding_windows_1252_non_latin1_rejected() {
    // Try to insert Japanese text then save.
    // The validator must reject the insert; the save line should never execute.
    let script = write_script(&["i 1 \u{6771}\u{4eac}", "s", "q"]);
    let output = unique_path("out_w1252_rejected.txt");

    let (_stdout, stderr) = edlin(&[
        "--input-file",
        script.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--force-encoding",
        "windows-1252",
    ]);

    // The insert must have been rejected — stderr should mention a failure.
    assert!(
        stderr.contains("windows-1252") || stderr.contains("edlin"),
        "expected rejection message in stderr, got: {:?}",
        stderr
    );
    // The output file should either not exist or be empty (no valid insert).
    let bytes = read_output(&output);
    assert!(
        bytes.is_empty(),
        "expected empty output after rejected insert; got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

// ── 26: --force-encoding windows-1252: mixed accepted/rejected inserts ─────────
//
// ML-56: Insert one accepted line then attempt a rejected line.  Only the
// accepted line must appear in the saved output.

#[test]
fn force_encoding_windows_1252_partial_rejection() {
    // "café" is valid; "東京" is not.  The invalid insert is quietly rejected.
    let script = write_script(&["i 1 caf\u{e9}", "i 2 \u{6771}\u{4eac}", "s", "q"]);
    let output = unique_path("out_w1252_partial.txt");

    edlin(&[
        "--input-file",
        script.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--force-encoding",
        "windows-1252",
        "--line-ending",
        "lf",
    ]);

    // Only the first (valid) line appears in the output.
    assert_eq!(read_output(&output), b"caf\xe9\n");
}

// ── 27: unknown --force-encoding label exits with non-zero status ─────────────
//
// ML-56 (edge case): an unrecognised encoding label must cause edlin to print
// a message and exit with a non-zero status code without writing any output.

#[test]
fn force_encoding_unknown_label_exits_nonzero() {
    let script = write_script(&["i 1 test", "s", "q"]);
    let output = unique_path("out_unknown_enc.txt");

    let bin = env!("CARGO_BIN_EXE_medlin");
    let result = std::process::Command::new(bin)
        .args([
            "--input-file",
            script.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--force-encoding",
            "not-a-real-encoding-xyz",
        ])
        .output()
        .expect("invoke edlin");

    assert!(
        !result.status.success(),
        "edlin should exit non-zero for unknown encoding"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("not-a-real-encoding-xyz") || stderr.contains("unknown"),
        "stderr should mention the bad label: {:?}",
        stderr
    );
}
