//! Randomised transaction properties against the naive model.

use std::num::NonZeroU8;

use proptest::prelude::*;

use super::reference::{Intent, naive};
use super::*;
use crate::changeset::reference::apply_ops;
use crate::selection::proptests::assert_normalised;
use crate::strategies::{selection_for, text_strategy};
use crate::{Direction, Motion, Movement};

fn intent() -> impl Strategy<Value = Intent> {
    prop_oneof![
        text_strategy(2).prop_map(Intent::Insert),
        Just(Intent::Delete),
        text_strategy(2).prop_map(Intent::Replace),
        Just(Intent::Backspace),
        Just(Intent::DeleteForward),
    ]
}

fn build(text: &Text, sel: &Selection, intent: &Intent) -> Result<Transaction, TransactionError> {
    match intent {
        Intent::Insert(s) => Transaction::insert(text, sel, s),
        Intent::Delete => Transaction::delete(text, sel),
        Intent::Replace(s) => Transaction::replace(text, sel, s),
        Intent::Backspace => Transaction::backspace(text, sel),
        Intent::DeleteForward => Transaction::delete_forward(text, sel),
    }
}

#[derive(Debug, Clone)]
enum Step {
    Move(Motion, Movement),
    Edit(Intent),
}

fn motion() -> impl Strategy<Value = Motion> {
    let dir = prop_oneof![Just(Direction::Forward), Just(Direction::Backward)];
    (0..7u8, 1usize..=3, dir).prop_map(|(kind, count, direction)| match kind {
        0 => Motion::Grapheme { count, direction },
        1 => Motion::Word { count, direction },
        2 => Motion::LineStart,
        3 => Motion::LineEnd,
        4 => Motion::Vertical {
            count,
            direction,
            tab_width: NonZeroU8::new(4).unwrap(),
        },
        5 => Motion::DocumentStart,
        _ => Motion::DocumentEnd,
    })
}

fn step() -> impl Strategy<Value = Step> {
    let movement = prop_oneof![Just(Movement::Move), Just(Movement::Extend)];
    prop_oneof![
        (motion(), movement).prop_map(|(m, mv)| Step::Move(m, mv)),
        intent().prop_map(Step::Edit),
    ]
}

fn text_and_selection() -> impl Strategy<Value = (String, Selection)> {
    text_strategy(20).prop_flat_map(|s| {
        let sel = selection_for(&s);
        (Just(s), sel)
    })
}

proptest! {
    #[test]
    fn transaction_matches_naive_model((s, sel) in text_and_selection(), intent in intent()) {
        let mut text = Text::from(s.as_str());
        let mut selection = sel.clone();
        let txn = build(&text, &sel, &intent).unwrap();
        txn.apply(&mut text, &mut selection).unwrap();
        let (want_text, want_sel) = naive(&s, &sel, &intent);
        prop_assert_eq!(text.to_string(), want_text);
        prop_assert_eq!(selection, want_sel);
    }

    #[test]
    fn selection_valid_after_every_step(
        (s, sel) in text_and_selection(),
        steps in prop::collection::vec(step(), 0..20),
    ) {
        let mut text = Text::from(s.as_str());
        let mut shadow = s.clone();
        let mut selection = sel;
        for step in &steps {
            match step {
                Step::Move(m, mv) => selection = m.apply(&text, &selection, *mv).unwrap(),
                Step::Edit(intent) => {
                    let txn = build(&text, &selection, intent).unwrap();
                    shadow = apply_ops(txn.changes().ops(), &shadow);
                    txn.apply(&mut text, &mut selection).unwrap();
                }
            }
            prop_assert_eq!(selection.validate(&text), Ok(()), "after {:?}", step);
            assert_normalised(&selection)?;
            prop_assert_eq!(text.to_string(), shadow.as_str());
        }
    }

    #[test]
    fn failed_apply_leaves_state_unchanged(
        (s, sel) in text_and_selection(),
        intent in intent(),
        u in text_strategy(20),
    ) {
        let txn = build(&Text::from(s.as_str()), &sel, &intent).unwrap();
        let mut text = Text::from(u.as_str());
        let mut selection = Selection::point(0);
        match txn.apply(&mut text, &mut selection) {
            Ok(()) => {
                prop_assert_eq!(text.to_string(), apply_ops(txn.changes().ops(), &u));
                prop_assert_eq!(Some(&selection), txn.selection());
            }
            Err(_) => {
                prop_assert_eq!(text.to_string(), u);
                prop_assert_eq!(selection, Selection::point(0));
            }
        }
    }
}
