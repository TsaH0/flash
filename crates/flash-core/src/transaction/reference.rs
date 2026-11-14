//! Naive transaction model: computes each range's edit, merges overlapping
//! deletions pairwise until nothing overlaps, then applies the edits to a
//! `String` one at a time from last to first, shifting the results already
//! computed for later edits by each edit's length change.

use crate::Selection;
use crate::selection::Range;
use crate::text::Direction;
use crate::text::reference::RefText;

#[derive(Debug, Clone)]
pub(crate) enum Intent {
    Insert(String),
    Delete,
    Replace(String),
    Backspace,
    DeleteForward,
}

/// An edit and the indices of the ranges it belongs to.
struct Edit {
    start: usize,
    end: usize,
    insert: String,
    members: Vec<usize>,
}

pub(crate) fn naive(s: &str, selection: &Selection, intent: &Intent) -> (String, Selection) {
    let ranges = selection.ranges();
    let model = RefText::new(s);
    let mut edits: Vec<Edit> = ranges
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (start, end, insert) = match intent {
                Intent::Insert(x) => (r.head(), r.head(), x.clone()),
                Intent::Delete => (r.start(), r.end(), String::new()),
                Intent::Replace(x) => (r.start(), r.end(), x.clone()),
                Intent::Backspace | Intent::DeleteForward if !r.is_empty() => {
                    (r.start(), r.end(), String::new())
                }
                Intent::Backspace => {
                    let p = model
                        .move_graphemes(r.head(), 1, Direction::Backward)
                        .unwrap();
                    (p, r.head(), String::new())
                }
                Intent::DeleteForward => {
                    let p = model
                        .move_graphemes(r.head(), 1, Direction::Forward)
                        .unwrap();
                    (r.head(), p, String::new())
                }
            };
            Edit {
                start,
                end,
                insert,
                members: vec![i],
            }
        })
        .collect();

    // Merge overlapping deletions until a fixpoint (deletions only).
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..edits.len() {
            for j in i + 1..edits.len() {
                let (a, b) = (&edits[i], &edits[j]);
                if a.start.max(b.start) < a.end.min(b.end) {
                    let b = edits.remove(j);
                    let a = &mut edits[i];
                    a.start = a.start.min(b.start);
                    a.end = a.end.max(b.end);
                    a.members.extend(b.members);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    edits.sort_by_key(|e| (e.start, e.members.iter().min().copied()));

    let mut text = s.to_owned();
    let mut results: Vec<Option<Range>> = vec![None; ranges.len()];
    for e in edits.iter().rev() {
        text.replace_range(e.start..e.end, &e.insert);
        let delta = e.insert.len() as isize - (e.end - e.start) as isize;
        let shift = |p: usize| (p as isize + delta) as usize;
        for later in results.iter_mut().flatten() {
            *later = shift_range(*later, &shift);
        }
        let (n, len) = (e.start, e.insert.len());
        for &i in &e.members {
            let r = ranges[i];
            results[i] = Some(match intent {
                Intent::Insert(_) if r.is_empty() => Range::point(n + len),
                Intent::Insert(_) if r.anchor() < r.head() => Range::new(r.anchor(), n + len),
                Intent::Insert(_) => Range::new(r.anchor() + len, n + len),
                Intent::Replace(_) if r.direction() == Direction::Backward => {
                    Range::new(n + len, n)
                }
                Intent::Replace(_) => Range::new(n, n + len),
                _ => Range::point(n),
            });
        }
    }
    let results: Vec<Range> = results
        .into_iter()
        .map(|r| r.expect("every range has an edit"))
        .collect();
    (
        text,
        Selection::new(results, selection.primary_index()).unwrap(),
    )
}

fn shift_range(r: Range, shift: &impl Fn(usize) -> usize) -> Range {
    Range::new(shift(r.anchor()), shift(r.head()))
}
