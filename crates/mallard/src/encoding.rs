// Copyright (c) 2026, Michael Grier.

//! Encoding validation policy for mallard line buffers.
//!
//! [`EncodingValidator`] is the trait that controls which Unicode strings are
//! acceptable as line content in a [`crate::LineBuffer`].  Mallard stores all
//! content as UTF-8 (`&str` / `Arc<str>`); validators decide which subset of
//! UTF-8 is legal for a particular use-case (e.g. ASCII-only files, files that
//! must round-trip through a legacy encoding, etc.).
//!
//! Two built-in validators are provided:
//! - [`Utf8Validator`] — accepts every string unconditionally.
//! - [`AsciiValidator`] — rejects any string containing a code point above U+007F.
//!
//! Additional validators (e.g. wrapping `encoding_rs` codecs) can be supplied
//! by callers; they need only implement [`EncodingValidator`].

use std::fmt;

// ── EncodingError ─────────────────────────────────────────────────────────────

/// Describes a line that was rejected by an [`EncodingValidator`].
///
/// Carries the offending line content and a human-readable explanation of why
/// it was rejected.
#[derive(Debug, Clone)]
pub struct EncodingError {
    /// The exact line content that was rejected.
    pub line: Box<str>,
    /// A human-readable explanation of the rejection.
    pub description: Box<str>,
}

impl EncodingError {
    pub(crate) fn new(line: &str, description: impl Into<String>) -> EncodingError {
        EncodingError {
            line: line.into(),
            description: description.into().into_boxed_str(),
        }
    }
}

impl fmt::Display for EncodingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "encoding error: {} — line: {:?}",
            self.description, self.line
        )
    }
}

impl std::error::Error for EncodingError {}

// ── EncodingValidator trait ───────────────────────────────────────────────────

/// Policy object that decides whether a line of text may be stored in a
/// [`crate::LineBuffer`].
///
/// # Contract
///
/// Implementations **must** be pure: `validate(line)` must return the same
/// result for the same `line` regardless of call order or external state.
/// Implementations **must not** retain a reference to `line` after `validate`
/// returns.
///
/// The trait requires `Send + Sync` so that a shared `Arc<dyn EncodingValidator>`
/// can be used across threads.
pub trait EncodingValidator: Send + Sync {
    /// Returns `Ok(())` if `line` is acceptable, or an [`EncodingError`]
    /// describing the rejection.
    fn validate(&self, line: &str) -> Result<(), EncodingError>;
}

// ── Utf8Validator ─────────────────────────────────────────────────────────────

/// An [`EncodingValidator`] that accepts every valid UTF-8 string unconditionally.
///
/// Because mallard stores all content as UTF-8, this validator imposes no
/// additional restrictions and is suitable for files intended to be saved as
/// UTF-8.
pub struct Utf8Validator;

impl EncodingValidator for Utf8Validator {
    fn validate(&self, _line: &str) -> Result<(), EncodingError> {
        Ok(())
    }
}

// ── AsciiValidator ────────────────────────────────────────────────────────────

/// An [`EncodingValidator`] that rejects any line containing a code point above
/// U+007F.
///
/// Suitable for files that must be representable as 7-bit ASCII.  The error
/// message identifies the first offending character and its Unicode scalar value.
pub struct AsciiValidator;

impl EncodingValidator for AsciiValidator {
    fn validate(&self, line: &str) -> Result<(), EncodingError> {
        if let Some(ch) = line.chars().find(|c| *c > '\u{007F}') {
            return Err(EncodingError::new(
                line,
                format!(
                    "character {:?} (U+{:04X}) is not ASCII (above U+007F)",
                    ch, ch as u32
                ),
            ));
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    // ── ML-10: Milestone 2 unit tests ────────────────────────────────────────

    // — Utf8Validator —

    #[test]
    fn utf8_accepts_pure_ascii() {
        let v = Utf8Validator;
        assert!(v.validate("Hello, world!").is_ok());
        assert!(v.validate("").is_ok());
        assert!(v.validate("!@#$%^&*()_+-=[]{}|;':\",./<>?").is_ok());
    }

    #[test]
    fn utf8_accepts_cjk() {
        let v = Utf8Validator;
        assert!(v.validate("日本語テスト").is_ok());
        assert!(v.validate("中文测试").is_ok());
        assert!(v.validate("한국어 테스트").is_ok());
    }

    #[test]
    fn utf8_accepts_emoji() {
        let v = Utf8Validator;
        assert!(v.validate("Hello 🦆").is_ok());
        assert!(v.validate("🌍🌎🌏").is_ok());
    }

    #[test]
    fn utf8_accepts_rtl() {
        let v = Utf8Validator;
        assert!(v.validate("مرحبا بالعالم").is_ok());
        assert!(v.validate("שלום עולם").is_ok());
    }

    #[test]
    fn utf8_accepts_combining_chars() {
        // e + combining acute (U+0301) = é as two code points
        let v = Utf8Validator;
        assert!(v.validate("e\u{0301}").is_ok());
        assert!(v.validate("a\u{0300}\u{0301}").is_ok());
    }

    // — AsciiValidator —

    #[test]
    fn ascii_accepts_diverse_ascii_strings() {
        let v = AsciiValidator;
        let cases = [
            "",
            "hello",
            "HELLO WORLD",
            "1234567890",
            "!@#$%^&*()",
            "\t\r\n",
            "line with spaces   ",
            "mix OF CaSe 123",
            "symbols: ~`|\\",
            "all printable: abcdefghijklmnopqrstuvwxyz",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ];
        for s in cases {
            assert!(v.validate(s).is_ok(), "expected Ok for {:?}", s);
        }
    }

    #[test]
    fn ascii_rejects_single_non_ascii_char() {
        let v = AsciiValidator;
        let result = v.validate("hello\u{00e9}world"); // é
        assert!(result.is_err());
    }

    #[test]
    fn ascii_rejects_entirely_non_ascii() {
        let v = AsciiValidator;
        assert!(v.validate("日本語").is_err());
        assert!(v.validate("🦆🦆🦆").is_err());
    }

    #[test]
    fn ascii_error_is_human_readable_and_identifies_char() {
        let v = AsciiValidator;
        let err = v.validate("caf\u{00e9}").unwrap_err();
        let msg = err.to_string();
        // Must mention the character in a readable way.
        assert!(
            msg.contains("U+00E9") || msg.contains("U+00e9"),
            "got: {}",
            msg
        );
        // The line itself must be carried in the error.
        assert_eq!(&*err.line, "caf\u{00e9}");
    }

    #[test]
    fn ascii_error_description_field() {
        let v = AsciiValidator;
        let err = v.validate("x\u{1F986}").unwrap_err(); // 🦆
        assert!(!err.description.is_empty());
    }

    // — Trait object usage —

    #[test]
    fn trait_object_box() {
        let v: Box<dyn EncodingValidator> = Box::new(AsciiValidator);
        assert!(v.validate("ok").is_ok());
        assert!(v.validate("日").is_err());
    }

    #[test]
    fn trait_object_arc() {
        let v: Arc<dyn EncodingValidator> = Arc::new(Utf8Validator);
        assert!(v.validate("anything goes 🦆").is_ok());
    }

    // — Send + Sync —

    #[test]
    fn utf8_validator_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Utf8Validator>();
    }

    #[test]
    fn ascii_validator_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AsciiValidator>();
    }

    // — ShiftJisValidator (test-only, uses encoding_rs) —

    /// A test-only `EncodingValidator` that rejects any string that cannot be
    /// losslessly encoded to Shift-JIS and decoded back to the original UTF-8.
    ///
    /// This demonstrates that a caller-supplied, third-party validator integrates
    /// correctly with the `EncodingValidator` trait.
    struct ShiftJisValidator;

    impl EncodingValidator for ShiftJisValidator {
        fn validate(&self, line: &str) -> Result<(), EncodingError> {
            let encoding = encoding_rs::SHIFT_JIS;
            let (encoded, _, had_unmappable) = encoding.encode(line);
            if had_unmappable {
                return Err(EncodingError::new(
                    line,
                    "line contains characters that cannot be represented in Shift-JIS",
                ));
            }
            // Round-trip: decode back and verify identity.
            let (decoded, _, had_error) = encoding.decode(&encoded);
            if had_error || decoded.as_ref() != line {
                return Err(EncodingError::new(
                    line,
                    "Shift-JIS round-trip failed: decoded output differs from input",
                ));
            }
            Ok(())
        }
    }

    #[test]
    fn shift_jis_accepts_pure_ascii() {
        let v = ShiftJisValidator;
        assert!(v.validate("Hello, world!").is_ok());
        assert!(v.validate("").is_ok());
        assert!(v.validate("1234567890 !@#$%^&*()").is_ok());
    }

    #[test]
    fn shift_jis_accepts_representable_kanji() {
        let v = ShiftJisValidator;
        // 日 (U+65E5), 本 (U+672C), 語 (U+8A9E) — all in the JIS X 0208 set.
        assert!(
            v.validate("日本語").is_ok(),
            "kanji in JIS X 0208 must be accepted"
        );
        // Further kanji also in Shift-JIS.
        assert!(v.validate("東京大阪").is_ok());
        assert!(v.validate("漢字テスト").is_ok());
    }

    #[test]
    fn shift_jis_rejects_emoji() {
        let v = ShiftJisValidator;
        // 🦆 (U+1F986) has no Shift-JIS representation.
        let result = v.validate("Hello 🦆");
        assert!(
            result.is_err(),
            "emoji must be rejected by ShiftJisValidator"
        );
    }

    #[test]
    fn shift_jis_rejects_characters_outside_jis_set() {
        let v = ShiftJisValidator;
        // Arabic is not in Shift-JIS.
        assert!(v.validate("مرحبا").is_err());
        // Emoji (duck) rejected.
        assert!(v.validate("🦆").is_err());
        // Combining diacritics outside Latin-1 block that Shift-JIS cannot encode.
        assert!(v.validate("café ☕").is_err()); // ☕ U+2615 not in JIS X 0208
    }

    #[test]
    fn shift_jis_error_message_is_descriptive() {
        let v = ShiftJisValidator;
        let err = v.validate("🦆").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.is_empty());
        assert!(
            msg.contains("Shift-JIS"),
            "error message should mention Shift-JIS: {}",
            msg
        );
    }

    #[test]
    fn shift_jis_validator_integrates_as_trait_object() {
        let v: Box<dyn EncodingValidator> = Box::new(ShiftJisValidator);
        assert!(v.validate("日本語").is_ok());
        assert!(v.validate("🦆").is_err());
    }

    #[test]
    fn shift_jis_validator_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ShiftJisValidator>();
    }
}
