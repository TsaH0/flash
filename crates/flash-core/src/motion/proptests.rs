//! Randomised motion properties against the naive model.

use std::num::NonZeroU8;

use proptest::prelude::*;

use super::reference::{vertical_naive, word_naive};
use super::*;
use crate::strategies::text_strategy;
use crate::text::reference::RefText;

const T4: NonZeroU8 = NonZeroU8::new(4).unwrap();

fn every_motion(count: usize) -> Vec<Motion> {
    let mut v = vec![
        Motion::LineStart,
        Motion::LineEnd,
        Motion::DocumentStart,
        Motion::DocumentEnd,
    ];
    for direction in [Direction::Forward, Direction::Backward] {
        v.push(Motion::Grapheme { count, direction });
        v.push(Motion::Word { count, direction });
        v.push(Motion::Vertical {
            count,
            direction,
            tab_width: T4,
        });
    }
    v
}

proptest! {
    #[test]
    fn word_and_vertical_match_reference(s in text_strategy(20), count in 1usize..=3) {
        let t = Text::from(s.as_str());
        for h in RefText::new(&s).grapheme_boundaries() {
            for direction in [Direction::Forward, Direction::Backward] {
                let got = Motion::Word { count, direction }
                    .apply_range(&t, Range::point(h), Movement::Move)
                    .unwrap();
                prop_assert_eq!(got.head(), word_naive(&s, h, count, direction), "word {:?} from {}", direction, h);
                let got = Motion::Vertical { count, direction, tab_width: T4 }
                    .apply_range(&t, Range::point(h), Movement::Move)
                    .unwrap();
                let (head, goal) = vertical_naive(&s, h, None, count, direction, T4);
                prop_assert_eq!((got.head(), got.goal_column()), (head, Some(goal)), "vertical {:?} from {}", direction, h);
            }
        }
    }

    #[test]
    fn results_are_boundaries(s in text_strategy(20), count in 1usize..=3) {
        let t = Text::from(s.as_str());
        let model = RefText::new(&s);
        for h in (0..=s.len()).filter(|&h| s.is_char_boundary(h)) {
            for m in every_motion(count) {
                for movement in [Movement::Move, Movement::Extend] {
                    let got = m.apply_range(&t, Range::new(0, h), movement).unwrap();
                    prop_assert!(got.head() <= s.len() && s.is_char_boundary(got.head()), "{:?} from {} gave {:?}", m, h, got);
                    if model.is_grapheme_boundary(h) {
                        prop_assert!(model.is_grapheme_boundary(got.head()), "{:?} from {} gave {:?}", m, h, got);
                    }
                    match movement {
                        Movement::Extend => prop_assert_eq!(got.anchor(), 0),
                        Movement::Move => prop_assert!(got.is_empty()),
                    }
                }
            }
        }
    }

    #[test]
    fn grapheme_round_trip(s in text_strategy(20)) {
        let t = Text::from(s.as_str());
        let right = Motion::Grapheme { count: 1, direction: Direction::Forward };
        let left = Motion::Grapheme { count: 1, direction: Direction::Backward };
        for h in RefText::new(&s).grapheme_boundaries().into_iter().filter(|&h| h < s.len()) {
            let there = right.apply_range(&t, Range::point(h), Movement::Move).unwrap();
            let back = left.apply_range(&t, there, Movement::Move).unwrap();
            prop_assert_eq!(back.head(), h);
        }
    }
}
