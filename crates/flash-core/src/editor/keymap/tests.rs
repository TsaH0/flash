//! Key notation, layering, and resolution (against a naive resolver).

use proptest::prelude::*;

use super::*;

const N: ModeId = ModeId(0);
const I: ModeId = ModeId(1);

fn k(text: &str) -> KeyEvent {
    parse_key(text).unwrap()
}

fn seq(text: &str) -> KeySeq {
    parse_sequence(text).unwrap()
}

fn cmd(n: u32) -> Bind {
    Bind::Command {
        id: CommandId(n),
        arg: None,
    }
}

fn act(n: u32) -> Action {
    Action {
        command: CommandId(n),
        arg: None,
    }
}

fn at(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

const TIMEOUT: u64 = 1_000_000_000;

fn run(n: u32) -> Resolved {
    Resolved {
        first: None,
        step: Step::Run(act(n)),
    }
}

fn pending() -> Resolved {
    Resolved {
        first: None,
        step: Step::Pending,
    }
}

fn unbound() -> Resolved {
    Resolved {
        first: None,
        step: Step::Unbound,
    }
}

#[test]
fn notation_round_trips() {
    for text in [
        "x",
        "U",
        "C-s",
        "A-z",
        "C-A-x",
        "S-left",
        "ret",
        "esc",
        "tab",
        "backspace",
        "del",
        "pageup",
        "C-pagedown",
        "space",
        "minus",
        "F1",
        "F12",
        "A-C",
        "%",
        ";",
        ":",
        "老",
    ] {
        assert_eq!(key_name(k(text)), text, "{text}");
    }
    assert_eq!(k("C--"), KeyEvent::new(KeyCode::Char('-'), Modifiers::CTRL));
    assert_eq!(k("-"), KeyEvent::plain(KeyCode::Char('-')));
    assert_eq!(seq("g g").as_slice(), [k("g"), k("g")]);
    assert_eq!(seq("  C-x   y ").as_slice(), [k("C-x"), k("y")]);
    for bad in [
        "", "C-", "C-C-x", "S-a", "S-U", "foo", "F13", "F0", "x y", "\t", "c-x",
    ] {
        assert!(parse_key(bad).is_err(), "{bad:?}");
    }
    assert!(parse_sequence("   ").is_err());
    assert!(parse_sequence("g foo").is_err());
    // Shift on a character key is the character's business.
    let shifted = KeyEvent::new(KeyCode::Char('U'), Modifiers::SHIFT);
    assert_eq!(normalize(shifted), k("U"));
    let lower = KeyEvent::new(
        KeyCode::Char('z'),
        Modifiers {
            shift: true,
            ..Modifiers::ALT
        },
    );
    assert_eq!(normalize(lower), k("A-Z"));
    let digit = KeyEvent::new(KeyCode::Char('1'), Modifiers::SHIFT);
    assert_eq!(normalize(digit), k("1"), "no single uppercase: unchanged");
    assert_eq!(normalize(k("S-left")), k("S-left"));
}

#[test]
fn higher_layers_win_and_unbinding_hides_lower_ones() {
    let mut map = Keymap::default();
    map.set_layer(
        Owner::DEFAULTS,
        N,
        vec![(seq("x"), cmd(1)), (seq("y"), cmd(2)), (seq("g g"), cmd(3))],
    );
    map.set_layer(Owner(10), N, vec![(seq("x"), cmd(4))]);
    map.set_layer(
        Owner::CONFIG,
        N,
        vec![(seq("y"), Bind::Unbound), (seq("z"), cmd(5))],
    );
    let names = |m: &Keymap| -> Vec<(String, u32)> {
        m.bindings(N)
            .into_iter()
            .map(|(s, a)| {
                let keys: Vec<String> = s.iter().map(|&k| key_name(k)).collect();
                (keys.join(" "), a.command.0)
            })
            .collect()
    };
    assert_eq!(
        names(&map),
        [("g g".into(), 3), ("x".into(), 4), ("z".into(), 5)]
    );
    assert_eq!(map.lookup(N, &seq("x")), Some(&act(4)));
    assert_eq!(map.lookup(N, &seq("y")), None);
    assert_eq!(map.lookup(N, &seq("g")), None, "a prefix is not a binding");
    // The user's layer outranks plugins even when the plugin binds later.
    map.set_layer(Owner(11), N, vec![(seq("z"), cmd(6))]);
    assert_eq!(map.lookup(N, &seq("z")), Some(&act(5)));
    map.remove_owner(Owner::CONFIG);
    assert_eq!(map.lookup(N, &seq("y")), Some(&act(2)));
    assert_eq!(map.lookup(N, &seq("z")), Some(&act(6)));
    map.remove_owner(Owner(10));
    assert_eq!(map.lookup(N, &seq("x")), Some(&act(1)));
    // Modes are separate.
    assert_eq!(map.lookup(I, &seq("x")), None);
    map.remove_mode(N);
    assert_eq!(map.lookup(N, &seq("x")), None);
}

#[test]
fn sequences_prefixes_and_timeouts() {
    let mut map = Keymap::default();
    map.set_layer(
        Owner::DEFAULTS,
        N,
        vec![
            (seq("x"), cmd(1)),
            (seq("g g"), cmd(2)),
            (seq("g e"), cmd(3)),
            (seq("d"), cmd(4)),
            (seq("d d"), cmd(5)),
            (seq("C-w v s"), cmd(6)),
        ],
    );
    assert_eq!(map.feed(N, k("x"), at(0), TIMEOUT), run(1));
    assert_eq!(map.feed(N, k("g"), at(0), TIMEOUT), pending());
    assert_eq!(map.pending(), [k("g")]);
    assert_eq!(map.feed(N, k("e"), at(10), TIMEOUT), run(3));
    assert!(map.pending().is_empty());
    // A key that does not continue the sequence starts over.
    map.feed(N, k("g"), at(0), TIMEOUT);
    assert_eq!(map.feed(N, k("x"), at(10), TIMEOUT), run(1));
    map.feed(N, k("g"), at(0), TIMEOUT);
    assert_eq!(map.feed(N, k("q"), at(10), TIMEOUT), unbound());
    assert!(map.pending().is_empty());
    // An ambiguous prefix: its action runs when the next key does not
    // continue it, or comes too late.
    map.feed(N, k("d"), at(0), TIMEOUT);
    assert_eq!(map.feed(N, k("d"), at(999), TIMEOUT), run(5));
    map.feed(N, k("d"), at(0), TIMEOUT);
    assert_eq!(
        map.feed(N, k("x"), at(5), TIMEOUT),
        Resolved {
            first: Some(act(4)),
            step: Step::Run(act(1)),
        }
    );
    map.feed(N, k("d"), at(0), TIMEOUT);
    assert_eq!(
        map.feed(N, k("d"), at(1001), TIMEOUT),
        Resolved {
            first: Some(act(4)),
            step: Step::Pending,
        },
        "too late: the old d runs, the new d starts a sequence"
    );
    // The timeout counts from the last key, not the first.
    map.cancel();
    map.feed(N, k("C-w"), at(0), TIMEOUT);
    map.feed(N, k("v"), at(900), TIMEOUT);
    assert_eq!(map.feed(N, k("s"), at(1800), TIMEOUT), run(6));
    // A pending sequence does not survive a mode change or recompiling.
    map.feed(N, k("g"), at(0), TIMEOUT);
    assert_eq!(map.feed(I, k("g"), at(1), TIMEOUT), unbound());
    assert!(map.pending().is_empty());
    map.feed(N, k("g"), at(0), TIMEOUT);
    map.set_layer(Owner::CONFIG, N, vec![(seq("e"), cmd(9))]);
    assert!(map.pending().is_empty());
    assert_eq!(map.feed(N, k("e"), at(1), TIMEOUT), run(9));
    // Shifted characters match their binding.
    map.set_layer(Owner::CONFIG, N, vec![(seq("U"), cmd(7))]);
    let shifted = KeyEvent::new(KeyCode::Char('U'), Modifiers::SHIFT);
    assert_eq!(map.feed(N, shifted, at(0), TIMEOUT), run(7));
}

// ---- property: the trie resolver against a naive one ----

/// The spec, over a plain list of bindings and a buffer.
#[derive(Default)]
struct Naive {
    buf: Vec<KeyEvent>,
    last: u64,
}

impl Naive {
    fn feed(&mut self, binds: &[(Vec<KeyEvent>, u32)], key: KeyEvent, now: u64) -> Resolved {
        let key = normalize(key);
        let exact = |s: &[KeyEvent]| binds.iter().find(|(b, _)| b == s).map(|&(_, n)| act(n));
        let extends = |s: &[KeyEvent]| {
            binds
                .iter()
                .any(|(b, _)| b.len() > s.len() && b.starts_with(s))
        };
        let mut first = None;
        if !self.buf.is_empty() {
            let mut longer = self.buf.clone();
            longer.push(key);
            let continued = exact(&longer).is_some() || extends(&longer);
            if now.saturating_sub(self.last) <= TIMEOUT && continued {
                self.buf = longer;
            } else {
                first = exact(&self.buf);
                self.buf.clear();
            }
        }
        if self.buf.is_empty() {
            if exact(&[key]).is_none() && !extends(&[key]) {
                return Resolved {
                    first,
                    step: Step::Unbound,
                };
            }
            self.buf.push(key);
        }
        self.last = now;
        if extends(&self.buf) {
            Resolved {
                first,
                step: Step::Pending,
            }
        } else {
            let step = exact(&self.buf).map_or(Step::Unbound, Step::Run);
            self.buf.clear();
            Resolved { first, step }
        }
    }
}

fn alphabet() -> Vec<KeyEvent> {
    ["a", "b", "C-a", "esc"].iter().map(|t| k(t)).collect()
}

fn bindings() -> impl Strategy<Value = Vec<(Vec<usize>, u32)>> {
    prop::collection::vec((prop::collection::vec(0..4usize, 1..4), 0..50u32), 0..10)
}

proptest! {
    #[test]
    fn trie_matches_the_naive_resolver(
        binds in bindings(),
        keys in prop::collection::vec((0..4usize, 0..1500u64), 0..60),
    ) {
        let abc = alphabet();
        // Later duplicates win, as later entries in one layer do.
        let mut list: Vec<(Vec<KeyEvent>, u32)> = Vec::new();
        for (s, n) in &binds {
            let s: Vec<KeyEvent> = s.iter().map(|&i| abc[i]).collect();
            list.retain(|(b, _)| *b != s);
            list.push((s, *n));
        }
        let mut map = Keymap::default();
        let layer: Vec<(KeySeq, Bind)> = list
            .iter()
            .map(|(s, n)| (s.iter().copied().collect(), cmd(*n)))
            .collect();
        map.set_layer(Owner::DEFAULTS, N, layer);
        let mut naive = Naive::default();
        let mut now = 0;
        for (i, gap) in keys {
            now += gap * 1_000_000;
            let key = abc[i];
            let want = naive.feed(&list, key, now);
            let got = map.feed(N, key, Timestamp::from_nanos(now), TIMEOUT);
            prop_assert_eq!(got, want, "key {} at {}", key_name(key), now);
            prop_assert_eq!(map.pending(), naive.buf.as_slice());
        }
    }
}
