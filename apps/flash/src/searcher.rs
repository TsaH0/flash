//! Searches of large documents off the main thread (PLAN Stage 7b).
//!
//! One worker thread and a one-job slot: a new search replaces one that
//! has not started yet (latest wins), so typing a pattern quickly does
//! not queue a search per key. A search that has started runs to the end
//! (regex-cursor has no cancellation); its result is ignored by the
//! editor if a newer search was asked for meanwhile
//! (`Editor::search_done` checks the id).

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use flash_core::{SearchDone, SearchJob};

#[derive(Default)]
struct Slot {
    job: Option<SearchJob>,
    closed: bool,
}

type Shared = Arc<(Mutex<Slot>, Condvar)>;

/// The search worker. Dropping it stops the worker after its current
/// search (it is not waited for).
pub struct Searcher {
    shared: Shared,
}

impl Searcher {
    /// Starts the worker; `deliver` gets each result and returning `false`
    /// (the receiver is gone) stops it.
    pub fn spawn(
        mut deliver: impl FnMut(SearchDone) -> bool + Send + 'static,
    ) -> std::io::Result<Searcher> {
        let shared: Shared = Arc::default();
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("flash-search".to_owned())
            .spawn(move || {
                let (lock, ready) = &*worker;
                loop {
                    let job = {
                        let mut slot = lock.lock().unwrap_or_else(PoisonError::into_inner);
                        loop {
                            if slot.closed {
                                return;
                            }
                            if let Some(job) = slot.job.take() {
                                break job;
                            }
                            slot = ready.wait(slot).unwrap_or_else(PoisonError::into_inner);
                        }
                    };
                    if !deliver(job.run()) {
                        return;
                    }
                }
            })?;
        Ok(Searcher { shared })
    }

    /// Asks for `job`, replacing a search that has not started.
    pub fn submit(&self, job: SearchJob) {
        let (lock, ready) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).job = Some(job);
        ready.notify_one();
    }
}

impl Drop for Searcher {
    fn drop(&mut self) {
        let (lock, ready) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use flash_core::{Editor, Event, KeyCode, KeyEvent, Request, SEARCH_INLINE_MAX, Text};

    use super::*;

    #[test]
    fn the_latest_search_is_run_and_reported() {
        let big = format!("{}needle", "x".repeat(SEARCH_INLINE_MAX));
        let mut ed = Editor::new(80, 24);
        let doc = ed.open(Text::from(big.as_str()), None);
        let view = ed.open_view(doc).unwrap();
        for c in "/needle".chars() {
            ed.handle(&Event::Key(KeyEvent::plain(KeyCode::Char(c))));
        }
        let (tx, rx) = mpsc::channel();
        let searcher = Searcher::spawn(move |done| tx.send(done).is_ok()).unwrap();
        for request in ed.take_requests() {
            if let Request::Search(job) = request {
                searcher.submit(job);
            }
        }
        // Earlier jobs may or may not have run; the last one always does.
        let mut last = None;
        while let Ok(done) = rx.recv_timeout(Duration::from_secs(10)) {
            let id = done.id;
            ed.search_done(done);
            last = Some(id);
            if ed.selection(view).unwrap().primary().start() == SEARCH_INLINE_MAX {
                break;
            }
        }
        assert!(last.is_some(), "a result arrived");
        let r = *ed.selection(view).unwrap().primary();
        assert_eq!(
            (r.start(), r.end()),
            (SEARCH_INLINE_MAX, SEARCH_INLINE_MAX + 6)
        );
        drop(searcher);
    }
}
