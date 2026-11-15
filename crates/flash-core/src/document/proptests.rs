//! Randomised commits from several views of one document.

use proptest::prelude::*;
use proptest::sample::Index;
use slotmap::SlotMap;

use super::*;
use crate::ChangeSet;
use crate::changeset::reference::apply_ops;
use crate::selection::proptests::assert_normalised;
use crate::strategies::{floor_boundary, selection_for, text_strategy};
use crate::transaction::proptests::{build, intent};
use crate::transaction::reference::Intent;

type Case = (String, Vec<Selection>, Vec<(Index, Intent)>, Vec<Index>);

fn case() -> impl Strategy<Value = Case> {
    text_strategy(8).prop_flat_map(|s| {
        (
            prop::collection::vec(selection_for(&s), 1..=3),
            prop::collection::vec((any::<Index>(), intent()), 1..12),
            prop::collection::vec(any::<Index>(), 3),
            Just(s),
        )
            .prop_map(|(sels, steps, probes, s)| (s, sels, steps, probes))
    })
}

proptest! {
    #[test]
    fn views_stay_valid_under_commits((s, sels, steps, probes) in case()) {
        let mut doc = Document::new(Text::from(s.as_str()), None);
        let mut ids: SlotMap<ViewId, ()> = SlotMap::with_key();
        let views: Vec<ViewId> = sels.iter().map(|_| ids.insert(())).collect();
        for (&v, sel) in views.iter().zip(&sels) {
            doc.add_view(v);
            doc.set_selection(v, sel.clone()).unwrap();
        }
        let mut model = s.clone();
        let mut history: Vec<ChangeSet> = Vec::new();

        for (pick, intent) in &steps {
            let origin = views[pick.index(views.len())];
            let before: Vec<Selection> =
                views.iter().map(|&v| doc.selection(v).unwrap().clone()).collect();
            let revision = doc.revision();
            let txn = build(doc.text(), &before[pick.index(views.len())], intent).unwrap();
            let want_origin = txn.prepare(doc.text(), &before[pick.index(views.len())]).unwrap();
            let changes = txn.changes().clone();
            model = apply_ops(changes.ops(), &model);

            prop_assert_eq!(doc.commit(origin, txn), Ok(doc.revision()));
            prop_assert_eq!(doc.text().to_string(), model.as_str());
            for (i, &v) in views.iter().enumerate() {
                let got = doc.selection(v).unwrap();
                if v == origin {
                    prop_assert_eq!(got, &want_origin);
                } else if !changes.is_identity() {
                    prop_assert_eq!(got, &before[i].map(&changes).unwrap());
                } else {
                    prop_assert_eq!(got, &before[i]);
                }
                prop_assert_eq!(got.validate(doc.text()), Ok(()));
                assert_normalised(got)?;
            }
            let advanced = u64::from(!changes.is_identity());
            prop_assert_eq!(doc.revision().get(), revision.get() + advanced);
            history.push(changes);
        }

        let mut positions: Vec<usize> = probes
            .iter()
            .map(|i| floor_boundary(&s, i.index(s.len() + 1)))
            .collect();
        positions.sort_unstable();
        let expected: Vec<usize> = positions
            .iter()
            .map(|&p| history.iter().fold(p, |p, c| c.map_pos(p, Assoc::After).unwrap()))
            .collect();
        doc.map_from(Revision::ZERO, &mut positions, Assoc::After).unwrap();
        prop_assert_eq!(positions, expected);
    }
}
