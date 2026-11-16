//! Search and replace through keys and the command line.

use super::*;
use crate::{Event, KeyCode, KeyEvent, Modifiers};

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), Modifiers::NONE))
}

fn press(ed: &mut Editor, keys: &str) {
    for c in keys.chars() {
        ed.handle(&key(c));
    }
}

fn special(ed: &mut Editor, code: KeyCode) {
    ed.handle(&Event::Key(KeyEvent::plain(code)));
}

fn editor(s: &str) -> (Editor, ViewId) {
    let mut ed = Editor::new(80, 24);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    (ed, view)
}

fn primary(ed: &Editor, view: ViewId) -> (usize, usize) {
    let r = *ed.selection(view).unwrap().primary();
    (r.start(), r.end())
}

fn text(ed: &Editor, view: ViewId) -> String {
    let (_, d) = ed.view_document(view).unwrap();
    d.text().to_string()
}

#[test]
fn incremental_search_selects_as_typed_and_esc_restores() {
    let (mut ed, v) = editor("one foo two fob three foo");
    assert_eq!(primary(&ed, v), (0, 1));
    press(&mut ed, "/fo");
    // The first match after the cursor, while typing.
    assert_eq!(primary(&ed, v), (4, 6));
    press(&mut ed, "b");
    assert_eq!(primary(&ed, v), (12, 15));
    assert!(ed.search_highlight().is_some());
    special(&mut ed, KeyCode::Esc);
    assert!(ed.prompt().is_none());
    assert_eq!(primary(&ed, v), (0, 1), "Esc restores the selection");
    assert_eq!(ed.search_pattern(), "", "and the pattern");
    // Enter keeps the match; n and N move.
    press(&mut ed, "/foo");
    special(&mut ed, KeyCode::Enter);
    assert_eq!(primary(&ed, v), (4, 7));
    press(&mut ed, "n");
    assert_eq!(primary(&ed, v), (22, 25));
    press(&mut ed, "n");
    assert_eq!(primary(&ed, v), (4, 7), "wrapped");
    assert!(ed.message().contains("wrapped"), "{}", ed.message());
    press(&mut ed, "N");
    assert_eq!(primary(&ed, v), (22, 25));
    // A pattern with no match restores the selection and says so.
    press(&mut ed, "/zzz");
    assert_eq!(primary(&ed, v), (22, 25));
    assert!(ed.message().contains("no match"), "{}", ed.message());
    special(&mut ed, KeyCode::Esc);
}

#[test]
fn backward_search_and_an_invalid_pattern() {
    let (mut ed, v) = editor("ab ab ab");
    press(&mut ed, "/ab");
    special(&mut ed, KeyCode::Enter);
    press(&mut ed, "n");
    assert_eq!(primary(&ed, v), (6, 8));
    press(&mut ed, "?ab");
    assert_eq!(primary(&ed, v), (3, 5));
    special(&mut ed, KeyCode::Enter);
    // `n` repeats backward now; `N` forward.
    press(&mut ed, "n");
    assert_eq!(primary(&ed, v), (0, 2));
    press(&mut ed, "N");
    assert_eq!(primary(&ed, v), (3, 5));
    // An unfinished pattern is reported and changes nothing.
    press(&mut ed, "/(");
    assert_eq!(primary(&ed, v), (3, 5));
    assert!(ed.message().contains("pattern"), "{}", ed.message());
    special(&mut ed, KeyCode::Esc);
}

#[test]
fn extend_mode_adds_matches_and_star_searches_the_selection() {
    let (mut ed, v) = editor("x.y and x.y and xzy");
    // Select "x.y" (three graphemes), make it the pattern.
    press(&mut ed, "v");
    press(&mut ed, "ll");
    assert_eq!(primary(&ed, v), (0, 3));
    press(&mut ed, "*");
    assert_eq!(ed.search_pattern(), r"x\.y");
    // In extend mode n adds the next match as a new selection.
    press(&mut ed, "n");
    let ranges: Vec<(usize, usize)> = ed
        .selection(v)
        .unwrap()
        .ranges()
        .iter()
        .map(|r| (r.start(), r.end()))
        .collect();
    assert_eq!(ranges, [(0, 3), (8, 11)], "xzy is not a match");
    // Clearing the highlight keeps the pattern.
    ed.run_named(
        "search-clear",
        &crate::Context {
            view: None,
            count: None,
            arg: None,
        },
    );
    assert!(ed.search_highlight().is_none());
    assert_eq!(ed.search_pattern(), r"x\.y");
}

#[test]
fn replace_in_selections_and_in_the_whole_text() {
    let (mut ed, v) = editor("a=1 b=2\nc=3");
    ed.command_line(r"%s/(\w)=(\d)/$2-$1/");
    assert_eq!(text(&ed, v), "1-a 2-b\n3-c");
    assert_eq!(ed.message(), "3 replacements");
    // One undo step.
    ed.handle(&Event::Key(KeyEvent::new(
        KeyCode::Char('u'),
        Modifiers::NONE,
    )));
    assert_eq!(text(&ed, v), "a=1 b=2\nc=3");
    // In the selections only: select the first line, then :s.
    press(&mut ed, "x");
    ed.command_line("s/=/:/");
    assert_eq!(text(&ed, v), "a:1 b:2\nc=3");
    // Any delimiter, an escaped one, and the i flag.
    ed.command_line(r"%s#A:1#\#X#i");
    assert_eq!(text(&ed, v), "#X b:2\nc=3");
    ed.command_line("%s/zz/y/");
    assert_eq!(ed.message(), "no match for zz");
    ed.command_line("%s/x");
    assert!(ed.message().starts_with("usage"), "{}", ed.message());
}

#[test]
fn substitution_specs_parse() {
    assert_eq!(
        parse_substitution("/a/b/"),
        Some(("a".to_owned(), "b".to_owned()))
    );
    assert_eq!(
        parse_substitution("/a/b"),
        Some(("a".to_owned(), "b".to_owned()))
    );
    assert_eq!(
        parse_substitution("|a\\|b|c|i"),
        Some(("(?i)a|b".to_owned(), "c".to_owned()))
    );
    // Other escapes stay for the regex.
    assert_eq!(
        parse_substitution(r"/\d+/n/g"),
        Some((r"\d+".to_owned(), "n".to_owned()))
    );
    assert_eq!(parse_substitution("/a"), None);
    assert_eq!(parse_substitution("//b/"), None);
    assert_eq!(parse_substitution("/a/b/x"), None);
    assert_eq!(parse_substitution("aa/b/"), None);
}

/// A large document is searched by request; the result is applied if it
/// is the latest, mapped through edits made meanwhile.
#[test]
fn large_documents_are_searched_by_request() {
    let big = format!("{}needle{}", "x".repeat(INLINE_MAX), "y".repeat(10));
    let (mut ed, v) = editor(&big);
    press(&mut ed, "/needle");
    let jobs: Vec<SearchJob> = ed
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Search(job) => Some(job),
            _ => None,
        })
        .collect();
    // One request per keystroke; only the last counts.
    assert_eq!(jobs.len(), 6);
    assert!(ed.message().contains("searching"), "{}", ed.message());
    assert_eq!(primary(&ed, v), (0, 1), "nothing applied yet");
    // An older search's result is ignored.
    ed.search_done(jobs[0].run());
    assert_eq!(primary(&ed, v), (0, 1));
    ed.search_done(jobs[5].run());
    assert_eq!(primary(&ed, v), (INLINE_MAX, INLINE_MAX + 6));
    special(&mut ed, KeyCode::Enter);

    // A result for an older revision is mapped through the edit.
    press(&mut ed, "gg");
    press(&mut ed, "n");
    let job = match ed.take_requests().pop() {
        Some(Request::Search(job)) => job,
        other => panic!("{other:?}"),
    };
    press(&mut ed, "iab");
    special(&mut ed, KeyCode::Esc);
    ed.search_done(job.run());
    assert_eq!(primary(&ed, v), (INLINE_MAX + 2, INLINE_MAX + 8));

    // Esc while a search runs: its result is dropped.
    press(&mut ed, "gg/nee");
    let job = match ed.take_requests().pop() {
        Some(Request::Search(job)) => job,
        other => panic!("{other:?}"),
    };
    special(&mut ed, KeyCode::Esc);
    let before = primary(&ed, v);
    ed.search_done(job.run());
    assert_eq!(primary(&ed, v), before);
}
