//! File I/O off the main thread (PLAN 2.8, revision 3).
//!
//! Saving or opening a large file takes hundreds of milliseconds (282 ms
//! to save and 220 ms to load 100 MB on the Stage 6 machine); done inline
//! it stalls every key behind it. [`Io`] owns one worker thread that runs
//! saves and opens in order and hands each result to a `deliver` callback
//! (the main loop's channel). The main loop applies results with
//! [`Io::complete`], on its own thread, like any other event.
//!
//! A save writes a [`flash_core::SaveSnapshot`] taken when it starts, and
//! completion marks that snapshot's history node as saved, so typing
//! during the save stays modified. A second save of a document whose save
//! is still running waits for it: started at once, it would compare
//! against the disk state from before the first save and report a false
//! conflict.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flash_core::file::{self, LoadError, Loaded, SaveError, Saved};
use flash_core::{DocumentId, Editor, Effects, NodeId, Request, SaveSnapshot};

/// Work for the worker thread.
enum Job {
    Save {
        doc: DocumentId,
        path: PathBuf,
        snapshot: SaveSnapshot,
        force: bool,
    },
    Open {
        path: PathBuf,
    },
}

/// A finished job, to pass to [`Io::complete`] on the main thread.
#[derive(Debug)]
pub enum Done {
    Saved {
        doc: DocumentId,
        path: PathBuf,
        node: NodeId,
        result: Result<Saved, SaveError>,
    },
    Opened {
        path: PathBuf,
        result: Result<Loaded, LoadError>,
    },
}

/// The worker and the bookkeeping of what it is doing.
pub struct Io {
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
    /// Closed by the worker when it exits (for a bounded wait).
    exited: Receiver<()>,
    /// Documents with a save submitted and not yet completed.
    saving: Vec<DocumentId>,
    /// Saves waiting for an earlier save of the same document.
    deferred: VecDeque<Request>,
    /// Jobs submitted and not yet completed.
    in_flight: usize,
}

impl Io {
    /// Starts the worker. `deliver` gets each result; returning `false`
    /// (the receiver is gone) stops the worker.
    pub fn spawn(mut deliver: impl FnMut(Done) -> bool + Send + 'static) -> std::io::Result<Io> {
        let (jobs, rx) = mpsc::channel::<Job>();
        let (exit_tx, exited) = mpsc::channel::<()>();
        let worker = thread::Builder::new()
            .name("flash-io".to_owned())
            .spawn(move || {
                let _exit = exit_tx;
                for job in rx {
                    if !deliver(run(job)) {
                        break;
                    }
                }
            })?;
        Ok(Io {
            jobs: Some(jobs),
            worker: Some(worker),
            exited,
            saving: Vec::new(),
            deferred: VecDeque::new(),
            in_flight: 0,
        })
    }

    /// Takes a [`Request::Save`] or [`Request::Open`] and starts it on the
    /// worker; gives any other request back. A save of a document already
    /// being saved waits for that save to complete.
    pub fn submit(&mut self, editor: &mut Editor, request: Request) -> Result<Effects, Request> {
        match request {
            Request::Save { doc, path, force } => {
                if self.saving.contains(&doc) {
                    self.deferred.push_back(Request::Save { doc, path, force });
                    return Ok(Effects::default());
                }
                let Some(snapshot) = editor.save_snapshot(doc, &path) else {
                    return Ok(Effects::default());
                };
                self.saving.push(doc);
                let fx = editor.notify(format_args!("saving {}…", path.display()));
                Ok(fx.or(self.send(
                    editor,
                    Job::Save {
                        doc,
                        path,
                        snapshot,
                        force,
                    },
                )))
            }
            Request::Open { path } => Ok(self.send(editor, Job::Open { path })),
            other => Err(other),
        }
    }

    /// Sends `job` to the worker, or runs it here if the worker is gone
    /// (correct, only slower).
    fn send(&mut self, editor: &mut Editor, job: Job) -> Effects {
        self.in_flight += 1;
        let job = match &self.jobs {
            Some(jobs) => match jobs.send(job) {
                Ok(()) => return Effects::default(),
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        self.jobs = None;
        self.complete(editor, run(job))
    }

    /// Applies a finished job to the editor, then starts the next deferred
    /// save of the same document, if any.
    pub fn complete(&mut self, editor: &mut Editor, done: Done) -> Effects {
        self.in_flight = self.in_flight.saturating_sub(1);
        match done {
            Done::Opened { path, result } => editor.opened(path, result),
            Done::Saved {
                doc,
                path,
                node,
                result,
            } => {
                self.saving.retain(|&d| d != doc);
                let mut fx = editor.saved_node(doc, path, node, result);
                if let Some(i) = self
                    .deferred
                    .iter()
                    .position(|r| matches!(r, Request::Save { doc: d, .. } if *d == doc))
                {
                    if let Some(next) = self.deferred.remove(i) {
                        if let Ok(more) = self.submit(editor, next) {
                            fx = fx.or(more);
                        }
                    }
                }
                fx
            }
        }
    }

    /// Whether any job is submitted and not yet completed.
    pub fn busy(&self) -> bool {
        self.in_flight > 0 || !self.deferred.is_empty()
    }

    /// Stops taking jobs and waits up to `timeout` for the worker to finish
    /// the ones it has (a save in progress must not be cut off by exit).
    /// Returns whether it finished in time.
    pub fn finish(mut self, timeout: Duration) -> bool {
        self.jobs = None;
        match self.exited.recv_timeout(timeout) {
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                true
            }
            Ok(()) | Err(RecvTimeoutError::Timeout) => false,
        }
    }
}

fn run(job: Job) -> Done {
    match job {
        Job::Save {
            doc,
            path,
            snapshot,
            force,
        } => {
            let result = file::save(
                &path,
                &snapshot.text,
                snapshot.bom,
                snapshot.expected.as_ref(),
                force,
            );
            Done::Saved {
                doc,
                path,
                node: snapshot.node,
                result,
            }
        }
        Job::Open { path } => {
            let result = file::load(&path);
            Done::Opened { path, result }
        }
    }
}

#[cfg(test)]
#[path = "io_tests.rs"]
mod tests;
