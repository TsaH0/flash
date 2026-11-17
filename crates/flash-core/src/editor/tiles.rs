//! Which views are on screen, and where (PLAN Stage 7c: the grid
//! composites views; splits).
//!
//! A binary tree of tiles over the text area (the screen minus its last
//! row, which holds the focused view's status line or the prompt). A
//! *stacked* split (`:split`) puts one view above the other with a
//! separator row between them naming the upper one; a *side-by-side*
//! split (`:vsplit`) puts a separator column between them. Each split
//! halves its area. Views not in the tree exist but are not shown; the
//! focused view is always shown. With one view the layout is the Stage 6
//! one: the whole text area.

use super::ViewId;

/// A rectangle of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// A line between split views: a row naming the view above (`view`), or
/// a column (`vertical`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separator {
    pub rect: Rect,
    pub vertical: bool,
    /// The view above (a row) or to the left (a column).
    pub view: ViewId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tile {
    View(ViewId),
    Split {
        /// Side by side (a separator column) rather than stacked.
        vertical: bool,
        first: Box<Tile>,
        second: Box<Tile>,
    },
}

/// The tree of shown views.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tiling {
    root: Option<Tile>,
}

impl Tiling {
    pub(crate) fn contains(&self, view: ViewId) -> bool {
        self.leaves().contains(&view)
    }

    /// The shown views, left to right and top to bottom.
    pub(crate) fn leaves(&self) -> Vec<ViewId> {
        fn walk(t: &Tile, out: &mut Vec<ViewId>) {
            match t {
                Tile::View(v) => out.push(*v),
                Tile::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }

    /// Shows `view` alone.
    pub(crate) fn set_only(&mut self, view: ViewId) {
        self.root = Some(Tile::View(view));
    }

    /// Shows `new` where `old` is. `false` if `old` is not shown.
    pub(crate) fn replace(&mut self, old: ViewId, new: ViewId) -> bool {
        fn walk(t: &mut Tile, old: ViewId, new: ViewId) -> bool {
            match t {
                Tile::View(v) if *v == old => {
                    *v = new;
                    true
                }
                Tile::View(_) => false,
                Tile::Split { first, second, .. } => {
                    walk(first, old, new) || walk(second, old, new)
                }
            }
        }
        self.root.as_mut().is_some_and(|r| walk(r, old, new))
    }

    /// Splits `at`'s tile: `at` first, `new` second. `false` if `at` is
    /// not shown.
    pub(crate) fn split(&mut self, at: ViewId, new: ViewId, vertical: bool) -> bool {
        fn walk(t: &mut Tile, at: ViewId, new: ViewId, vertical: bool) -> bool {
            match t {
                Tile::View(v) if *v == at => {
                    *t = Tile::Split {
                        vertical,
                        first: Box::new(Tile::View(at)),
                        second: Box::new(Tile::View(new)),
                    };
                    true
                }
                Tile::View(_) => false,
                Tile::Split { first, second, .. } => {
                    walk(first, at, new, vertical) || walk(second, at, new, vertical)
                }
            }
        }
        self.root
            .as_mut()
            .is_some_and(|r| walk(r, at, new, vertical))
    }

    /// Stops showing `view`: its sibling takes the split's place. `false`
    /// if it was not shown.
    pub(crate) fn remove(&mut self, view: ViewId) -> bool {
        fn walk(t: &mut Tile, view: ViewId) -> bool {
            let Tile::Split { first, second, .. } = t else {
                return false;
            };
            let keep = if **first == Tile::View(view) {
                Some((**second).clone())
            } else if **second == Tile::View(view) {
                Some((**first).clone())
            } else {
                None
            };
            match keep {
                Some(keep) => {
                    *t = keep;
                    true
                }
                None => walk(first, view) || walk(second, view),
            }
        }
        match &mut self.root {
            Some(Tile::View(v)) if *v == view => {
                self.root = None;
                true
            }
            Some(root) => walk(root, view),
            None => false,
        }
    }

    /// Where each shown view and separator goes in `area`.
    pub(crate) fn layout(
        &self,
        area: Rect,
        views: &mut Vec<(ViewId, Rect)>,
        separators: &mut Vec<Separator>,
    ) {
        views.clear();
        separators.clear();
        if let Some(root) = &self.root {
            place(root, area, views, separators);
        }
    }
}

/// The view of a tile that touches the separator after it: the lowest of
/// a stacked split, the leftmost of a side-by-side one.
fn first_view(t: &Tile) -> ViewId {
    match t {
        Tile::View(v) => *v,
        Tile::Split {
            first,
            second,
            vertical,
        } => {
            // A stacked split's lower part is what touches the separator
            // below it; a side-by-side one's first part is its left.
            if *vertical {
                first_view(first)
            } else {
                first_view(second)
            }
        }
    }
}

fn place(t: &Tile, area: Rect, views: &mut Vec<(ViewId, Rect)>, seps: &mut Vec<Separator>) {
    match t {
        Tile::View(v) => views.push((*v, area)),
        Tile::Split {
            vertical: true,
            first,
            second,
        } => {
            // Side by side: | between, the left part the larger.
            let avail = area.width.saturating_sub(1);
            let left = avail - avail / 2;
            let right = avail / 2;
            place(
                first,
                Rect {
                    width: left,
                    ..area
                },
                views,
                seps,
            );
            if area.width > 0 {
                seps.push(Separator {
                    rect: Rect {
                        x: area.x + left,
                        width: 1,
                        ..area
                    },
                    vertical: true,
                    view: first_view(first),
                });
            }
            place(
                second,
                Rect {
                    x: (area.x + left + 1).min(area.x + area.width),
                    width: right,
                    ..area
                },
                views,
                seps,
            );
        }
        Tile::Split {
            vertical: false,
            first,
            second,
        } => {
            // Stacked: a row naming the upper view between.
            let avail = area.height.saturating_sub(1);
            let top = avail - avail / 2;
            let bottom = avail / 2;
            place(
                first,
                Rect {
                    height: top,
                    ..area
                },
                views,
                seps,
            );
            if area.height > 0 {
                seps.push(Separator {
                    rect: Rect {
                        y: area.y + top,
                        height: 1,
                        ..area
                    },
                    vertical: false,
                    view: first_view(first),
                });
            }
            place(
                second,
                Rect {
                    y: (area.y + top + 1).min(area.y + area.height),
                    height: bottom,
                    ..area
                },
                views,
                seps,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use slotmap::SlotMap;

    use super::*;

    fn ids(n: usize) -> Vec<ViewId> {
        let mut m: SlotMap<ViewId, ()> = SlotMap::with_key();
        (0..n).map(|_| m.insert(())).collect()
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn splits_halve_their_area_with_separators() {
        let v = ids(3);
        let mut t = Tiling::default();
        t.set_only(v[0]);
        let (mut views, mut seps) = (Vec::new(), Vec::new());
        let area = rect(0, 0, 80, 23);
        t.layout(area, &mut views, &mut seps);
        assert_eq!(views, [(v[0], area)]);
        assert!(seps.is_empty());
        assert!(t.split(v[0], v[1], false));
        t.layout(area, &mut views, &mut seps);
        assert_eq!(
            views,
            [(v[0], rect(0, 0, 80, 11)), (v[1], rect(0, 12, 80, 11))]
        );
        assert_eq!(seps[0].rect, rect(0, 11, 80, 1));
        assert_eq!(seps[0].view, v[0]);
        assert!(t.split(v[1], v[2], true));
        t.layout(area, &mut views, &mut seps);
        assert_eq!(
            views,
            [
                (v[0], rect(0, 0, 80, 11)),
                (v[1], rect(0, 12, 40, 11)),
                (v[2], rect(41, 12, 39, 11))
            ]
        );
        assert_eq!(seps[1].rect, rect(40, 12, 1, 11));
        assert!(seps[1].vertical);
        assert_eq!(t.leaves(), [v[0], v[1], v[2]]);
        // Removing collapses the split; the sibling takes its place.
        assert!(t.remove(v[1]));
        t.layout(area, &mut views, &mut seps);
        assert_eq!(
            views,
            [(v[0], rect(0, 0, 80, 11)), (v[2], rect(0, 12, 80, 11))]
        );
        assert!(!t.remove(v[1]));
        assert!(t.replace(v[2], v[1]));
        assert_eq!(t.leaves(), [v[0], v[1]]);
        assert!(t.remove(v[0]) && t.remove(v[1]));
        assert!(t.leaves().is_empty());
    }

    #[test]
    fn tiny_areas_do_not_underflow() {
        let v = ids(2);
        let mut t = Tiling::default();
        t.set_only(v[0]);
        t.split(v[0], v[1], true);
        let (mut views, mut seps) = (Vec::new(), Vec::new());
        for (w, h) in [(0, 0), (1, 1), (2, 0), (0, 3)] {
            t.layout(rect(0, 0, w, h), &mut views, &mut seps);
            for (_, r) in &views {
                assert!(
                    r.x + r.width <= w && r.y + r.height <= h,
                    "{r:?} in {w}x{h}"
                );
            }
        }
    }
}
