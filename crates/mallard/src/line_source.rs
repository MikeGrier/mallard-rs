// Copyright (c) 2026, Michael Grier.

//! The [`LineSource`] trait for supplying lines to a [`crate::LineBuffer`].
//!
//! Implementors are responsible for stripping any line-ending characters
//! (`\n`, `\r\n`, `\r`) before passing strings to the callback.  Lines
//! supplied to `for_each_line` must not include any terminator bytes.

/// An object-safe source of text lines.
///
/// Implementors supply lines stripped of their terminators.  Lines must not
/// contain any line-ending characters (`\n`, `\r`, or `\r\n`).  Implementors
/// are responsible for stripping the terminators before invoking the callback.
///
/// # Object safety
///
/// The trait is deliberately object-safe so that heterogeneous sources can be
/// used at runtime via `&dyn LineSource`.
pub trait LineSource {
    /// A hint about the total number of lines, used for pre-allocation.
    ///
    /// Returns `None` if the count is unknown.  Implementations may
    /// over- or under-estimate without affecting correctness.
    fn line_count_hint(&self) -> Option<usize>;

    /// Calls `f` once for each line, in order.
    ///
    /// Lines must not include any line-ending characters.
    fn for_each_line(&self, f: &mut dyn FnMut(&str));
}
