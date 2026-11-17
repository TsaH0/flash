//! Renderer tests against a minimal terminal emulator.

use std::num::NonZeroU8;
use std::path::PathBuf;

use flash_core::{Editor, Event, KeyCode, KeyEvent, Modifiers, Range, Selection, Text};

use super::Renderer;
use crate::emit::Strategy;
use flash_core::width::grapheme_width;

/// What a terminal shows: characters (`'\0'` for the right half of a
/// wide character), reverse-video and underline flags, and the cursor.
/// Frames are applied in turn, as a terminal would see them, so a frame
/// that writes only what changed is checked against the screen it
/// changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Screen {
    cells: Vec<Vec<char>>,
    reversed: Vec<Vec<bool>>,
    underlined: Vec<Vec<bool>>,
    cursor: Option<(usize, usize)>,
    /// The last DECSCUSR cursor style (2 block, 4 underline, 6 bar).
    cursor_style: Option<usize>,
    term: Term,
}

/// Cells, reverse and underline flags, and the cursor.
type Shows<'a> = (
    &'a [Vec<char>],
    &'a [Vec<bool>],
    &'a [Vec<bool>],
    Option<(usize, usize)>,
);

/// The emulated terminal's state between frames.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Term {
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    rev: bool,
    under: bool,
    shown: bool,
    per_char: bool,
}

impl Screen {
    /// A blank terminal. With `per_char`, one that does not join
    /// clusters: every char takes its own width, and cells past the right
    /// edge are clipped.
    fn new(w: usize, h: usize, per_char: bool) -> Screen {
        Screen {
            cells: vec![vec![' '; w]; h],
            reversed: vec![vec![false; w]; h],
            underlined: vec![vec![false; w]; h],
            cursor: None,
            cursor_style: None,
            term: Term {
                w,
                h,
                x: 0,
                y: 0,
                rev: false,
                under: false,
                shown: false,
                per_char,
            },
        }
    }

    /// A terminal that measures grapheme clusters as `unicode-width`
    /// does (as the renderer assumes), after one frame.
    fn of(bytes: &[u8], w: usize, h: usize) -> Screen {
        Screen::emulate(bytes, w, h, false)
    }

    fn emulate(bytes: &[u8], w: usize, h: usize, per_char: bool) -> Screen {
        let mut s = Screen::new(w, h, per_char);
        s.apply(bytes);
        s
    }

    /// What the screen shows (not how the terminal got there).
    fn shows(&self) -> Shows<'_> {
        (&self.cells, &self.reversed, &self.underlined, self.cursor)
    }

    /// Interprets a frame: CUP, EL, ED, SGR (attributes, colors skipped),
    /// cursor show/hide and style, synchronized update.
    fn apply(&mut self, bytes: &[u8]) {
        let s = std::str::from_utf8(bytes).expect("frames are UTF-8");
        let (w, h, per_char) = (self.term.w, self.term.h, self.term.per_char);
        let mut prev: Option<char> = None;
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            let t = &mut self.term;
            if c == '\u{1b}' {
                prev = None;
                assert_eq!(chars.next(), Some('['), "only CSI escapes in frames");
                let mut params = String::new();
                let final_byte = loop {
                    let c = chars.next().expect("unterminated CSI");
                    if c.is_ascii_alphabetic() {
                        break c;
                    }
                    params.push(c);
                };
                let nums: Vec<usize> = params
                    .trim_start_matches('?')
                    .split(';')
                    .filter_map(|p| p.parse().ok())
                    .collect();
                match final_byte {
                    'H' => {
                        t.y = nums.first().copied().unwrap_or(1) - 1;
                        t.x = nums.get(1).copied().unwrap_or(1) - 1;
                    }
                    'K' => {
                        for i in t.x..w {
                            self.cells[t.y][i] = ' ';
                            self.reversed[t.y][i] = false;
                            self.underlined[t.y][i] = false;
                        }
                    }
                    'J' => {
                        self.cells = vec![vec![' '; w]; h];
                        self.reversed = vec![vec![false; w]; h];
                        self.underlined = vec![vec![false; w]; h];
                    }
                    'm' => {
                        let nums = if nums.is_empty() { vec![0] } else { nums };
                        let mut i = 0;
                        while i < nums.len() {
                            match nums[i] {
                                0 => {
                                    t.rev = false;
                                    t.under = false;
                                }
                                27 => t.rev = false,
                                7 => t.rev = true,
                                4 => t.under = true,
                                24 => t.under = false,
                                // Extended colors: skip their arguments.
                                38 | 48 if nums.get(i + 1) == Some(&5) => i += 2,
                                38 | 48 if nums.get(i + 1) == Some(&2) => i += 4,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                    'q' if params.ends_with(' ') => {
                        self.cursor_style = params.trim_end().parse().ok();
                    }
                    'h' if params == "?25" => t.shown = true,
                    'l' if params == "?25" => t.shown = false,
                    'h' | 'l' => {}
                    other => panic!("unexpected CSI {params}{other}"),
                }
                continue;
            }
            assert!(!c.is_control(), "raw control {c:?} in a frame");
            // A cluster-aware terminal joins a char into the previous cell
            // when it continues a cluster (ZWJ, emoji after ZWJ, a second
            // regional indicator, variation selectors, combining marks).
            let mut buf = [0; 4];
            let mut cw = grapheme_width(c.encode_utf8(&mut buf), 0, NonZeroU8::MIN);
            if !per_char && joins(prev, c) {
                cw = 0;
            }
            prev = Some(c);
            if cw == 0 {
                continue;
            }
            if per_char && t.x + cw > w {
                t.x = w;
                continue;
            }
            assert!(
                t.x + cw <= w,
                "{c:?} drawn past the right edge at {}, row {} of {h}: {:?}",
                t.x,
                t.y,
                self.cells[t.y].iter().collect::<String>()
            );
            let (x, y) = (t.x, t.y);
            self.cells[y][x] = c;
            self.reversed[y][x] = t.rev;
            self.underlined[y][x] = t.under;
            if cw == 2 {
                self.cells[y][x + 1] = '\0';
                self.reversed[y][x + 1] = t.rev;
                self.underlined[y][x + 1] = t.under;
            }
            t.x += cw;
        }
        self.cursor = self.term.shown.then_some((self.term.x, self.term.y));
    }

    fn row(&self, y: usize) -> String {
        self.cells[y]
            .iter()
            .filter(|&&c| c != '\0')
            .collect::<String>()
            .trim_end()
            .to_owned()
    }
}

/// Whether `c` continues the cluster `prev` ended (enough for the test
/// pieces: ZWJ sequences and regional-indicator pairs).
fn joins(prev: Option<char>, c: char) -> bool {
    let ri = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
    match prev {
        Some('\u{200D}') => true,
        Some(p) if ri(p) && ri(c) => true,
        _ => c == '\u{200D}',
    }
}

fn plain(code: KeyCode) -> Event {
    Event::Key(KeyEvent::plain(code))
}

/// An editor showing `s`, in insert mode with a cursor at 0: these cases
/// draw the Stage 6 point cursor (normal mode's block has its own test).
fn editor_with(s: &str, width: u16, height: u16) -> Editor {
    let mut ed = Editor::new(width, height);
    let doc = ed.open(Text::from(s), None);
    let view = ed.open_view(doc).unwrap();
    ed.set_mode(view, flash_core::ModeId::INSERT);
    ed.set_selection(view, Selection::point(0)).unwrap();
    ed
}

fn frame(ed: &Editor) -> Vec<u8> {
    Renderer::new().frame(ed, 1234).to_vec()
}

fn screen(ed: &Editor) -> Screen {
    let (w, h) = ed.screen();
    Screen::of(&frame(ed), usize::from(w), usize::from(h))
}

#[test]
fn draws_lines_status_and_cursor() {
    let mut ed = editor_with("hello\nworld", 40, 4);
    ed.handle(&plain(KeyCode::Down));
    ed.handle(&plain(KeyCode::End));
    let s = screen(&ed);
    assert_eq!(s.row(0), "hello");
    assert_eq!(s.row(1), "world");
    assert_eq!(s.row(2), "");
    assert!(
        s.row(3).starts_with("INSERT  [scratch]  2:6"),
        "{}",
        s.row(3)
    );
    assert!(!s.row(3).contains("µs"), "no latency unless asked for");
    assert_eq!(s.cursor_style, Some(6), "a bar in insert mode");
    assert!(s.reversed[3][0], "status line is reversed");
    assert_eq!(s.cursor, Some((5, 1)));
    let bytes = frame(&ed);
    assert!(bytes.starts_with(b"\x1b[?2026h"), "synchronized update");
    assert!(bytes.ends_with(b"\x1b[?2026l"));
}

#[test]
fn status_shows_path_modified_cursors_and_message() {
    let mut ed = Editor::new(70, 3);
    let doc = ed.open(Text::from("ab"), Some(PathBuf::from("dir/a.txt")));
    let view = ed.open_view(doc).unwrap();
    ed.set_mode(view, flash_core::ModeId::INSERT);
    ed.set_selection(view, Selection::point(0)).unwrap();
    ed.handle(&plain(KeyCode::Char('x')));
    ed.set_selection(
        view,
        Selection::new([Range::point(0), Range::point(2)], 0).unwrap(),
    )
    .unwrap();
    ed.handle(&Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        Modifiers::CTRL,
    )));
    let status = screen(&ed).row(2);
    assert!(
        status.starts_with("INSERT  dir/a.txt [+]  2 cursors  1:1"),
        "{status}"
    );
    assert!(status.ends_with("nothing selected"), "{status}");
}

#[test]
fn replaces_control_characters_and_drops_bidi_overrides() {
    let ed = editor_with("a\u{1b}[2Jb\u{7}c\u{202E}d", 80, 3);
    let bytes = frame(&ed);
    // Only the renderer's own clear of a first frame.
    let clears = |b: &[u8]| b.windows(4).filter(|w| *w == b"\x1b[2J").count();
    assert_eq!(clears(&bytes), 1);
    assert!(!bytes.contains(&0x07));
    let s = Screen::of(&bytes, 80, 3);
    assert_eq!(s.row(0), "a\u{fffd}[2Jb\u{fffd}cd");
    let mut ed = Editor::new(80, 3);
    let doc = ed.open(
        Text::from("x"),
        Some(PathBuf::from("evil\u{1b}[2J\u{202E}")),
    );
    ed.open_view(doc).unwrap();
    let bytes = frame(&ed);
    assert_eq!(clears(&bytes), 1);
    assert!(!String::from_utf8_lossy(&bytes).contains('\u{202E}'));
}

#[test]
fn tabs_expand_to_the_tab_width() {
    let mut ed = editor_with("\tx\n12\ty", 20, 3);
    assert_eq!(screen(&ed).row(0), "    x");
    assert_eq!(screen(&ed).row(1), "12  y");
    ed.set_tab_width(NonZeroU8::new(8).unwrap());
    assert_eq!(screen(&ed).row(0), "        x");
    assert_eq!(screen(&ed).row(1), "12      y");
}

#[test]
fn horizontal_scroll_and_wide_characters_at_the_edges() {
    // 10 columns; the line is 0123456789 then a wide 漢 then abc.
    let mut ed = editor_with("0123456789漢abc", 10, 2);
    // Without scrolling, 漢 does not fit (columns 10-11): not drawn.
    let s = screen(&ed);
    assert_eq!(s.row(0), "0123456789");
    assert_eq!(s.cursor, Some((0, 0)));
    // End: the cursor (column 15) is shown; left = 6, so 漢 is at 4-5.
    ed.handle(&plain(KeyCode::End));
    let s = screen(&ed);
    assert_eq!(ed.view(ed.focus().unwrap()).unwrap().left(), 6);
    assert_eq!(s.row(0), "6789漢abc");
    assert_eq!(s.cursor, Some((9, 0)));
    // The left edge cutting 漢 in half: a longer line, cursor moved to
    // column 20, so left = 11 (the right half of 漢 at columns 10-11).
    let mut ed = editor_with("0123456789漢abcdefghijklmnop", 10, 2);
    for _ in 0..19 {
        ed.handle(&plain(KeyCode::Right));
    }
    assert_eq!(ed.view(ed.focus().unwrap()).unwrap().left(), 11);
    let s = screen(&ed);
    assert_eq!(s.row(0), " abcdefghi");
    assert!(!s.cells[0].contains(&'漢'));
    assert_eq!(s.cursor, Some((9, 0)));
}

#[test]
fn columns_after_a_disputed_cluster_are_right_on_any_terminal() {
    // The family is 2 cells for unicode-width, 6 on a terminal that does
    // not join ZWJ sequences; "x" belongs at column 3 either way.
    let ed = editor_with("c\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}x", 20, 2);
    let bytes = frame(&ed);
    for per_char in [false, true] {
        let s = Screen::emulate(&bytes, 20, 2, per_char);
        assert_eq!(s.cells[0][3], 'x', "per_char {per_char}: {:?}", s.cells[0]);
    }
    // A plain wide character needs no repositioning.
    let ed = editor_with("漢x", 20, 2);
    let n = frame(&ed).windows(2).filter(|w| w == b"\x1b[").count();
    let m = frame(&editor_with("ab", 20, 2))
        .windows(2)
        .filter(|w| w == b"\x1b[")
        .count();
    assert_eq!(n, m);
}

#[test]
fn the_layout_cache_matches_walking_from_the_line_start() {
    // Long lines with wide characters, combining marks, and tabs,
    // scrolled to many positions.
    let line = "ab漢e\u{301}cd😀xyz\u{1F1FA}\u{1F1F8}".repeat(30);
    for text in [
        line.clone(),
        format!("{line}\t{line}"),
        format!("x\n{line}\ny"),
    ] {
        let mut ed = editor_with(&text, 17, 4);
        for i in 0..400 {
            let key = if i % 97 < 60 {
                KeyCode::Right
            } else {
                KeyCode::Left
            };
            ed.handle(&plain_key(key));
            let cached = Renderer::new().frame(&ed, 0).to_vec();
            let mut plain = Renderer::new();
            plain.use_layout = false;
            assert_eq!(plain.frame(&ed, 0), &cached[..], "step {i}");
        }
    }
    // The cache is used: a long line starts drawing near the left column,
    // not at the line start.
    let mut ed = editor_with(&"a".repeat(5000), 10, 2);
    ed.handle(&plain_key(KeyCode::End));
    let (view, doc) = ed.view_document(ed.focus().unwrap()).unwrap();
    assert_eq!(view.cursor().unwrap().column, 5000);
    let (offset, column) = view.layout().start_before(doc, 0, view.left());
    assert_eq!(offset, column);
    assert!(
        column > 0 && view.left() - column <= 1100,
        "{column} for {}",
        view.left()
    );
}

fn soft_wrapped(s: &str, width: u16, height: u16) -> Editor {
    let mut ed = editor_with(s, width, height);
    ed.set_option(
        flash_core::Scope::Global,
        flash_core::OptionId::SOFT_WRAP,
        flash_core::OptionValue::Bool(true),
    )
    .unwrap();
    ed
}

#[test]
fn soft_wrap_draws_rows_and_the_cursor_on_its_row() {
    let mut ed = soft_wrapped("hello world foo\nbar", 11, 5);
    let s = screen(&ed);
    assert_eq!(s.row(0).trim_end(), "hello");
    assert_eq!(s.row(1).trim_end(), "world foo");
    assert_eq!(s.row(2).trim_end(), "bar");
    assert_eq!(s.cursor, Some((0, 0)));
    // The cursor on "foo" is on the second row.
    for _ in 0..12 {
        ed.handle(&plain_key(KeyCode::Right));
    }
    assert_eq!(screen(&ed).cursor, Some((6, 1)));
    // Down goes to the next logical line.
    ed.handle(&plain_key(KeyCode::Down));
    assert_eq!(screen(&ed).cursor, Some((3, 2)));
    let view = ed.view(ed.focus().unwrap()).unwrap();
    assert_eq!(view.left(), 0);
}

#[test]
fn soft_wrap_scrolls_by_rows_inside_a_long_line() {
    // One line of 50 rows in a 3-row view: the cursor at its end shows
    // the last rows of the line.
    let line = "abcdefghij".repeat(50);
    let mut ed = soft_wrapped(&line, 10, 4);
    ed.handle(&plain_key(KeyCode::End));
    let view = ed.view(ed.focus().unwrap()).unwrap();
    assert_eq!((view.top(), view.top_row()), (0, 48));
    let s = screen(&ed);
    assert_eq!(s.row(0), "abcdefghij");
    assert_eq!(s.row(1), "abcdefghij");
    // The cursor cell after the full last row is on an empty row.
    assert_eq!(s.row(2).trim_end(), "");
    assert_eq!(s.cursor, Some((0, 2)));
    ed.handle(&plain_key(KeyCode::Home));
    let view = ed.view(ed.focus().unwrap()).unwrap();
    assert_eq!((view.top(), view.top_row()), (0, 0));
}

#[test]
fn search_matches_are_underlined_until_cleared() {
    let mut ed = editor_with("foo bar foo\nxfoo", 20, 4);
    // Normal mode, as typed.
    ed.handle(&plain_key(KeyCode::Esc));
    for c in "/foo".chars() {
        ed.handle(&plain_key(KeyCode::Char(c)));
    }
    ed.handle(&plain_key(KeyCode::Enter));
    let s = screen(&ed);
    let under = |y: usize| -> String {
        (0..20)
            .map(|x| if s.underlined[y][x] { 'u' } else { '.' })
            .collect()
    };
    assert_eq!(under(0), "uuu.....uuu.........");
    assert_eq!(under(1), ".uuu................");
    ed.run_named(
        "search-clear",
        &flash_core::Context {
            view: None,
            count: None,
            arg: None,
        },
    );
    let s = screen(&ed);
    assert!(s.underlined.iter().flatten().all(|&u| !u));
}

#[test]
fn the_latency_is_on_the_status_line_when_asked_for() {
    let mut ed = editor_with("x", 40, 3);
    ed.set_option(
        flash_core::Scope::Global,
        flash_core::OptionId::STATUS_LATENCY,
        flash_core::OptionValue::Bool(true),
    )
    .unwrap();
    assert!(screen(&ed).row(2).contains("  1 µs"), "{}", screen(&ed).row(2));
}

/// A cursor move rewrites the status line's position and moves the
/// cursor: tens of bytes, not a screen.
#[test]
fn small_changes_write_few_bytes() {
    let text: String = (0..30).map(|i| format!("line {i} of some text\n")).collect();
    let mut ed = editor_with(&text, 80, 24);
    let mut r = Renderer::new();
    let first = r.frame(&ed, 0).len();
    assert!(first > 500, "the first frame is whole: {first}");
    ed.handle(&plain_key(KeyCode::Right));
    let moved = r.frame(&ed, 0).len();
    assert!(moved < 64, "a cursor move wrote {moved} bytes");
    ed.handle(&plain_key(KeyCode::Char('x')));
    let typed = r.frame(&ed, 0).len();
    assert!(typed < 120, "a character typed wrote {typed} bytes");
    // Nothing changed: nothing written.
    assert_eq!(r.frame(&ed, 0).len(), 0);
    assert_eq!(r.stats().cells, 0);
    // After invalidating, everything again.
    r.invalidate();
    assert!(r.frame(&ed, 0).len() > 500);
}

/// Split views are drawn in their tiles with a separator, the cursor in
/// the focused one.
#[test]
fn split_views_are_composed() {
    let mut ed = editor_with("left\ntext", 21, 7);
    let cx = flash_core::Context {
        view: None,
        count: None,
        arg: None,
    };
    ed.run_named("vsplit", &cx);
    let s = screen(&ed);
    assert_eq!(s.cells[0][10], '│');
    assert_eq!(s.row(0), "left      │left");
    assert_eq!(s.cursor, Some((11, 0)), "in the focused (right) view");
    ed.run_named("split", &cx);
    let s = screen(&ed);
    // The right half split again: a separator row naming its document.
    assert!(s.row(3).contains("│ [scratch]"), "{:?}", s.row(3));
    assert_eq!(s.cursor, Some((11, 4)));
    ed.run_named("only", &cx);
    let s = screen(&ed);
    assert_eq!(s.row(0), "left");
}

/// A theme's colors reach the terminal.
#[test]
fn theme_colors_are_written() {
    let mut ed = editor_with("abc", 20, 3);
    ed.load_config(
        "test",
        Some("[theme]\n\"ui.statusline\" = { fg = \"red\", bg = \"#102030\", modifiers = [\"bold\"] }\n"),
    );
    assert_eq!(ed.config_errors(), []);
    let bytes = frame(&ed);
    let s = String::from_utf8_lossy(&bytes);
    assert!(s.contains("\x1b[0;1;31;48;2;16;32;48m"), "{s:?}");
    let screen = Screen::of(&bytes, 20, 3);
    assert!(!screen.reversed[2][0], "the user's style replaces reverse");
}

fn plain_key(code: KeyCode) -> Event {
    plain(code)
}

#[test]
fn a_wide_character_cut_by_the_right_edge_is_spaces() {
    let ed = editor_with("a漢", 2, 3);
    let s = screen(&ed);
    assert_eq!(s.row(0), "a");
    assert!(!s.cells[0].contains(&'漢'));
}

#[test]
fn selections_and_secondary_cursors_are_reversed() {
    let mut ed = editor_with("abcdef\nxy", 20, 4);
    let view = ed.focus().unwrap();
    let sel = Selection::new([Range::new(1, 3), Range::point(5), Range::point(9)], 0).unwrap();
    ed.set_selection(view, sel).unwrap();
    let s = screen(&ed);
    let rev: Vec<bool> = s.reversed[0][..7].to_vec();
    assert_eq!(rev, [false, true, true, false, false, true, false]);
    // A secondary cursor at the end of line 2 is a reversed space.
    assert!(s.reversed[1][2]);
    assert_eq!(s.cursor, Some((3, 0)), "primary head");
}

#[test]
fn the_prompt_replaces_the_status_line_and_scrolls() {
    let mut ed = editor_with("text", 12, 3);
    ed.handle(&Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        Modifiers::CTRL,
    )));
    for c in "abc".chars() {
        ed.handle(&plain(KeyCode::Char(c)));
    }
    let s = screen(&ed);
    assert_eq!(s.row(2), "open: abc");
    assert!(!s.reversed[2][0]);
    assert_eq!(s.cursor, Some((9, 2)));
    // Longer than the screen: the tail with the cursor is shown.
    for c in "defghijk".chars() {
        ed.handle(&plain(KeyCode::Char(c)));
    }
    let s = screen(&ed);
    assert_eq!(s.row(2), "abcdefghijk");
    assert_eq!(s.cursor, Some((11, 2)));
    assert_eq!(s.row(0), "text");
}

#[test]
fn without_focus_the_screen_is_cleared() {
    let mut ed = editor_with("hello", 20, 5);
    let view = ed.focus().unwrap();
    ed.close_view(view).unwrap();
    let s = screen(&ed);
    assert_eq!(s.row(0), "");
    assert_eq!(s.cursor, None);
}

#[test]
fn scrolls_to_the_primary_cursor() {
    let text: String = (0..100).map(|i| format!("line{i}\n")).collect();
    let mut ed = editor_with(&text, 40, 10);
    ed.handle(&Event::Key(KeyEvent::new(KeyCode::End, Modifiers::CTRL)));
    let s = screen(&ed);
    assert_eq!(s.row(7), "line99");
    assert_eq!(s.cursor, Some((0, 8)));
}

#[test]
fn survives_tiny_terminals() {
    for s in ["", "abc", "漢字\tx"] {
        for (w, h) in [(0, 0), (1, 1), (2, 1), (1, 5), (0, 5), (5, 0)] {
            let mut ed = editor_with(s, 80, 24);
            ed.handle(&Event::Resize {
                width: w,
                height: h,
            });
            Screen::of(&frame(&ed), w.into(), h.into());
            ed.handle(&plain(KeyCode::Char('z')));
            ed.handle(&plain(KeyCode::End));
            Screen::of(&frame(&ed), w.into(), h.into());
            ed.handle(&Event::Key(KeyEvent::new(
                KeyCode::Char('o'),
                Modifiers::CTRL,
            )));
            ed.handle(&plain(KeyCode::Char('漢')));
            Screen::of(&frame(&ed), w.into(), h.into());
        }
    }
}

/// A hostile trace can resize to 65535×65535: rendering stays bounded
/// (one cursor move and clear per row, nothing per empty cell).
#[test]
fn survives_a_huge_screen() {
    let mut ed = editor_with("abc\n漢字", 80, 24);
    ed.handle(&Event::Resize {
        width: u16::MAX,
        height: u16::MAX,
    });
    ed.handle(&Event::Key(KeyEvent::new(KeyCode::End, Modifiers::CTRL)));
    let start = std::time::Instant::now();
    let bytes = frame(&ed);
    assert!(bytes.len() < 2 << 20, "{} bytes", bytes.len());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

/// Random keys, pastes, and resizes over Unicode-heavy texts, rendered
/// after every event through the emulator: never panics, never draws past
/// the right edge, and no raw control character reaches the output.
#[test]
fn random_events_render_cleanly() {
    const PIECES: &[&str] = &[
        "a",
        "xyz ",
        "\n",
        "\r\n",
        "\r",
        "\t",
        "é",
        "e\u{301}",
        "老虎",
        "😀",
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "\u{1F1FA}\u{1F1F8}",
        "\u{7}",
        "\u{1b}[31m",
        "\u{202E}",
        "\u{200B}",
    ];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n) as usize
    };
    let codes = [
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Esc,
        KeyCode::Char('x'),
        KeyCode::Char('o'),
        KeyCode::Char('c'),
        KeyCode::Char('v'),
        KeyCode::Char('é'),
        KeyCode::Char('漢'),
        KeyCode::Char('\u{301}'),
    ];
    let mods = [
        Modifiers::NONE,
        Modifiers::SHIFT,
        Modifiers::CTRL,
        Modifiers::ALT,
    ];
    for run in 0..150 {
        let text: String = (0..next(40))
            .map(|_| PIECES[next(PIECES.len() as u64)])
            .collect();
        let mut ed = editor_with(&text, 1 + next(30) as u16, 1 + next(8) as u16);
        if next(2) == 0 {
            ed.set_option(
                flash_core::Scope::Global,
                flash_core::OptionId::SOFT_WRAP,
                flash_core::OptionValue::Bool(true),
            )
            .unwrap();
        }
        let mut renderer = Renderer::new();
        // Every strategy keeps the screen right.
        renderer.set_strategy([Strategy::Cells, Strategy::Rows, Strategy::Full][run % 3]);
        if next(3) == 0 {
            ed.run_named(
                if next(2) == 0 { "split" } else { "vsplit" },
                &flash_core::Context {
                    view: None,
                    count: None,
                    arg: None,
                },
            );
        }
        let (w0, h0) = ed.screen();
        let mut screen = Screen::new(w0.into(), h0.into(), false);
        for _ in 0..100 {
            let event = match next(20) {
                0 => Event::Paste {
                    text: PIECES[next(PIECES.len() as u64)].to_owned(),
                },
                1 => Event::Resize {
                    width: next(12) as u16,
                    height: next(5) as u16,
                },
                _ => Event::Key(KeyEvent::new(
                    codes[next(codes.len() as u64)],
                    mods[next(mods.len() as u64)],
                )),
            };
            ed.handle(&event);
            ed.take_requests();
            let (w, h) = ed.screen();
            if (usize::from(w), usize::from(h)) != (screen.term.w, screen.term.h) {
                // A resize: the terminal is a new size (the renderer
                // writes a whole frame).
                screen = Screen::new(w.into(), h.into(), false);
            }
            let bytes = renderer.frame(&ed, 0).to_vec();
            screen.apply(&bytes);
            // What the changes left on screen is what a whole frame, laid
            // out without the cache, shows.
            let mut plain_renderer = Renderer::new();
            plain_renderer.use_layout = false;
            let full = Screen::of(plain_renderer.frame(&ed, 0), w.into(), h.into());
            if full.shows() != screen.shows() {
                let f = ed.focus().unwrap();
                let (v, d) = ed.view_document(f).unwrap();
                for y in 0..full.cells.len() {
                    eprintln!(
                        "{y}: full {:?} {:?} | diffed {:?} {:?}",
                        full.cells[y].iter().collect::<String>(),
                        full.reversed[y].iter().map(|&r| if r { 'r' } else { '.' }).collect::<String>(),
                        screen.cells[y].iter().collect::<String>(),
                        screen.reversed[y].iter().map(|&r| if r { 'r' } else { '.' }).collect::<String>(),
                    );
                }
                eprintln!("cursor full {:?} diffed {:?}; strategy {}; frame {:?}", full.cursor, screen.cursor, run % 3, String::from_utf8_lossy(&bytes));
                panic!(
                    "after {event:?}: text {:?} sel {:?} left {} top {} size {:?} cursor {:?}",
                    d.text().to_string(),
                    d.selection(f),
                    v.left(),
                    v.top(),
                    v.size(),
                    v.cursor()
                );
            }
        }
    }
}

/// Normal mode: the cursor cell is the block's grapheme (not the head
/// after it), the status line names the mode and the keys typed so far,
/// and the cursor is a block; the prompt's is a bar.
#[test]
fn normal_mode_draws_the_block_and_shows_the_mode() {
    let mut ed = Editor::new(40, 3);
    let doc = ed.open(Text::from("ab漢cd\nxy"), None);
    ed.open_view(doc).unwrap();
    let s = screen(&ed);
    assert_eq!(s.cursor, Some((0, 0)));
    assert!(s.reversed[0][0] && !s.reversed[0][1], "the block is drawn");
    assert!(
        s.row(2).starts_with("NORMAL  [scratch]  1:1"),
        "{}",
        s.row(2)
    );
    assert_eq!(s.cursor_style, Some(2), "a steady block");
    for c in ['l', 'l'] {
        ed.handle(&plain(KeyCode::Char(c)));
    }
    let s = screen(&ed);
    assert_eq!(s.cursor, Some((2, 0)), "on 漢, not after it");
    assert!(
        s.row(2).starts_with("NORMAL  [scratch]  1:3"),
        "{}",
        s.row(2)
    );
    ed.handle(&plain(KeyCode::Char('2')));
    ed.handle(&plain(KeyCode::Char('g')));
    assert!(
        screen(&ed).row(2).starts_with("NORMAL 2 g  "),
        "{}",
        screen(&ed).row(2)
    );
    ed.handle(&plain(KeyCode::Esc));
    ed.handle(&plain(KeyCode::Char(':')));
    let s = screen(&ed);
    assert_eq!(s.row(2), ":");
    assert_eq!(s.cursor_style, Some(6), "the prompt's bar");
}
