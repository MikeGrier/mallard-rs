# mallard Design Notes

## Green/red model applied to lines

mallard adapts the green/red (immutable/mutable) versioning model familiar from
compiler IR work, applying it at line granularity instead of syntax-tree node
granularity.

The **green** side (`GreenPool`, `GreenLine`, `GreenLineId`) is append-only and
immutable.  Every distinct line content string is interned once and assigned a
`GreenLineId` (a `u32` index).  The pool grows monotonically — nothing is ever
removed or changed.  Because ids are stable, any snapshot of the ordered id
sequence fully defines a version of the file without copying the content strings.

The **red** side is the mutable `Branch` and `LineBuffer`.  A `Branch` is just
a `Vec<GreenLineId>` plus undo/redo stacks.  An edit creates a new id (if the
line is new) and rewrites a small part of the vec; the old ids remain valid in
the undo stack and in any other branch that holds references to them.

## GreenPool append-only guarantee and why it enables cheap branching

Because the pool never removes or modifies an entry, a fork is a shallow O(n)
clone of the `Vec<GreenLineId>` — the content strings behind those ids are
shared at zero cost.  There is no reference-counting of individual lines; the
pool itself serves as the shared arena.

Cheap forking is the key enabler for mallard's branch model: a `LineBuffer` can
spawn dozens of independent `Branch` views of the same content with no
allocations beyond the id vectors.

## Encoding validator policy

mallard applies validation **eagerly**, at the point every line is offered to
the buffer, whether during initial load (`from_str`, `from_reader`, `from_source`)
or during subsequent edits (`insert_line`, `replace_line`).  A rejected line
leaves the buffer state completely unchanged.

This policy (informally "Option A") was chosen because:

- Errors surface at the earliest possible moment, closest to the user action
  that caused them.
- There is no deferred state to reconcile: the buffer either accepted the edit
  or it didn't.
- Undo/redo stacks remain clean — rejected edits are never recorded.

The `EncodingValidator` trait is object-safe and `Send + Sync`, so validators
can be shared across threads and stored as `Arc<dyn EncodingValidator>`.

## Branch/fork/undo ownership model and why `Arc<Mutex<GreenPool>>` is used

A `LineBuffer` owns a root `Branch` and an `Arc<Mutex<GreenPool>>`.  When a
fork is created via `LineBuffer::branch()`, the returned `Branch` receives a
clone of the same `Arc`, so both the parent and all forks share one pool.

`Mutex` is required because `GreenPool::intern` mutates the pool (appending new
entries), and forks may be held simultaneously — possibly on different threads.
The lock is short-held (only during intern, not during read access), so
contention is minimal in practice.

Each `Branch` has its own undo and redo stacks.  Forks start with empty stacks
regardless of the parent's history; they are fully independent after creation.
Undoing an edit in the parent does not affect a fork, and vice versa.

## LineSource trait as the integration boundary for grouse

`LineSource` is a narrow, object-safe trait that accepts an ordered stream of
stripped lines from any upstream source and feeds them into a `LineBuffer`.
The implementor is responsible for stripping line terminators before invoking
the callback; mallard receives only bare content strings.

This makes grouse the natural implementor: grouse handles encoding-aware file
reading and terminator detection, while mallard remains encoding-agnostic at
the structural level.  The seam is well-defined — all encoding knowledge lives
on the grouse side of the `LineSource` implementation.

## LineBufferView as the interface for generic algorithms

`LineBufferView` exposes a read-only, index-based window into any ordered line
sequence.  Both `Branch` and `LineBuffer` implement it.  The trait provides:

- `line_count()` — number of lines
- `get_line(n)` — content of line n
- `line_crc32(n)`, `line_md5(n)`, `line_sha256(n)` — content digests

CRC32 is computed eagerly at intern time.  MD5 and SHA-256 are lazy and cached
on first access via `Cell<Option<…>>` interior mutability.

The intended consumers are diff, merge, and search algorithms that need to
compare large sequences efficiently.  A diff algorithm can use CRC32 equality
as a fast path to skip lines that are certainly equal, reserving full string
comparison only for CRC32 collisions.

## Relationship between mallard and redwing

mallard and redwing operate at different granularities and are not competing:

- **mallard** works at **line granularity** — the unit of editing is a whole
  line, interned as an immutable string.
- **redwing** works at **character/token granularity** — its versioning model
  tracks sub-line mutations.

A future integration could layer redwing on top of mallard (or alongside it)
for in-line editing within a line that mallard is managing as a unit.  The two
crates are designed to be composable, not substitutable.

## edlin as the reference usage of the full public API

`src/bin/medlin.rs` exercises every public-facing capability of mallard:

- `LineBuffer::from_reader` for file loading
- `insert_line`, `replace_line`, `delete_line` for editing
- `undo` / `redo` for history navigation
- `write_lines` for save-back with configurable line endings
- `iter_lines` / `get_line` for display
- `LineBuffer::new(None)` for an empty buffer with no validator

edlin intentionally uses no validator (passing `None`) to remain
encoding-neutral; the `--force-encoding` path (ML-53) will introduce a
grouse-backed validator for encoding-constrained saves.
