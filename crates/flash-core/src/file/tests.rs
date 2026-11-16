//! Loading and saving in temp directories (PLAN Stage 6 tests).

use std::fs;
use std::io::{self, Cursor};
use std::path::Path;

use tempfile::TempDir;

use super::save::save_with;
use super::*;

fn dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn round_trip(bytes: &[u8]) {
    let d = dir();
    let p = d.path().join("f");
    fs::write(&p, bytes).unwrap();
    let loaded = load(&p).unwrap();
    assert_eq!(loaded.disk.size, bytes.len() as u64);
    let out = d.path().join("g");
    let saved = save(&out, &loaded.text, loaded.bom, None, false).unwrap();
    assert_eq!(fs::read(&out).unwrap(), bytes, "{bytes:?}");
    assert_eq!(saved.disk.hash, loaded.disk.hash);
    assert_eq!(saved.disk.size, loaded.disk.size);
}

#[test]
fn round_trips_are_byte_identical() {
    for bytes in [
        &b""[..],
        b"a",
        b"one\ntwo\n",
        b"one\r\ntwo\r\n",
        b"mixed\r\nends\nhere\r",
        b"no final newline",
        b"\xEF\xBB\xBFwith bom\r\n",
        b"\xEF\xBB\xBF",
        "h\u{e9}llo \u{1F468}\u{200D}\u{1F469} \u{8001}\u{864E}\n".as_bytes(),
    ] {
        round_trip(bytes);
    }
    // Large enough to span many reads and rope leaves.
    let big: Vec<u8> = "line \u{e9}\u{1F600}\r\n".repeat(20_000).into_bytes();
    round_trip(&big);
}

#[test]
fn load_strips_the_bom_and_detects_line_endings() {
    let loaded = load_from(Cursor::new(b"\xEF\xBB\xBFa\r\nb\r\nc\n"), 2).unwrap();
    assert!(loaded.bom);
    assert_eq!(loaded.text.to_string(), "a\r\nb\r\nc\n");
    assert_eq!(loaded.line_ending, LineEnding::Crlf);
    assert_eq!(loaded.disk.size, 11);
    let loaded = load_from(Cursor::new(b"a\nb\r\n"), 1).unwrap();
    assert!(!loaded.bom);
    assert_eq!(loaded.line_ending, LineEnding::Lf, "ties go to LF");
    // A BOM-like prefix that is not a BOM is content.
    let loaded = load_from(Cursor::new(&BOM[..2]), 1);
    assert_eq!(loaded.unwrap_err(), LoadError::InvalidUtf8 { offset: 0 });
    let loaded = load_from(Cursor::new("\u{feff}x".as_bytes()), 1).unwrap();
    assert!(loaded.bom);
    assert_eq!(loaded.text.to_string(), "x");
}

#[test]
fn every_chunk_size_loads_the_same() {
    let s = "\u{EF}\u{BB}\u{BF}?\u{1F468}\u{200D}\u{1F469}\r\n\u{8001}a\r\n\u{e9}\n";
    let bytes = [BOM, s.as_bytes()].concat();
    let reference = load_from(Cursor::new(&bytes), 1 << 20).unwrap();
    for chunk in 1..=12 {
        let l = load_from(Cursor::new(&bytes), chunk).unwrap();
        assert_eq!(l.text, reference.text, "chunk {chunk}");
        assert_eq!(l.bom, reference.bom);
        assert_eq!(l.line_ending, reference.line_ending);
        assert_eq!(l.disk, reference.disk);
    }
    assert_eq!(reference.disk.hash, {
        let mut h = Fnv::new();
        h.update(&bytes);
        h.finish()
    });
}

#[test]
fn invalid_utf8_is_refused_with_its_offset() {
    // A bad byte, a truncated sequence at the end, an overlong encoding,
    // and a lone continuation byte, each split across every read size.
    let cases: [(&[u8], u64); 5] = [
        (b"ab\xFFcd", 2),
        (b"abc\xE2\x82", 3),
        (b"x\xC0\xAFy", 1),
        (b"\x80", 0),
        (b"\xEF\xBB\xBFok\xFE", 5),
    ];
    for (bytes, offset) in cases {
        for chunk in 1..=6 {
            assert_eq!(
                load_from(Cursor::new(bytes), chunk).unwrap_err(),
                LoadError::InvalidUtf8 { offset },
                "{bytes:?} chunk {chunk}"
            );
        }
    }
    let d = dir();
    let p = d.path().join("bin");
    fs::write(&p, b"\x00\x01\xFF").unwrap();
    let e = load(&p).unwrap_err();
    assert_eq!(e, LoadError::InvalidUtf8 { offset: 2 });
    assert!(e.to_string().contains("refusing"));
}

#[test]
fn only_regular_files_load() {
    let d = dir();
    assert_eq!(load(d.path()).unwrap_err(), LoadError::NotAFile);
    assert_eq!(
        load(&d.path().join("missing")).unwrap_err(),
        LoadError::NotFound
    );
    #[cfg(unix)]
    {
        assert_eq!(
            load(Path::new("/dev/null")).unwrap_err(),
            LoadError::NotAFile
        );
    }
}

#[test]
fn load_records_mtime() {
    let d = dir();
    let p = d.path().join("f");
    fs::write(&p, "x").unwrap();
    let loaded = load(&p).unwrap();
    assert_eq!(
        loaded.disk.modified,
        Some(fs::metadata(&p).unwrap().modified().unwrap())
    );
}

// ---- saving ----

fn text(s: &str) -> Text {
    Text::from(s)
}

/// Writes `s` to `d/name` and loads it.
fn existing(d: &TempDir, name: &str, s: &str) -> (std::path::PathBuf, Loaded) {
    let p = d.path().join(name);
    fs::write(&p, s).unwrap();
    let loaded = load(&p).unwrap();
    (p, loaded)
}

/// The directory's entries other than `keep` (no temp or backup left).
fn leftovers(d: &TempDir, keep: &[&str]) -> Vec<String> {
    fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !keep.contains(&n.as_str()))
        .collect()
}

#[test]
fn save_creates_a_new_file() {
    let d = dir();
    let p = d.path().join("new.txt");
    let saved = save(&p, &text("hello\n"), false, None, false).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "hello\n");
    assert_eq!(saved.method, SaveMethod::Atomic);
    assert_eq!(saved.target, fs::canonicalize(&p).unwrap());
    assert_eq!(saved.disk.size, 6);
    assert!(saved.dir_synced);
    // The saved state is what a fresh load sees.
    let again = load(&p).unwrap();
    assert_eq!(again.disk, saved.disk);
    assert!(leftovers(&d, &["new.txt"]).is_empty());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_ne!(
            mode & 0o600,
            0,
            "readable and writable by the owner: {mode:o}"
        );
    }
}

#[test]
fn save_replaces_an_existing_file_keeping_its_permissions() {
    let d = dir();
    let (p, loaded) = existing(&d, "f.txt", "old\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
    }
    // Re-load to record the mtime after chmod (chmod changes ctime only,
    // but be exact).
    let loaded = load(&p).unwrap_or(loaded);
    let saved = save(&p, &text("new\n"), false, Some(&loaded.disk), false).unwrap();
    assert_eq!(saved.method, SaveMethod::Atomic);
    assert_eq!(fs::read_to_string(&p).unwrap(), "new\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    assert!(leftovers(&d, &["f.txt"]).is_empty());
    // And saving again from the new state works without force.
    save(&p, &text("newer\n"), false, Some(&saved.disk), false).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "newer\n");
}

#[cfg(unix)]
#[test]
fn save_through_a_symlink_writes_the_target_and_keeps_the_link() {
    let d = dir();
    let (real, loaded) = existing(&d, "real.txt", "a\n");
    let link = d.path().join("link.txt");
    std::os::unix::fs::symlink("real.txt", &link).unwrap();
    let saved = save(&link, &text("b\n"), false, Some(&loaded.disk), false).unwrap();
    assert_eq!(saved.target, fs::canonicalize(&real).unwrap());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&real).unwrap(), "b\n");
    assert_eq!(fs::read_to_string(&link).unwrap(), "b\n");
    // A dangling link: the file it points to is created, the link kept.
    let dangling = d.path().join("dangling");
    std::os::unix::fs::symlink("sub/../made.txt", &dangling).unwrap();
    fs::create_dir(d.path().join("sub")).unwrap();
    save(&dangling, &text("c"), false, None, false).unwrap();
    assert!(
        fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(d.path().join("made.txt")).unwrap(), "c");
}

#[cfg(unix)]
#[test]
fn save_with_hard_links_writes_in_place() {
    use std::os::unix::fs::MetadataExt;
    let d = dir();
    let (a, loaded) = existing(&d, "a.txt", "shared\n");
    let b = d.path().join("b.txt");
    fs::hard_link(&a, &b).unwrap();
    let inode = fs::metadata(&a).unwrap().ino();
    let loaded = load(&a).unwrap_or(loaded);
    let saved = save(
        &a,
        &text("both see this\n"),
        false,
        Some(&loaded.disk),
        false,
    )
    .unwrap();
    assert_eq!(saved.method, SaveMethod::InPlaceHardLinks(2));
    assert_eq!(fs::read_to_string(&b).unwrap(), "both see this\n");
    assert_eq!(fs::metadata(&a).unwrap().ino(), inode);
    assert_eq!(fs::metadata(&a).unwrap().nlink(), 2);
    assert!(
        leftovers(&d, &["a.txt", "b.txt"]).is_empty(),
        "backup removed"
    );
}

#[cfg(unix)]
#[test]
fn save_into_a_read_only_directory_fails_and_changes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir();
    let (p, loaded) = existing(&d, "f.txt", "original\n");
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = save(&p, &text("new\n"), false, Some(&loaded.disk), false);
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o755)).unwrap();
    match result {
        Err(SaveError::Io { phase, kind, .. }) => {
            assert_eq!(phase, Phase::CreateTemp);
            assert_eq!(kind, io::ErrorKind::PermissionDenied);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(&p).unwrap(), "original\n");
    assert!(leftovers(&d, &["f.txt"]).is_empty());
}

#[test]
fn changed_on_disk_is_a_conflict_until_forced() {
    let d = dir();
    let (p, loaded) = existing(&d, "f.txt", "mine\n");
    // Someone else changes it: different size.
    fs::write(&p, "theirs, longer\n").unwrap();
    assert_eq!(
        save(&p, &text("x"), false, Some(&loaded.disk), false),
        Err(SaveError::Conflict)
    );
    assert_eq!(fs::read_to_string(&p).unwrap(), "theirs, longer\n");
    // Same size and mtime, different bytes: the hash catches it.
    fs::write(&p, "MINE\n").unwrap();
    let f = fs::File::options().write(true).open(&p).unwrap();
    f.set_modified(loaded.disk.modified.unwrap()).unwrap();
    drop(f);
    assert_eq!(fs::metadata(&p).unwrap().len(), loaded.disk.size);
    assert_eq!(
        save(&p, &text("x"), false, Some(&loaded.disk), false),
        Err(SaveError::Conflict)
    );
    // Deleted since loading.
    fs::remove_file(&p).unwrap();
    assert_eq!(
        save(&p, &text("x"), false, Some(&loaded.disk), false),
        Err(SaveError::Conflict)
    );
    // Created since a new document was opened.
    fs::write(&p, "appeared").unwrap();
    assert_eq!(
        save(&p, &text("x"), false, None, false),
        Err(SaveError::Conflict)
    );
    // Force overwrites.
    save(&p, &text("forced"), false, None, true).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "forced");
    assert!(leftovers(&d, &["f.txt"]).is_empty());
}

#[test]
fn saving_over_a_directory_is_refused() {
    let d = dir();
    fs::create_dir(d.path().join("sub")).unwrap();
    assert_eq!(
        save(&d.path().join("sub"), &text("x"), false, None, true),
        Err(SaveError::NotAFile)
    );
}

#[test]
fn every_failure_before_the_rename_leaves_the_original() {
    for phase in [
        Phase::Resolve,
        Phase::Check,
        Phase::CreateTemp,
        Phase::Write,
        Phase::Sync,
        Phase::Permissions,
        Phase::Rename,
    ] {
        let d = dir();
        let (p, loaded) = existing(&d, "f.txt", "original\n");
        let mut hook = |at: Phase| {
            if at == phase {
                Err(io::Error::other("injected"))
            } else {
                Ok(())
            }
        };
        let result = save_with(
            &p,
            &text("new\n"),
            false,
            Some(&loaded.disk),
            false,
            &mut hook,
        );
        match result {
            Err(SaveError::Io { phase: p2, .. }) => assert_eq!(p2, phase),
            other => panic!("{phase:?}: {other:?}"),
        }
        assert_eq!(fs::read_to_string(&p).unwrap(), "original\n", "{phase:?}");
        assert!(
            leftovers(&d, &["f.txt"]).is_empty(),
            "{phase:?}: temp removed"
        );
    }
}

#[test]
fn a_failed_directory_sync_still_saves() {
    let d = dir();
    let (p, loaded) = existing(&d, "f.txt", "original\n");
    let mut hook = |at: Phase| {
        if at == Phase::SyncDir {
            Err(io::Error::other("injected"))
        } else {
            Ok(())
        }
    };
    let saved = save_with(
        &p,
        &text("new\n"),
        false,
        Some(&loaded.disk),
        false,
        &mut hook,
    )
    .unwrap();
    assert!(!saved.dir_synced);
    assert_eq!(fs::read_to_string(&p).unwrap(), "new\n");
}

#[test]
fn ownership_that_cannot_be_kept_falls_back_to_in_place() {
    let d = dir();
    let (p, loaded) = existing(&d, "f.txt", "original\n");
    let mut hook = |at: Phase| {
        if at == Phase::Ownership {
            Err(io::Error::other(
                "pretend the owner differs and fchown fails",
            ))
        } else {
            Ok(())
        }
    };
    let saved = save_with(
        &p,
        &text("new\n"),
        true,
        Some(&loaded.disk),
        false,
        &mut hook,
    )
    .unwrap();
    assert_eq!(saved.method, SaveMethod::InPlaceOwnership);
    assert_eq!(fs::read(&p).unwrap(), b"\xEF\xBB\xBFnew\n");
    assert!(leftovers(&d, &["f.txt"]).is_empty());
}

#[cfg(unix)]
#[test]
fn in_place_failures_keep_the_original_or_a_backup() {
    let d = dir();
    let (a, _) = existing(&d, "a.txt", "original\n");
    fs::hard_link(&a, d.path().join("b.txt")).unwrap();
    let loaded = load(&a).unwrap();
    // The backup cannot be made: nothing written, nothing left.
    let mut fail_backup = |at: Phase| {
        if at == Phase::Backup {
            Err(io::Error::other("injected"))
        } else {
            Ok(())
        }
    };
    let r = save_with(
        &a,
        &text("new\n"),
        false,
        Some(&loaded.disk),
        false,
        &mut fail_backup,
    );
    assert!(
        matches!(
            r,
            Err(SaveError::Io {
                phase: Phase::Backup,
                ..
            })
        ),
        "{r:?}"
    );
    assert_eq!(fs::read_to_string(&a).unwrap(), "original\n");
    assert!(leftovers(&d, &["a.txt", "b.txt"]).is_empty());
    // The in-place write fails: the backup holds the original.
    let mut fail_write = |at: Phase| {
        if at == Phase::WriteInPlace {
            Err(io::Error::other("injected"))
        } else {
            Ok(())
        }
    };
    let r = save_with(
        &a,
        &text("new\n"),
        false,
        Some(&loaded.disk),
        false,
        &mut fail_write,
    );
    let Err(SaveError::InPlace { backup, .. }) = r else {
        panic!("{r:?}");
    };
    assert_eq!(fs::read_to_string(&backup).unwrap(), "original\n");
    assert!(
        SaveError::InPlace {
            backup: backup.clone(),
            kind: io::ErrorKind::Other,
            message: "m".into()
        }
        .to_string()
        .contains(&backup.display().to_string())
    );
}

#[test]
fn save_writes_the_bom_back() {
    let d = dir();
    let p = d.path().join("bom.txt");
    save(&p, &text("x\r\n"), true, None, false).unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"\xEF\xBB\xBFx\r\n");
}

#[test]
fn relative_paths_resolve_against_the_current_directory() {
    // `resolve` turns a bare name into the canonical current directory
    // plus the name (the binary passes paths as given).
    let p = Path::new("definitely-not-here-flash-test.txt");
    let resolved = super::save::resolve(p).unwrap();
    assert_eq!(
        resolved,
        std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join(p)
    );
}
