//! Where the main loop's messages come from (PLAN 2.8; the Stage 7c
//! input-thread experiment, DECISIONS 045).
//!
//! - [`Threaded`] (the Stage 6 design): an input thread blocks in
//!   crossterm's `read` and sends each event down the channel the workers
//!   and the signal thread also send to; the main loop blocks on the
//!   channel. A key costs a thread wake-up and a channel hand-off before
//!   the main loop sees it.
//! - [`Polled`]: no input thread. The main loop blocks in one `poll(2)`
//!   over the terminal and a wake pipe; workers and the signal thread send
//!   down the channel and then write a byte to the pipe ([`Waker`]). When
//!   the terminal is readable, crossterm parses what is there on the main
//!   thread. Resizes reach crossterm through its own `SIGWINCH` pipe, which
//!   this `poll` does not watch, so the signal thread also forwards
//!   `SIGWINCH` as a wake-up.

use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crossterm::event as ct;
use rustix::event::{PollFd, PollFlags, poll};
use rustix::pipe::{PipeFlags, pipe_with};

/// A message source for the main loop. `M` is the loop's message type.
pub trait Source<M> {
    /// The next message, waiting for one. `None`: every sender is gone.
    fn wait(&mut self) -> io::Result<Option<M>>;
    /// The next message if one is ready.
    fn try_next(&mut self) -> io::Result<Option<M>>;
}

/// Messages from the channel only (an input thread sends the keys).
pub struct Threaded<M> {
    pub rx: Receiver<M>,
}

impl<M> Source<M> for Threaded<M> {
    fn wait(&mut self) -> io::Result<Option<M>> {
        Ok(self.rx.recv().ok())
    }

    fn try_next(&mut self) -> io::Result<Option<M>> {
        match self.rx.try_recv() {
            Ok(m) => Ok(Some(m)),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => Ok(None),
        }
    }
}

/// Wakes a [`Polled`] source: after sending a message, write a byte.
#[derive(Clone)]
pub struct Waker(Arc<OwnedFd>);

impl Waker {
    /// Never blocks: a full pipe already means "wake up".
    pub fn wake(&self) {
        let _ = rustix::io::write(&*self.0, &[1]);
    }
}

/// The terminal read on the main thread, the channel for the rest.
pub struct Polled<M> {
    rx: Receiver<M>,
    wake_read: OwnedFd,
    /// Builds the message for a terminal event read at an instant.
    input: fn(ct::Event, Instant) -> M,
    /// Builds the message for a read error.
    error: fn(io::Error) -> M,
}

impl<M> Polled<M> {
    /// A source over `rx`, and the [`Waker`] its senders use.
    pub fn new(
        rx: Receiver<M>,
        input: fn(ct::Event, Instant) -> M,
        error: fn(io::Error) -> M,
    ) -> io::Result<(Polled<M>, Waker)> {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
        Ok((
            Polled {
                rx,
                wake_read: read,
                input,
                error,
            },
            Waker(Arc::new(write)),
        ))
    }

    /// Empties the wake pipe (it only says "look").
    fn drain_wakes(&self) {
        let mut buf = [0u8; 64];
        while matches!(rustix::io::read(&self.wake_read, &mut buf), Ok(n) if n > 0) {}
    }
}

impl<M> Source<M> for Polled<M> {
    fn wait(&mut self) -> io::Result<Option<M>> {
        loop {
            if let Some(m) = self.try_next()? {
                return Ok(Some(m));
            }
            let stdin = io::stdin();
            let stdin = stdin.as_fd();
            let mut fds = [
                PollFd::new(&stdin, PollFlags::IN),
                PollFd::new(&self.wake_read, PollFlags::IN),
            ];
            match poll(&mut fds, None) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(e.into()),
            }
            if !fds[1].revents().is_empty() {
                self.drain_wakes();
            }
        }
    }

    fn try_next(&mut self) -> io::Result<Option<M>> {
        // Input first: crossterm may hold parsed events from an earlier
        // read, and a key is what latency is about.
        match ct::poll(Duration::ZERO) {
            Ok(true) => {
                return Ok(Some(match ct::read() {
                    Ok(event) => (self.input)(event, Instant::now()),
                    Err(e) => (self.error)(e),
                }));
            }
            Ok(false) => {}
            Err(e) => return Ok(Some((self.error)(e))),
        }
        match self.rx.try_recv() {
            Ok(m) => Ok(Some(m)),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn a_waker_wakes_and_never_blocks() {
        let (tx, rx) = mpsc::channel::<u8>();
        let (source, waker) = Polled::new(rx, |_, _| 0, |_| 0).unwrap();
        // Far more wakes than the pipe holds: none blocks.
        for _ in 0..100_000 {
            waker.wake();
        }
        let mut fds = [PollFd::new(&source.wake_read, PollFlags::IN)];
        assert_eq!(poll(&mut fds, Some(&rustix::event::Timespec::default())).unwrap(), 1);
        source.drain_wakes();
        let mut fds = [PollFd::new(&source.wake_read, PollFlags::IN)];
        assert_eq!(poll(&mut fds, Some(&rustix::event::Timespec::default())).unwrap(), 0);
        tx.send(7).unwrap();
        assert_eq!(source.rx.try_recv(), Ok(7));
    }
}
