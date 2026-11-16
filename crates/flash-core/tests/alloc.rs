//! PLAN Stage 7: key resolution allocates nothing, and neither does a
//! normal-mode motion key through `Editor::handle` (resolution, command,
//! mode minimum, hooks, scroll). Counted with a global allocator that
//! counts on this thread only while enabled.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use flash_core::{Editor, Event, KeyEvent, ModeId, Text, Timestamp, parse_key};

struct Counting;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    if ON.with(Cell::get) {
        COUNT.with(|c| c.set(c.get() + 1));
    }
}

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged; counting is a side effect only.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: forwarded from the caller, who upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: forwarded from the caller, who upholds `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocations made by `f` on this thread.
fn allocations(f: impl FnOnce()) -> u64 {
    COUNT.with(|c| c.set(0));
    ON.with(|on| on.set(true));
    f();
    ON.with(|on| on.set(false));
    COUNT.with(Cell::get)
}

fn key(text: &str) -> KeyEvent {
    parse_key(text).unwrap()
}

#[test]
fn key_resolution_and_normal_motions_allocate_nothing() {
    let mut ed = Editor::new(80, 24);
    let text: String = (0..200).map(|i| format!("word{i} other, text\n")).collect();
    let doc = ed.open(Text::from(text.as_str()), None);
    ed.open_view(doc).unwrap();

    // Resolution alone: single keys, a two-key sequence, a miss.
    let mut map = ed.keymap().clone();
    let t = |ms: u64| Timestamp::from_nanos(ms * 1_000_000);
    for k in ["l", "g", "g", "F5"] {
        map.feed(ModeId::NORMAL, key(k), t(0), 1_000_000_000);
    }
    let n = allocations(|| {
        for i in 0..1000 {
            map.feed(ModeId::NORMAL, key("l"), t(i), 1_000_000_000);
            map.feed(ModeId::NORMAL, key("g"), t(i), 1_000_000_000);
            map.feed(ModeId::NORMAL, key("e"), t(i), 1_000_000_000);
            map.feed(ModeId::NORMAL, key("F5"), t(i), 1_000_000_000);
            map.feed(ModeId::INSERT, key("x"), t(i), 1_000_000_000);
        }
    });
    assert_eq!(n, 0, "allocations in 5000 key resolutions");

    // Whole keys through the editor, after warming up every path once
    // (the hook queue and similar buffers keep their capacity).
    let mut now = 0;
    let mut press = |ed: &mut Editor, k: &str| {
        now += 10_000_000;
        ed.handle_at(&Event::Key(key(k)), Timestamp::from_nanos(now));
    };
    for k in ["l", "h", "j", "k", "w", "b", "g", "g", "g", "e", "g", "g"] {
        press(&mut ed, k);
    }
    for k in ["l", "h", "j", "k", "w", "b"] {
        let n = allocations(|| {
            for _ in 0..50 {
                press(&mut ed, k);
            }
        });
        assert_eq!(n, 0, "allocations handling {k:?} 50 times in normal mode");
    }
}
