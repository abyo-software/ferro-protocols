// SPDX-License-Identifier: Apache-2.0
//! Byte-offset → 1-indexed line/column resolver shared by both backends.
//!
//! ruff hands back [`ruff_text_size::TextRange`] (byte offsets);
//! rustpython hands back the same in 0.4. We pre-compute the `\n`
//! offsets once per file so the per-marker conversion is `O(log N)`
//! rather than `O(N)` over the source.

#[cfg(feature = "parser-ruff")]
use ruff_text_size::{Ranged, TextRange};

use crate::common::SourceSpan;

/// Pre-computed `\n` offsets for a single source string.
pub(crate) struct LineIndex {
    /// Sorted list of byte offsets, one entry per line: `line_starts[i]`
    /// is the byte index of the first character of line `i + 1`
    /// (1-indexed). `line_starts[0]` is always `0`.
    line_starts: Vec<u32>,
    /// Total source byte length, kept so we can clamp `end` offsets that
    /// run past EOF (some parsers emit one-past-end ranges).
    total_len: u32,
}

impl LineIndex {
    /// Build a [`LineIndex`] from a source string.
    pub(crate) fn new(src: &str) -> Self {
        let mut line_starts = Vec::with_capacity(src.len() / 40);
        line_starts.push(0u32);
        for (idx, b) in src.as_bytes().iter().enumerate() {
            if *b == b'\n' {
                // Next line starts at idx + 1.
                let next = u32::try_from(idx + 1).unwrap_or(u32::MAX);
                line_starts.push(next);
            }
        }
        Self {
            line_starts,
            total_len: u32::try_from(src.len()).unwrap_or(u32::MAX),
        }
    }

    /// Translate a byte offset into a 1-indexed `(line, col)` pair.
    /// Lines and columns are 1-indexed (matches Python tracebacks).
    pub(crate) fn line_col(&self, offset: u32) -> (u32, u32) {
        let clamped = offset.min(self.total_len);
        // Binary search for the largest line_start ≤ clamped.
        let line_idx = match self.line_starts.binary_search(&clamped) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts[line_idx];
        let col = clamped.saturating_sub(line_start) + 1;
        (u32::try_from(line_idx).unwrap_or(u32::MAX) + 1, col)
    }

    /// Convert a ruff [`TextRange`] into an inclusive 1-indexed line span.
    #[cfg(feature = "parser-ruff")]
    pub(crate) fn span_of(&self, range: TextRange) -> SourceSpan {
        let (start_line, _) = self.line_col(range.start().to_u32());
        // ruff `end()` is exclusive; if it lands on a newline character
        // the construct ends on the previous line.
        let end_offset = range.end().to_u32().saturating_sub(1);
        let (end_line, _) = self.line_col(end_offset);
        SourceSpan {
            start_line,
            end_line: end_line.max(start_line),
        }
    }
}

#[allow(dead_code)]
#[cfg(feature = "parser-ruff")]
pub(crate) fn ruff_line_col(idx: &LineIndex, node: &impl Ranged) -> (u32, u32) {
    idx.line_col(node.range().start().to_u32())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_byte_is_line_one_col_one() {
        let idx = LineIndex::new("abc\ndef\n");
        assert_eq!(idx.line_col(0), (1, 1));
    }

    #[test]
    fn newline_advances_line() {
        let idx = LineIndex::new("abc\ndef\n");
        // 'd' is at offset 4 — line 2 col 1.
        assert_eq!(idx.line_col(4), (2, 1));
        assert_eq!(idx.line_col(5), (2, 2));
    }

    #[test]
    fn offset_past_eof_clamps_to_last_line() {
        let idx = LineIndex::new("abc\ndef");
        // total_len is 7; binary search for past-EOF should still resolve.
        assert_eq!(idx.line_col(99).0, 2);
    }

    #[test]
    fn empty_source_collapses_to_one_one() {
        let idx = LineIndex::new("");
        assert_eq!(idx.line_col(0), (1, 1));
        assert_eq!(idx.line_col(7), (1, 1));
    }
}
