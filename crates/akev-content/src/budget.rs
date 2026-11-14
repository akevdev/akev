//! Output budget for the structured decoders.
//!
//! A structured decode turns a few input bytes into a heap-allocated `serde_json::Value` per
//! item (a MessagePack `nil` is one byte, a urlencoded `a&` pair two), so limiting the *input*
//! size alone still lets a small body grow into gigabytes of JSON tree. Every decoder used by
//! [`crate::view_body_with`] therefore charges the values it produces to a [`Budget`] and stops
//! — keeping what it decoded so far — once the budget is used up.

/// Remaining allowance of a structured decode: JSON values (nodes) and bytes of string data.
///
/// Exhaustion is sticky: once a charge doesn't fit, every later charge fails too, so decoders
/// can check [`Budget::exhausted`] at any point to find out whether they must stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    nodes: usize,
    text: usize,
    exhausted: bool,
}

impl Budget {
    /// A budget of `nodes` JSON values holding at most `text` bytes of strings in total.
    pub(crate) fn new(nodes: usize, text: usize) -> Self {
        Budget {
            nodes,
            text,
            exhausted: false,
        }
    }

    /// No limit (the public, non-view decoding functions).
    pub(crate) fn unlimited() -> Self {
        Self::new(usize::MAX, usize::MAX)
    }

    /// Reserve `nodes` values holding `text` bytes of string data. Returns `false` — and marks
    /// the budget exhausted for good — when that doesn't fit.
    #[must_use]
    pub(crate) fn charge(&mut self, nodes: usize, text: usize) -> bool {
        if self.exhausted || nodes > self.nodes || text > self.text {
            self.exhausted = true;
            return false;
        }
        self.nodes -= nodes;
        self.text -= text;
        true
    }

    /// Whether a charge has failed (the decode must stop).
    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// Values that can still be charged (used to cap pre-allocations).
    pub(crate) fn remaining_nodes(&self) -> usize {
        if self.exhausted { 0 } else { self.nodes }
    }
}

/// Size of the standard padded base64 encoding of `n` bytes.
pub(crate) fn base64_len(n: usize) -> usize {
    n.div_ceil(3).saturating_mul(4)
}

/// An upper bound on the number of JSON values in the JSON text `bytes`: every value but the
/// first of a container follows a `,`, the first one follows a `[` or `:` (and object keys
/// follow `{` or `,`), so values ≤ commas + opening brackets + colons + 1. Strings are skipped
/// (with their escapes), so their content never counts. Linear, no allocation.
pub(crate) fn json_value_bound(bytes: &[u8]) -> usize {
    let mut count = 1usize;
    let mut in_string = false;
    let mut escaped = false;
    for &b in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b',' | b'[' | b'{' | b':' => count = count.saturating_add(1),
            _ => {}
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_until_exhausted_and_stays_exhausted() {
        let mut b = Budget::new(3, 10);
        assert!(b.charge(2, 4));
        assert_eq!(b.remaining_nodes(), 1);
        assert!(!b.charge(1, 7)); // text doesn't fit
        assert!(b.exhausted());
        assert!(!b.charge(0, 0)); // sticky
        assert_eq!(b.remaining_nodes(), 0);
        let mut u = Budget::unlimited();
        assert!(u.charge(1 << 40, 1 << 40));
        assert!(!u.exhausted());
    }

    #[test]
    fn json_bounds() {
        assert_eq!(json_value_bound(b"1"), 1);
        assert!(json_value_bound(br#"{"a":1,"b":[2,3]}"#) >= 5);
        // commas inside strings don't count
        assert_eq!(json_value_bound(br#"["a,b,\",c"]"#), 2);
        let many = format!("[{}0]", "0,".repeat(1000));
        assert!(json_value_bound(many.as_bytes()) >= 1001);
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(1), 4);
        assert_eq!(base64_len(3), 4);
        assert_eq!(base64_len(4), 8);
    }
}
