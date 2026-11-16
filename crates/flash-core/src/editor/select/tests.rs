//! Multi-selection commands: hand cases and a property.

use proptest::prelude::*;

use super::*;
use crate::strategies::text_strategy;
use crate::{Context, Event, KeyCode, KeyEvent, Text};

fn editor(s: &str, ranges: &[(usize, usize)], primary: usize) -> (Editor, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    let sel = Selection::new(ranges.iter().map(|&(a, h)| Range::new(a, h)), primary).unwrap();
    ed.set_selection(view, sel).unwrap();
    ed.flush_events();
    (ed, view)
}

fn ranges(ed: &Editor, v: ViewId) -> (Vec<(usize, usize)>, usize) {
    let sel = ed.selection(v).unwrap();
    (
        sel.ranges()
            .iter()
            .map(|r| (r.anchor(), r.head()))
            .collect(),
        sel.primary_index(),
    )
}

fn run(ed: &mut Editor, name: &str, arg: &str) -> Effects {
    ed.run_named(
        name,
        &Context {
            arg: Some(arg),
            ..Context::default()
        },
    )
}

#[test]
fn select_matches_inside_the_selections() {
    let (mut ed, v) = editor("foo bar foo baz foo", &[(0, 11), (16, 19)], 1);
    run(&mut ed, "select-regex", "foo|ba.");
    assert_eq!(ranges(&ed, v), (vec![(0, 3), (4, 7), (8, 11), (16, 19)], 3));
    let fx = run(&mut ed, "select-regex", "zzz");
    assert!(fx.message && ed.message().contains("no matches"));
    assert_eq!(ranges(&ed, v).0.len(), 4, "unchanged");
    run(&mut ed, "select-regex", "(");
    assert!(ed.message().contains("bad pattern"), "{}", ed.message());
}

#[test]
fn split_on_matches_and_lines() {
    let (mut ed, v) = editor("a, b,c,, d", &[(0, 10)], 0);
    run(&mut ed, "split-regex", r",\s*");
    assert_eq!(ranges(&ed, v).0, [(0, 1), (3, 4), (5, 6), (9, 10)]);
    let (mut ed, v) = editor("one\r\ntwo\n\nthree", &[(1, 15)], 0);
    ed.run_named("split-lines", &Context::default());
    assert_eq!(ranges(&ed, v).0, [(1, 3), (5, 8), (10, 15)]);
    // Splitting on everything leaves nothing: refused.
    let fx = run(&mut ed, "split-regex", r"[\s\S]+");
    assert!(fx.message);
    assert_eq!(ranges(&ed, v).0.len(), 3);
}

#[test]
fn keep_and_remove_matching() {
    let (mut ed, v) = editor("ab cd ae", &[(0, 2), (3, 5), (6, 8)], 2);
    run(&mut ed, "keep-matching", "^a");
    assert_eq!(ranges(&ed, v), (vec![(0, 2), (6, 8)], 1));
    run(&mut ed, "remove-matching", "e");
    assert_eq!(ranges(&ed, v), (vec![(0, 2)], 0));
    let fx = run(&mut ed, "remove-matching", "a");
    assert!(fx.message && ed.message().contains("no selection"));
}

#[test]
fn rotating_the_primary() {
    let (mut ed, v) = editor("abcdef", &[(0, 1), (2, 3), (4, 5)], 0);
    ed.run_named("rotate-primary-forward", &Context::default());
    assert_eq!(ranges(&ed, v).1, 1);
    ed.run_named("rotate-primary-backward", &Context::default());
    ed.run_named("rotate-primary-backward", &Context::default());
    assert_eq!(ranges(&ed, v).1, 2, "wraps around");
}

#[test]
fn keys_and_the_pattern_prompt() {
    let (mut ed, v) = editor("x1 y22 z333", &[(0, 11)], 0);
    let press = |ed: &mut Editor, c| ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Char(c))));
    press(&mut ed, 's');
    assert_eq!(
        ed.prompt().unwrap().kind(),
        PromptKind::Regex(RegexAction::Select)
    );
    for c in r"\d+".chars() {
        press(&mut ed, c);
    }
    ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Enter)));
    assert_eq!(ranges(&ed, v).0, [(1, 2), (4, 6), (8, 11)]);
    press(&mut ed, ')');
    assert_eq!(ranges(&ed, v).1, 1);
    press(&mut ed, ',');
    assert_eq!(ranges(&ed, v), (vec![(4, 6)], 0));
}

proptest! {
    /// Every result is valid, inside some original selection, and the
    /// selection stays normalised; refusals change nothing.
    #[test]
    fn results_stay_inside_the_selections(
        s in text_strategy(12),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 1..6),
        action in 0..4usize,
        pattern in prop::sample::select(vec!["a", r"\s+", r"\w", ".", "x|é", "^", r"\n"]),
    ) {
        let t = Text::from(s.as_str());
        let mut points: Vec<usize> = cuts
            .iter()
            .map(|i| i.index(s.len() + 1))
            .map(|i| crate::strategies::floor_boundary(&s, i))
            .collect();
        points.sort_unstable();
        points.dedup();
        let ranges: Vec<(usize, usize)> = points.windows(2).map(|w| (w[0], w[1])).collect();
        let ranges = if ranges.is_empty() { vec![(0, s.len())] } else { ranges };
        let (mut ed, v) = editor(&s, &ranges, 0);
        let before = ed.selection(v).unwrap().clone();
        let name = ["select-regex", "split-regex", "keep-matching", "remove-matching"][action];
        run(&mut ed, name, pattern);
        let after = ed.selection(v).unwrap().clone();
        prop_assert_eq!(after.validate(&t), Ok(()));
        prop_assert_eq!(
            &after,
            &Selection::new(after.ranges().iter().copied(), after.primary_index()).unwrap()
        );
        for r in after.ranges() {
            prop_assert!(
                before.ranges().iter().any(|b| b.start() <= r.start() && r.end() <= b.end()),
                "{:?} outside {:?}", r, before
            );
        }
        if ed.message().contains("no ") {
            prop_assert_eq!(&after, &before);
        }
    }
}
