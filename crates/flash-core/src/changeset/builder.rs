//! [`ChangeSetBuilder`]: the only constructor of [`ChangeSet`], which keeps
//! every change set in canonical form (see the module docs).

use super::{ChangeSet, ChangeSetError, Op};

/// Largest op capacity [`ChangeSetBuilder::with_capacity`] reserves up front.
const MAX_CAPACITY_HINT: usize = 1 << 16;

/// Builds a [`ChangeSet`] op by op, canonicalising as it goes.
///
/// Ops describe a walk over the old text from start to end. Lengths that
/// overflow `usize` and ops that do not cover exactly `len_before` bytes are
/// reported by [`finish`](ChangeSetBuilder::finish); the builder itself never
/// panics.
#[derive(Debug, Clone)]
pub struct ChangeSetBuilder {
    ops: Vec<Op>,
    len_before: usize,
    /// Old-text bytes covered so far (retained + deleted).
    consumed: usize,
    /// New-text bytes produced so far (retained + inserted).
    produced: usize,
    /// Set once any running sum overflowed; sticky.
    overflowed: bool,
}

impl ChangeSetBuilder {
    /// A builder for a change to a text of `len_before` bytes.
    pub fn new(len_before: usize) -> Self {
        Self::with_capacity(len_before, 0)
    }

    /// Like [`new`](Self::new), with room for `ops` ops. `ops` is a hint:
    /// at most 65,536 ops are reserved up front, so a huge value cannot panic
    /// or abort on allocation; the builder grows as needed.
    pub fn with_capacity(len_before: usize, ops: usize) -> Self {
        Self {
            ops: Vec::with_capacity(ops.min(MAX_CAPACITY_HINT)),
            len_before,
            consumed: 0,
            produced: 0,
            overflowed: false,
        }
    }

    /// Keeps the next `n` bytes.
    pub fn retain(&mut self, n: usize) -> &mut Self {
        if n == 0 {
            return self;
        }
        self.consumed = self.add(self.consumed, n);
        self.produced = self.add(self.produced, n);
        match self.ops.last_mut() {
            Some(Op::Retain(r)) => *r = r.saturating_add(n),
            _ => self.ops.push(Op::Retain(n)),
        }
        self
    }

    /// Removes the next `n` bytes.
    pub fn delete(&mut self, n: usize) -> &mut Self {
        if n == 0 {
            return self;
        }
        self.consumed = self.add(self.consumed, n);
        let len = self.ops.len();
        match self.ops.as_mut_slice() {
            [.., Op::Delete(d)] | [.., Op::Delete(d), Op::Insert(_)] => {
                *d = d.saturating_add(n);
            }
            // Keep deletes before inserts: `Insert, Delete` becomes
            // `Delete, Insert`, which has the same effect.
            [.., Op::Insert(_)] => self.ops.insert(len - 1, Op::Delete(n)),
            _ => self.ops.push(Op::Delete(n)),
        }
        self
    }

    /// Inserts `s` at the current position.
    pub fn insert(&mut self, s: &str) -> &mut Self {
        if s.is_empty() {
            return self;
        }
        self.produced = self.add(self.produced, s.len());
        match self.ops.last_mut() {
            Some(Op::Insert(existing)) => existing.push_str(s),
            _ => self.ops.push(Op::Insert(s.into())),
        }
        self
    }

    /// Keeps every byte not yet covered. Does nothing if the ops already
    /// cover `len_before` bytes or more.
    pub fn retain_rest(&mut self) -> &mut Self {
        let rest = self.len_before.saturating_sub(self.consumed);
        self.retain(rest)
    }

    /// The finished change set.
    ///
    /// Fails with [`ChangeSetError::Overflow`] if a length sum overflowed,
    /// and with [`ChangeSetError::LengthMismatch`] if the ops do not cover
    /// exactly `len_before` bytes.
    pub fn finish(self) -> Result<ChangeSet, ChangeSetError> {
        if self.overflowed {
            return Err(ChangeSetError::Overflow);
        }
        if self.consumed != self.len_before {
            return Err(ChangeSetError::LengthMismatch {
                expected: self.len_before,
                actual: self.consumed,
            });
        }
        Ok(ChangeSet {
            ops: self.ops,
            len_before: self.len_before,
            len_after: self.produced,
        })
    }

    /// `a + b`, recording overflow instead of panicking.
    fn add(&mut self, a: usize, b: usize) -> usize {
        a.checked_add(b).unwrap_or_else(|| {
            self.overflowed = true;
            usize::MAX
        })
    }
}
