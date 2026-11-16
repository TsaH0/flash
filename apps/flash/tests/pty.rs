//! Smoke tests in a real pseudo-terminal: `flash` runs in a new session
//! (`setsid`) with a pty as its terminal, and the tests type into the
//! master side. They check that files are saved and that the terminal is
//! restored on every exit path: quit, panic, a terminating signal, and
//! around a suspend.

#![cfg(target_os = "linux")]

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, Winsize, tcgetattr, tcsetwinsize};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Session {
    child: Child,
    master: File,
    slave: File,
    output: Arc<Mutex<Vec<u8>>>,
    original: LocalModes,
}

impl Session {
    fn start(args: &[&Path], env: &[(&str, &str)]) -> Session {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().unwrap())
            .unwrap();
        tcsetwinsize(
            &slave,
            Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        let original = tcgetattr(&slave).unwrap().local_modes;
        assert!(original.contains(LocalModes::ICANON | LocalModes::ECHO));
        let mut cmd = Command::new("setsid");
        cmd.arg(env!("CARGO_BIN_EXE_flash"))
            .args(args)
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(Stdio::piped())
            .env_remove("FLASH_TRACE")
            .env_remove("FLASH_LATENCY_LOG");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().unwrap();
        let master = File::from(master);
        let output = Arc::new(Mutex::new(Vec::new()));
        {
            let mut reader = master.try_clone().unwrap();
            let output = Arc::clone(&output);
            thread::spawn(move || {
                let mut buf = [0; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    output.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            });
        }
        Session {
            child,
            master,
            slave,
            output,
            original,
        }
    }

    fn modes(&self) -> LocalModes {
        tcgetattr(&self.slave).unwrap().local_modes
    }

    fn raw(&self) -> bool {
        !self.modes().contains(LocalModes::ICANON)
    }

    fn wait_until(&mut self, what: &str, mut ok: impl FnMut(&mut Session) -> bool) {
        let start = Instant::now();
        while !ok(self) {
            if start.elapsed() > TIMEOUT {
                let _ = self.child.kill();
                panic!("timed out waiting for {what}; output: {:?}", self.text());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits until the editor has put the terminal in raw mode and drawn.
    fn wait_ready(&mut self) {
        self.wait_until("raw mode", |s| {
            s.raw() && s.text().contains("\u{1b}[?1049h")
        });
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if start.elapsed() > TIMEOUT {
                let _ = self.child.kill();
                panic!("flash did not exit; output: {:?}", self.text());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stderr(&mut self) -> String {
        let mut s = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        s
    }

    /// After exit: the terminal modes are what they were before, and the
    /// alternate screen, bracketed paste, and hidden cursor were undone
    /// after they were set.
    fn assert_restored(&self) {
        assert_eq!(self.modes(), self.original, "terminal modes restored");
        let out = self.text();
        let entered = out
            .find("\u{1b}[?1049h")
            .expect("entered the alternate screen");
        let left = out
            .rfind("\u{1b}[?1049l")
            .expect("left the alternate screen");
        assert!(left > entered);
        assert!(
            out.rfind("\u{1b}[?2004l")
                .expect("disabled bracketed paste")
                > entered
        );
        assert!(out.rfind("\u{1b}[?25h").expect("showed the cursor") > entered);
    }
}

fn kill(signal: &str, pid: u32) {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .unwrap();
    assert!(status.success());
}

/// The process state letter from `/proc/<pid>/stat` ('T' when stopped).
fn state(pid: u32) -> char {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    stat.rsplit(')')
        .next()
        .and_then(|rest| rest.trim_start().chars().next())
        .unwrap_or('?')
}

#[test]
fn type_save_and_quit() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("doc.txt");
    fs::write(&doc, "abc\n").unwrap();
    let mut s = Session::start(&[&doc], &[]);
    s.wait_ready();
    // 'i' (insert mode), 'x', Ctrl+S, Ctrl+Q (raw mode: XON/XOFF are
    // plain keys).
    s.send(b"ix\x13\x11");
    let status = s.wait_exit();
    assert!(status.success(), "{status:?}: {}", s.stderr());
    assert_eq!(fs::read_to_string(&doc).unwrap(), "xabc\n");
    s.assert_restored();
}

#[test]
fn quitting_with_unsaved_changes_asks() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("doc.txt");
    fs::write(&doc, "abc\n").unwrap();
    let mut s = Session::start(&[&doc], &[]);
    s.wait_ready();
    s.send(b"ix\x11");
    s.wait_until("the quit question", |s| s.text().contains("quit anyway"));
    assert!(s.child.try_wait().unwrap().is_none(), "still running");
    s.send(b"y");
    assert!(s.wait_exit().success());
    assert_eq!(fs::read_to_string(&doc).unwrap(), "abc\n", "not saved");
    s.assert_restored();
}

#[test]
fn a_panic_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("doc.txt");
    fs::write(&doc, "abc\n").unwrap();
    let mut s = Session::start(&[&doc], &[("FLASH_DEBUG_PANIC", "1")]);
    let status = s.wait_exit();
    assert_eq!(status.code(), Some(101));
    assert!(s.stderr().contains("FLASH_DEBUG_PANIC"));
    s.assert_restored();
}

#[test]
fn sigterm_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("doc.txt");
    fs::write(&doc, "abc\n").unwrap();
    let mut s = Session::start(&[&doc], &[]);
    s.wait_ready();
    kill("TERM", s.child.id());
    let status = s.wait_exit();
    assert_eq!(status.code(), Some(128 + 15));
    s.assert_restored();
    assert_eq!(fs::read_to_string(&doc).unwrap(), "abc\n");
}

#[test]
fn ctrl_z_suspends_and_continuing_resumes() {
    let mut s = Session::start(&[], &[]);
    s.wait_ready();
    let pid = s.child.id();
    s.send(b"\x1a");
    // Stopped, with the terminal handed back in cooked mode.
    s.wait_until("stop", |s| state(pid) == 'T' && !s.raw());
    assert_eq!(s.modes(), s.original);
    let left = s.text().rfind("\u{1b}[?1049l");
    assert!(left.is_some(), "left the alternate screen before stopping");
    kill("CONT", pid);
    s.wait_until("resume", |s| state(pid) != 'T' && s.raw());
    // Back on the alternate screen, redrawn, and still working.
    s.wait_until("redraw", |s| s.text().rfind("\u{1b}[?1049h") > left);
    s.send(b"\x11");
    assert!(s.wait_exit().success());
    s.assert_restored();
}

/// `FLASH_TRACE` records a real terminal session, ending with the key that
/// quit, and `--replay` replays it without touching the file.
#[test]
fn a_recorded_session_replays() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("doc.txt");
    let trace = dir.path().join("session.jsonl");
    fs::write(&doc, "abc\n").unwrap();
    let mut s = Session::start(&[&doc], &[("FLASH_TRACE", trace.to_str().unwrap())]);
    s.wait_ready();
    s.send(b"ixy\x1b[C");
    s.send(b"\x11");
    s.wait_until("the quit question", |s| s.text().contains("quit anyway"));
    s.send(b"y");
    assert!(s.wait_exit().success());
    let recorded = fs::read_to_string(&trace).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        lines.len(),
        1 + 6,
        "header + i, x, y, Right, Ctrl+Q, y: {recorded}"
    );
    assert!(lines[0].contains(r#""flash_trace":1"#));
    assert!(lines[6].contains(r#""code":{"char":"y"}"#), "{}", lines[6]);
    let out = Command::new(env!("CARGO_BIN_EXE_flash"))
        .arg("--replay")
        .arg(&trace)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains(r#""events":6"#));
    assert_eq!(fs::read_to_string(&doc).unwrap(), "abc\n");
}

/// Not a check: measures the whole interactive path (input thread,
/// channel, command, frame, write to a pty) for 500 keys typed 5 ms apart
/// into a 1 MB file, and appends the latency log line to
/// `$FLASH_PTY_LATENCY_OUT`. Run with `--ignored`.
#[test]
#[ignore]
fn interactive_latency() {
    let out = std::env::var("FLASH_PTY_LATENCY_OUT").expect("set FLASH_PTY_LATENCY_OUT");
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("medium.txt");
    let content: String = (0..25_000)
        .map(|i| format!("    let value_{i} = compute({i}, \"text\"); // note\n"))
        .collect();
    fs::write(&doc, &content).unwrap();
    let mut s = Session::start(&[&doc], &[("FLASH_LATENCY_LOG", &out)]);
    s.wait_ready();
    // Typing is measured in insert mode.
    s.send(b"i");
    let keys: &[&[u8]] = &[
        b"x", b"y", b"\x7f", b"\x1b[B", b"\x1b[C", b"z", b"\r", b"\x1b[A",
    ];
    for i in 0..500 {
        s.send(keys[i % keys.len()]);
        thread::sleep(Duration::from_millis(5));
    }
    // Quit without saving: Ctrl+Q, then confirm.
    s.send(b"\x11");
    s.wait_until("the quit question", |s| s.text().contains("quit anyway"));
    s.send(b"y");
    assert!(s.wait_exit().success());
    s.assert_restored();
}
