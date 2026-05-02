// Copyright (c) 2026, Michael Grier.
//! ML-46: Integration test — large buffer performance.
//!
//! Parameters (user-specified, see ML-46 plan notes):
//!   - 500,000 lines loaded
//!   - 20,000 operations (insert/replace/delete interleaved)
//!   - 10,000 undo steps
//!   - Time limit: 10 seconds
//!
//! Positions are chosen with a deterministic LCG so the test is reproducible.

use std::{fmt::Write as FmtWrite, time::Instant};

use mallard::LineBuffer;

const LINES: usize = 500_000;
const OPS: usize = 20_000;
const UNDO_STEPS: usize = 10_000;
const TIME_LIMIT_SECS: f64 = 10.0;

/// Deterministic LCG (Knuth multiplicative, 64-bit).
/// Returns the next state; caller extracts a position from it.
fn lcg_next(state: u64) -> u64 {
    state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

#[test]
fn large_buffer_performance() {
    // ── 1. Build 500,000-line input string ───────────────────────────────────
    let mut input = String::with_capacity(LINES * 14);
    for i in 0..LINES {
        writeln!(input, "line-{:07}", i).expect("fmt write failed");
    }

    let start = Instant::now();

    // ── 2. Load ───────────────────────────────────────────────────────────────
    let mut lb = LineBuffer::from_str(&input, None).expect("from_str must succeed");
    assert_eq!(lb.line_count(), LINES);

    // ── 3. 20,000 operations ──────────────────────────────────────────────────
    //
    // Pattern (i % 3):
    //   0 = replace  (no line-count change)
    //   1 = insert   (+1)
    //   2 = delete   (−1)
    //
    // After all 20,000 ops the net line-count change is:
    //   inserts (6 667) − deletes (6 666) = +1  →  final = LINES + 1
    //
    // After undoing the last UNDO_STEPS ops (i = 10 000 .. 19 999):
    //   inserts in range: 3 334  (each undo removes a line)
    //   deletes in range: 3 333  (each undo restores a line)
    //   net from undo: −3 334 + 3 333 = −1  →  final = LINES + 1 − 1 = LINES

    let mut lcg: u64 = 0xDEAD_BEEF_CAFE_1234;
    let mut count = LINES; // tracks expected line_count

    for i in 0..OPS {
        lcg = lcg_next(lcg);
        let pos = (lcg >> 33) as usize; // upper 31 bits

        match i % 3 {
            0 => {
                // replace — position must be in [0, count)
                let n = pos % count;
                lb.replace_line(n, "perf-replace")
                    .expect("replace must succeed");
            }
            1 => {
                // insert — position in [0, count]
                let n = pos % (count + 1);
                lb.insert_line(n, "perf-insert")
                    .expect("insert must succeed");
                count += 1;
            }
            _ => {
                // delete — position in [0, count)
                let n = pos % count;
                let removed = lb.delete_line(n);
                assert!(removed, "delete must succeed (count = {})", count);
                count -= 1;
            }
        }
    }

    // ── 4. Undo 10,000 steps ──────────────────────────────────────────────────
    for step in 0..UNDO_STEPS {
        assert!(lb.undo(), "undo step {} must succeed", step);
    }

    // ── 5. Verify line count ──────────────────────────────────────────────────
    assert_eq!(
        lb.line_count(),
        LINES,
        "after {} ops then {} undos, line_count must equal LINES",
        OPS,
        UNDO_STEPS
    );

    // ── 6. Write back ─────────────────────────────────────────────────────────
    let mut out: Vec<u8> = Vec::with_capacity(LINES * 14);
    lb.write_lines(&mut out, "\n")
        .expect("write_lines must succeed");
    drop(out);

    // ── 7. Timing gate ────────────────────────────────────────────────────────
    let elapsed = start.elapsed().as_secs_f64();
    assert!(
        elapsed < TIME_LIMIT_SECS,
        "performance test took {:.2}s, limit is {}s",
        elapsed,
        TIME_LIMIT_SECS
    );
}
