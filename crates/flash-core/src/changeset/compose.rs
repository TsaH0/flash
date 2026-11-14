//! Composition: one change set equivalent to applying two in sequence.

use super::{ChangeSet, ChangeSetBuilder, ChangeSetError, Op};

/// The unconsumed part of the op a [`Cursor`] is on.
#[derive(Clone, Copy)]
enum Head<'a> {
    Retain(usize),
    Delete(usize),
    Insert(&'a str),
}

/// Walks a change set's ops, letting the caller consume part of an op.
struct Cursor<'a> {
    ops: std::slice::Iter<'a, Op>,
    head: Option<Head<'a>>,
}

impl<'a> Cursor<'a> {
    fn new(ops: &'a [Op]) -> Self {
        let mut cursor = Self {
            ops: ops.iter(),
            head: None,
        };
        cursor.advance();
        cursor
    }

    fn advance(&mut self) {
        self.head = self.ops.next().map(|op| match op {
            Op::Retain(n) => Head::Retain(*n),
            Op::Delete(n) => Head::Delete(*n),
            Op::Insert(s) => Head::Insert(s),
        });
    }

    /// Consumes `m` bytes of the head, which must have at least `m` (for an
    /// insert, `m` must be a char boundary of it).
    fn consume(&mut self, m: usize) {
        self.head = match self.head {
            Some(Head::Retain(n)) if n > m => Some(Head::Retain(n - m)),
            Some(Head::Delete(n)) if n > m => Some(Head::Delete(n - m)),
            Some(Head::Insert(s)) if s.len() > m => Some(Head::Insert(&s[m..])),
            _ => {
                self.advance();
                return;
            }
        };
    }
}

impl ChangeSet {
    /// The change set equivalent to applying `self` and then `next`.
    ///
    /// Requires `self.len_after() == next.len_before()`, else
    /// [`ChangeSetError::LengthMismatch`]. Fails with
    /// [`ChangeSetError::NotCharBoundary`] if `next` splits a character that
    /// `self` inserted (then `next` is not valid for `self`'s output).
    ///
    /// Guarantees text equivalence only. Positions must be mapped through
    /// the original sequence, not the composition: a delete followed by a
    /// re-insert composes to a replacement, and positions inside it collapse
    /// (docs/PLAN.md Stage 2, "Known trap").
    ///
    /// O(ops of both), plus the inserted bytes copied.
    pub fn compose(&self, next: &ChangeSet) -> Result<ChangeSet, ChangeSetError> {
        if self.len_after != next.len_before {
            return Err(ChangeSetError::LengthMismatch {
                expected: self.len_after,
                actual: next.len_before,
            });
        }
        let mut out =
            ChangeSetBuilder::with_capacity(self.len_before, self.ops.len() + next.ops.len());
        let mut a = Cursor::new(&self.ops);
        let mut b = Cursor::new(&next.ops);
        // Offset in the middle text (self's output, next's input).
        let mut mid = 0;

        loop {
            match (a.head, b.head) {
                // Text deleted by `self` never reaches `next`.
                (Some(Head::Delete(d)), _) => {
                    out.delete(d);
                    a.advance();
                }
                // Text inserted by `next` does not come from `self`.
                (_, Some(Head::Insert(s))) => {
                    out.insert(s);
                    b.advance();
                }
                (None, None) => break,
                (Some(Head::Retain(i)), Some(Head::Retain(j))) => {
                    let m = i.min(j);
                    out.retain(m);
                    a.consume(m);
                    b.consume(m);
                    mid += m;
                }
                (Some(Head::Retain(i)), Some(Head::Delete(j))) => {
                    let m = i.min(j);
                    out.delete(m);
                    a.consume(m);
                    b.consume(m);
                    mid += m;
                }
                (Some(Head::Insert(s)), Some(Head::Retain(j) | Head::Delete(j))) => {
                    let m = s.len().min(j);
                    if !s.is_char_boundary(m) {
                        return Err(ChangeSetError::NotCharBoundary { offset: mid + m });
                    }
                    // Retained inserts survive; deleted ones cancel.
                    if matches!(b.head, Some(Head::Retain(_))) {
                        out.insert(&s[..m]);
                    }
                    a.consume(m);
                    b.consume(m);
                    mid += m;
                }
                // One side ran out early. The length check above rules this
                // out; report it rather than panic.
                _ => {
                    return Err(ChangeSetError::LengthMismatch {
                        expected: self.len_after,
                        actual: next.len_before,
                    });
                }
            }
        }
        out.finish()
    }
}
