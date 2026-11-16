//! Option registry and scopes, alone and through the editor.

use std::num::NonZeroU8;
use std::time::Duration;

use super::*;
use crate::{Editor, LineEnding, Text};

#[test]
fn builtins_are_at_their_constant_ids() {
    let o = Options::with_builtins();
    let names: Vec<(OptionId, &str)> = o.iter().map(|i| (i.id, i.name)).collect();
    assert_eq!(
        names,
        [
            (OptionId::TAB_WIDTH, "tab-width"),
            (OptionId::UNDO_GROUP_PAUSE_MS, "undo-group-pause-ms"),
            (OptionId::SEQUENCE_TIMEOUT_MS, "sequence-timeout-ms"),
            (OptionId::NORMAL_MIN_GRAPHEME, "normal-min-grapheme"),
            (OptionId::DEFAULT_LINE_ENDING, "default-line-ending"),
        ]
    );
    assert!(
        o.iter()
            .all(|i| i.owner == Owner::BUILTIN && !i.doc.is_empty())
    );
    assert_eq!(o.resolve(None, None), ViewOptions::default());
}

#[test]
fn values_parse_format_and_validate() {
    let o = Options::with_builtins();
    let tab = OptionId::TAB_WIDTH;
    assert_eq!(o.parse(tab, "8"), Ok(OptionValue::Int(8)));
    assert_eq!(o.format(tab, OptionValue::Int(8)), "8");
    for bad in ["0", "256", "-1", "x", "", "4.0"] {
        assert!(
            matches!(o.parse(tab, bad), Err(OptionError::Invalid { .. })),
            "{bad:?}"
        );
    }
    let min = OptionId::NORMAL_MIN_GRAPHEME;
    for (text, value) in [("true", true), ("on", true), ("off", false), ("no", false)] {
        assert_eq!(o.parse(min, text), Ok(OptionValue::Bool(value)));
    }
    assert!(o.parse(min, "1").is_err());
    let le = OptionId::DEFAULT_LINE_ENDING;
    assert_eq!(o.parse(le, "crlf"), Ok(OptionValue::Choice(1)));
    assert_eq!(o.format(le, OptionValue::Choice(1)), "crlf");
    assert!(o.parse(le, "cr").is_err());
    // A value of the wrong kind is refused when set.
    let mut o = o;
    assert!(o.set(Scope::Global, tab, OptionValue::Bool(true)).is_err());
    assert!(o.set(Scope::Global, le, OptionValue::Choice(2)).is_err());
    assert_eq!(o.get(tab, None, None), Some(OptionValue::Int(4)));
}

#[test]
fn registering_removing_and_owners() {
    let mut o = Options::with_builtins();
    let plugin = Owner(7);
    let kind = OptionKind::Int { min: 0, max: 3 };
    assert_eq!(
        o.register("tab-width", plugin, "", kind, OptionValue::Int(0)),
        Err(OptionError::Duplicate("tab-width".into()))
    );
    assert!(
        o.register("x", plugin, "", kind, OptionValue::Int(9))
            .is_err()
    );
    let a = o
        .register("a", plugin, "", kind, OptionValue::Int(1))
        .unwrap();
    let b = o
        .register("b", plugin, "", OptionKind::Bool, OptionValue::Bool(false))
        .unwrap();
    o.set(Scope::Global, a, OptionValue::Int(2)).unwrap();
    assert_eq!(o.get(a, None, None), Some(OptionValue::Int(2)));
    o.remove(a).unwrap();
    assert_eq!(o.id("a"), None);
    assert_eq!(o.get(a, None, None), None);
    assert!(o.set(Scope::Global, a, OptionValue::Int(1)).is_err());
    assert!(o.remove(a).is_err());
    // A new option never reuses a removed ID.
    let c = o
        .register("a", plugin, "", kind, OptionValue::Int(0))
        .unwrap();
    assert_ne!(c, a);
    o.remove_owner(plugin);
    assert_eq!((o.id("a"), o.id("b")), (None, None));
    assert!(o.info(b).is_none());
    assert_eq!(o.iter().count(), 5, "built-ins stay");
}

#[test]
fn scopes_resolve_view_then_document_then_global_then_default() {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from("a\tb"), None);
    let v1 = ed.open_view(doc).unwrap();
    let v2 = ed.open_view(doc).unwrap();
    let other = ed.open(Text::from("x"), None);
    let v3 = ed.open_view(other).unwrap();
    let tab = |ed: &Editor, v| ed.view(v).unwrap().options().tab_width.get();
    assert_eq!((tab(&ed, v1), tab(&ed, v2), tab(&ed, v3)), (4, 4, 4));
    ed.set_option_text(Scope::Global, "tab-width", "8").unwrap();
    assert_eq!((tab(&ed, v1), tab(&ed, v2), tab(&ed, v3)), (8, 8, 8));
    ed.set_option(
        Scope::Document(doc),
        OptionId::TAB_WIDTH,
        OptionValue::Int(2),
    )
    .unwrap();
    assert_eq!((tab(&ed, v1), tab(&ed, v2), tab(&ed, v3)), (2, 2, 8));
    ed.set_option(Scope::View(v2), OptionId::TAB_WIDTH, OptionValue::Int(3))
        .unwrap();
    assert_eq!((tab(&ed, v1), tab(&ed, v2), tab(&ed, v3)), (2, 3, 8));
    assert_eq!(
        ed.option(OptionId::TAB_WIDTH, Some(v2)),
        Some(OptionValue::Int(3))
    );
    assert_eq!(
        ed.option(OptionId::TAB_WIDTH, None),
        Some(OptionValue::Int(8))
    );
    ed.unset_option(Scope::Document(doc), OptionId::TAB_WIDTH)
        .unwrap();
    assert_eq!((tab(&ed, v1), tab(&ed, v2), tab(&ed, v3)), (8, 3, 8));
    ed.unset_option(Scope::Global, OptionId::TAB_WIDTH).unwrap();
    assert_eq!((tab(&ed, v1), tab(&ed, v2), tab(&ed, v3)), (4, 3, 4));
    // The scroll anchor follows the view's tab width.
    ed.set_focus(v2).unwrap();
    let a = *ed.view(v2).unwrap().anchor().unwrap();
    assert_eq!(a.tab_width.get(), 3);
    // Closed scopes are refused; a closed view's values are forgotten.
    ed.close_view(v2).unwrap();
    assert_eq!(
        ed.set_option(Scope::View(v2), OptionId::TAB_WIDTH, OptionValue::Int(5)),
        Err(OptionError::NoSuchScope)
    );
    assert_eq!(
        ed.options().get(OptionId::TAB_WIDTH, Some(v2), None),
        Some(OptionValue::Int(4))
    );
    assert!(ed.set_option_text(Scope::Global, "no-such", "1").is_err());
    assert!(
        ed.set_option_text(Scope::Global, "tab-width", "999")
            .is_err()
    );
    assert_eq!(tab(&ed, v1), 4, "a refused value changes nothing");
}

#[test]
fn group_pause_and_line_ending_reach_documents() {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(""), None);
    let pause = |ed: &Editor, d| ed.document(d).unwrap().history().group_pause();
    assert_eq!(pause(&ed, doc), Duration::from_secs(1));
    ed.set_option_text(Scope::Global, "undo-group-pause-ms", "250")
        .unwrap();
    assert_eq!(pause(&ed, doc), Duration::from_millis(250));
    ed.set_option(
        Scope::Document(doc),
        OptionId::UNDO_GROUP_PAUSE_MS,
        OptionValue::Int(0),
    )
    .unwrap();
    assert_eq!(pause(&ed, doc), Duration::ZERO);
    // A document opened later gets the global value.
    let later = ed.open(Text::from(""), None);
    assert_eq!(pause(&ed, later), Duration::from_millis(250));

    ed.set_option_text(Scope::Global, "default-line-ending", "crlf")
        .unwrap();
    let empty = ed.open(Text::from("no break"), None);
    assert_eq!(ed.document(empty).unwrap().line_ending(), LineEnding::Crlf);
    let lf = ed.open(Text::from("a\nb\n"), None);
    assert_eq!(
        ed.document(lf).unwrap().line_ending(),
        LineEnding::Lf,
        "detected wins"
    );
    // The tab-width wrapper still works.
    ed.set_tab_width(NonZeroU8::new(2).unwrap());
    assert_eq!(ed.tab_width().get(), 2);
}
