// ── ForcedEncodingValidator ───────────────────────────────────────────────────

/// An [`EncodingValidator`] that rejects lines containing characters that
/// cannot be faithfully represented in the target encoding.
///
/// Uses `encoding_rs`'s `encode_from_utf8_without_replacement` directly so
/// that unmappable code points are detected rather than silently substituted.
/// Pure-ASCII lines bypass the encoder entirely.
struct ForcedEncodingValidator {
    encoding: &'static Encoding,
}

impl EncodingValidator for ForcedEncodingValidator {
    fn validate(&self, line: &str) -> Result<(), EncodingError> {
        if line.is_ascii() {
            return Ok(());
        }
        let mut encoder = self.encoding.new_encoder();
        let cap = encoder
            .max_buffer_length_from_utf8_if_no_unmappables(line.len())
            .unwrap_or_else(|| line.len().saturating_mul(4).max(16));
        let mut out = vec![0u8; cap];
        match encoder.encode_from_utf8_without_replacement(line, &mut out, true) {
            (encoding_rs::EncoderResult::InputEmpty, _, _) => Ok(()),
            _ => Err(EncodingError {
                line: line.into(),
                description: format!(
                    "line contains characters that cannot be encoded in '{}'",
                    self.encoding.name()
                )
                .into_boxed_str(),
            }),
        }
    }
}

/// Construct an empty `LineBuffer` with a `ForcedEncodingValidator` installed.
fn empty_buf_with_enc(enc: &'static Encoding) -> LineBuffer {
    LineBuffer::new(Some(Arc::new(ForcedEncodingValidator { encoding: enc })))
}

// ── load_file / save ──────────────────────────────────────────────────────────
// Copyright (c) 2026, Michael Grier.

use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::{Parser, ValueEnum};
use encoding_rs::Encoding;
use mallard::{EncodingError, EncodingValidator, LineBuffer};

/// Line ending written when saving a file.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum LineEnding {
    Lf,
    Crlf,
    Cr,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// edlin — a simple line-oriented text editor.
#[derive(Debug, Parser)]
#[command(name = "edlin")]
struct Args {
    /// File to edit. If the file exists it is loaded on start; if absent or
    /// the file does not exist, an empty buffer is used. Also serves as the
    /// default save target unless `--output` is given.
    #[arg(long)]
    file: Option<PathBuf>,

    /// Override save target. When given, saves go here regardless of `--file`.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Read commands from this file instead of stdin. Lines are processed in
    /// order; EOF is treated as `q`.
    #[arg(long)]
    input_file: Option<PathBuf>,

    /// Line separator written when saving (default: lf).
    #[arg(long, default_value = "lf")]
    line_ending: LineEnding,

    /// Force a specific output encoding (e.g. `utf-8`, `windows-1252`,
    /// `shift_jis`).  When supplied, the buffer validator enforces that every
    /// line is representable in this encoding, and saves are written in this
    /// encoding.  `utf-8` is always available; other names are WHATWG
    /// encoding labels passed to `encoding_rs`.  An unrecognised label prints
    /// an error and exits.
    #[arg(long)]
    force_encoding: Option<String>,
}

const HELP: &str = "\
Commands (line numbers are 1-based):
  l           List all lines with line numbers
  p N         Print line N
  i N <text>  Insert <text> before line N (appends if N > line count)
  r N <text>  Replace line N with <text>
  d N         Delete line N
  u           Undo
  e           Redo
  s           Save to default output path
  s <path>    Save to given path
  q           Quit without saving
  ? or h      Print this help";

/// State threaded through the command loop.
struct Context<'a> {
    buf: &'a mut LineBuffer,
    default_output: Option<&'a Path>,
    line_ending: &'a str,
    /// When `--force-encoding` is given, saves are written in this encoding.
    /// `None` means use the plain UTF-8 path.
    force_encoding: Option<&'static Encoding>,
}

/// Loads `path` into a new `LineBuffer`.
///
/// When `encoding` is `Some`, the file bytes are decoded from that encoding
/// and a `ForcedEncodingValidator` is installed so subsequent edits are
/// constrained to the target encoding.  When `None`, the file is loaded as
/// plain UTF-8 via `LineBuffer::from_reader`.
///
/// Read or decode errors are printed to stderr and the function returns an
/// empty buffer.  Callers should not call this when the file does not exist.
fn load_file(path: &Path, encoding: Option<&'static Encoding>) -> LineBuffer {
    if let Some(enc) = encoding {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("edlin: load: {}: {}", path.display(), e);
                return empty_buf_with_enc(enc);
            }
        };
        // Decode with encoding_rs; unmappable bytes become U+FFFD.
        let (text, _used, _had_errors) = enc.decode(&bytes);
        let mut lb = LineBuffer::new(Some(Arc::new(ForcedEncodingValidator { encoding: enc })));
        for line in text.lines() {
            if let Err(e) = lb.insert_line(lb.line_count(), line) {
                eprintln!("edlin: load: line validation failed: {}", e);
            }
        }
        lb
    } else {
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("edlin: load: {}: {}", path.display(), e);
                return LineBuffer::new(None);
            }
        };
        match LineBuffer::from_reader(io::BufReader::new(file), None) {
            Ok(lb) => lb,
            Err(e) => {
                eprintln!("edlin: load: {}: {}", path.display(), e);
                LineBuffer::new(None)
            }
        }
    }
}

/// Save the buffer to `path` with the configured line ending.
///
/// When `encoding` is `Some`, each line and line-ending are encoded into the
/// target encoding's byte representation before writing.  All WHATWG encodings
/// are ASCII supersets, so line-ending bytes are stable across encodings.
/// When `None`, the buffer is written as plain UTF-8.
fn save(buf: &LineBuffer, path: &Path, line_ending: &str, encoding: Option<&'static Encoding>) {
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("edlin: save: {}: {}", path.display(), e);
            return;
        }
    };
    let mut writer = io::BufWriter::new(file);

    if let Some(enc) = encoding {
        // Encode each line + terminator into the target encoding.
        let terminator_bytes = line_ending.as_bytes(); // safe: always ASCII
        let count = buf.line_count();
        for i in 0..count {
            let line = buf.get_line(i).expect("line index in range");
            let mut encoder = enc.new_encoder();
            let capacity = encoder
                .max_buffer_length_from_utf8_if_no_unmappables(line.len())
                .unwrap_or_else(|| line.len().saturating_mul(4).max(16));
            let mut out = vec![0u8; capacity];
            let (result, _read, written) =
                encoder.encode_from_utf8_without_replacement(&line, &mut out, true);
            match result {
                encoding_rs::EncoderResult::InputEmpty => {
                    if let Err(e) = writer.write_all(&out[..written]) {
                        eprintln!("edlin: save: {}: {}", path.display(), e);
                        return;
                    }
                }
                _ => {
                    eprintln!(
                        "edlin: save: line {} cannot be encoded in '{}'; save aborted",
                        i + 1,
                        enc.name()
                    );
                    return;
                }
            }
            if let Err(e) = writer.write_all(terminator_bytes) {
                eprintln!("edlin: save: {}: {}", path.display(), e);
                return;
            }
        }
    } else if let Err(e) = buf.write_lines(&mut writer, line_ending) {
        eprintln!("edlin: save: {}: {}", path.display(), e);
        return;
    }
    println!("saved to {}", path.display());
}

/// Parse a 1-based line number token, returning a 0-based index.
fn parse_line_number(token: &str) -> Option<usize> {
    match token.parse::<usize>() {
        Ok(0) => {
            eprintln!("edlin: line numbers are 1-based; 0 is not valid");
            None
        }
        Ok(n) => Some(n - 1),
        Err(_) => {
            eprintln!("edlin: invalid line number: {:?}", token);
            None
        }
    }
}

/// Processes one command line. Returns `false` to quit.
fn dispatch(ctx: &mut Context<'_>, line: &str) -> bool {
    let trimmed = line.trim_end_matches([' ', '\t', '\r', '\n']);
    // Split into at most 3 parts: cmd, first-arg, rest (rest may contain spaces).
    let mut parts = trimmed.splitn(3, ' ');
    let cmd = parts.next().unwrap_or("");

    match cmd {
        "q" => return false,

        "?" | "h" => println!("{}", HELP),

        "l" => {
            if ctx.buf.line_count() == 0 {
                println!("(empty)");
            } else {
                for (i, ln) in ctx.buf.iter_lines().enumerate() {
                    println!("{:>6}  {}", i + 1, ln);
                }
            }
        }

        "p" => {
            let Some(n_str) = parts.next() else {
                eprintln!("edlin: p: usage: p N");
                return true;
            };
            let Some(idx) = parse_line_number(n_str) else {
                return true;
            };
            match ctx.buf.get_line(idx) {
                Some(content) => println!("{}", content),
                None => eprintln!("edlin: p: line {} out of range", idx + 1),
            }
        }

        "i" => {
            let Some(n_str) = parts.next() else {
                eprintln!("edlin: i: usage: i N <text>");
                return true;
            };
            let text = parts.next().unwrap_or("");
            let Some(idx) = parse_line_number(n_str) else {
                return true;
            };
            // Clamp so that N > line_count appends rather than panics.
            let idx = idx.min(ctx.buf.line_count());
            if let Err(e) = ctx.buf.insert_line(idx, text) {
                eprintln!("edlin: i: {}", e);
            }
        }

        "r" => {
            let Some(n_str) = parts.next() else {
                eprintln!("edlin: r: usage: r N <text>");
                return true;
            };
            let text = parts.next().unwrap_or("");
            let Some(idx) = parse_line_number(n_str) else {
                return true;
            };
            if let Err(e) = ctx.buf.replace_line(idx, text) {
                eprintln!(
                    "edlin: r: line {} out of range (line_count = {}): {}",
                    idx + 1,
                    ctx.buf.line_count(),
                    e
                );
            }
        }

        "d" => {
            let Some(n_str) = parts.next() else {
                eprintln!("edlin: d: usage: d N");
                return true;
            };
            let Some(idx) = parse_line_number(n_str) else {
                return true;
            };
            if !ctx.buf.delete_line(idx) {
                eprintln!("edlin: d: line {} out of range", idx + 1);
            }
        }

        "u" => {
            if !ctx.buf.undo() {
                eprintln!("edlin: nothing to undo");
            }
        }

        "e" => {
            if !ctx.buf.redo() {
                eprintln!("edlin: nothing to redo");
            }
        }

        "s" => {
            let path: Option<PathBuf> = parts
                .next()
                .map(PathBuf::from)
                .or_else(|| ctx.default_output.map(|p| p.to_path_buf()));
            match path {
                Some(p) => save(ctx.buf, &p, ctx.line_ending, ctx.force_encoding),
                None => eprintln!("edlin: s: no output path (use --file, --output, or s <path>)"),
            }
        }

        "" => {} // blank lines are silently ignored

        _ => eprintln!("edlin: unrecognised command: {:?}", cmd),
    }

    true
}

/// Runs the command loop, reading from `reader`. EOF is treated as `q`.
fn run_loop(ctx: &mut Context<'_>, reader: impl BufRead) {
    for line in reader.lines() {
        match line {
            Err(e) => {
                eprintln!("edlin: read error: {}", e);
                break;
            }
            Ok(text) => {
                if !dispatch(ctx, &text) {
                    break;
                }
            }
        }
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // Resolve --force-encoding to a &'static Encoding, or exit on bad label.
    let force_encoding: Option<&'static Encoding> = match &args.force_encoding {
        None => None,
        Some(label) => match Encoding::for_label(label.as_bytes()) {
            Some(enc) => Some(enc),
            None => {
                eprintln!("edlin: unknown encoding label: {:?}", label);
                std::process::exit(1);
            }
        },
    };

    let mut buf = if let Some(ref path) = args.file {
        if path.exists() {
            load_file(path, force_encoding)
        } else {
            match force_encoding {
                None => LineBuffer::new(None),
                Some(enc) => empty_buf_with_enc(enc),
            }
        }
    } else {
        match force_encoding {
            None => LineBuffer::new(None),
            Some(enc) => empty_buf_with_enc(enc),
        }
    };

    let default_output: Option<PathBuf> = args.output.clone().or_else(|| args.file.clone());
    let mut ctx = Context {
        buf: &mut buf,
        default_output: default_output.as_deref(),
        line_ending: args.line_ending.as_str(),
        force_encoding,
    };

    if let Some(ref path) = args.input_file {
        let f = std::fs::File::open(path)
            .map_err(|e| io::Error::other(format!("--input-file: {}: {}", path.display(), e)))?;
        run_loop(&mut ctx, io::BufReader::new(f));
    } else {
        run_loop(&mut ctx, io::BufReader::new(io::stdin()));
    }

    Ok(())
}
