//! Saving: atomic replace, or in place behind a backup.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{BOM, DiskState, Fnv, hash_file};
use crate::text::Text;

/// The steps of a save, for error reports (and the test-only failure hook,
/// which is consulted before each one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// Resolving symlinks and reading the target's metadata.
    Resolve,
    /// Comparing the target with what was loaded.
    Check,
    /// Creating the temp file next to the target.
    CreateTemp,
    /// Keeping the target's owner and group on the temp file.
    Ownership,
    /// Writing the content.
    Write,
    /// Flushing the content to disk.
    Sync,
    /// Copying the target's permissions.
    Permissions,
    /// Renaming the temp file over the target.
    Rename,
    /// Flushing the directory entry.
    SyncDir,
    /// Copying the target to a backup before writing in place.
    Backup,
    /// Writing the target in place.
    WriteInPlace,
}

/// How a save wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMethod {
    /// Temp file renamed over the target.
    Atomic,
    /// Written in place (behind a backup) because the target has this many
    /// hard links; replacing it would split them.
    InPlaceHardLinks(u64),
    /// Written in place (behind a backup) because a new file could not get
    /// the target's owner and group.
    InPlaceOwnership,
}

/// A successful save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// The file written: the path with symlinks resolved.
    pub target: PathBuf,
    pub method: SaveMethod,
    /// The file as written, for the next conflict check.
    pub disk: DiskState,
    /// Whether the directory was flushed too (some file systems refuse;
    /// the content itself is flushed either way).
    pub dir_synced: bool,
}

/// Why a save did not happen. In every case the original file is intact.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SaveError {
    /// The file on disk is not the one loaded (changed, created, or
    /// deleted since); nothing was written. Save again with `force`.
    Conflict,
    /// The target exists and is not a regular file.
    NotAFile,
    /// A step failed; nothing was replaced and no temp file is left.
    Io {
        phase: Phase,
        kind: io::ErrorKind,
        message: String,
    },
    /// Writing in place failed after the backup was made. The original
    /// content is in `backup`; the target may be partly written.
    InPlace {
        backup: PathBuf,
        kind: io::ErrorKind,
        message: String,
    },
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => write!(f, "the file changed on disk since it was opened"),
            Self::NotAFile => write!(f, "not a regular file"),
            Self::Io { phase, message, .. } => write!(f, "save failed ({phase:?}): {message}"),
            Self::InPlace {
                backup, message, ..
            } => write!(
                f,
                "in-place save failed: {message}; the original is in {}",
                backup.display()
            ),
        }
    }
}

impl std::error::Error for SaveError {}

fn io_error(phase: Phase) -> impl Fn(io::Error) -> SaveError {
    move |e| SaveError::Io {
        phase,
        kind: e.kind(),
        message: e.to_string(),
    }
}

/// Saves `text` (with a BOM first if `bom`) to `path`.
///
/// Unless `force`, first checks that the file on disk is the one described
/// by `expected` (size, mtime, then content hash), or, with no `expected`,
/// that it does not exist; otherwise [`SaveError::Conflict`] and nothing is
/// written. Symlinks are followed: the resolved target is written and the
/// link kept. O(n) (plus O(n) to hash the target when its size and mtime
/// match).
pub fn save(
    path: &Path,
    text: &Text,
    bom: bool,
    expected: Option<&DiskState>,
    force: bool,
) -> Result<Saved, SaveError> {
    save_with(path, text, bom, expected, force, &mut |_| Ok(()))
}

/// [`save`] with `hook` called before each [`Phase`]; an error from it
/// fails that phase (tests inject failures this way). An error at
/// [`Phase::Ownership`] means "ownership cannot be kept".
pub(crate) fn save_with(
    path: &Path,
    text: &Text,
    bom: bool,
    expected: Option<&DiskState>,
    force: bool,
    hook: &mut dyn FnMut(Phase) -> io::Result<()>,
) -> Result<Saved, SaveError> {
    hook(Phase::Resolve).map_err(io_error(Phase::Resolve))?;
    let target = resolve(path).map_err(io_error(Phase::Resolve))?;
    let existing = match fs::metadata(&target) {
        Ok(m) if !m.is_file() => return Err(SaveError::NotAFile),
        Ok(m) => Some(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_error(Phase::Resolve)(e)),
    };
    if !force {
        hook(Phase::Check).map_err(io_error(Phase::Check))?;
        check(&target, expected, existing.as_ref())?;
    }
    if let Some(links) = existing.as_ref().map(link_count).filter(|&n| n > 1) {
        return in_place(
            &target,
            text,
            bom,
            SaveMethod::InPlaceHardLinks(links),
            hook,
        );
    }
    atomic(&target, text, bom, existing.as_ref(), hook)
}

/// `path` with symlinks resolved; for a missing file, its canonical
/// directory plus its name, following a dangling symlink chain (bounded).
pub(crate) fn resolve(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..40 {
        match fs::canonicalize(&path) {
            Ok(p) => return Ok(p),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            Err(_) => {}
        }
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                let link = fs::read_link(&path)?;
                path = match path.parent() {
                    Some(dir) if link.is_relative() => dir.join(link),
                    _ => link,
                };
            }
            _ => {
                let name = path.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "path has no file name")
                })?;
                let dir = match path.parent() {
                    Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                    _ => PathBuf::from("."),
                };
                return Ok(fs::canonicalize(dir)?.join(name));
            }
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// The conflict check: the target is what `expected` says it was.
fn check(
    target: &Path,
    expected: Option<&DiskState>,
    existing: Option<&fs::Metadata>,
) -> Result<(), SaveError> {
    match (expected, existing) {
        (None, None) => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(SaveError::Conflict),
        (Some(e), Some(m)) => {
            if m.len() != e.size || m.modified().ok() != e.modified {
                return Err(SaveError::Conflict);
            }
            // Same size and mtime (whose granularity can hide an edit):
            // compare the content.
            match hash_file(target) {
                Ok(h) if h == e.hash => Ok(()),
                Ok(_) => Err(SaveError::Conflict),
                Err(err) => Err(io_error(Phase::Check)(err)),
            }
        }
    }
}

#[cfg(unix)]
fn link_count(m: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.nlink()
}

#[cfg(not(unix))]
fn link_count(_: &fs::Metadata) -> u64 {
    1
}

/// A fresh name next to `target`: `.<name>.flash-<pid>-<nanos>-<i>.<ext>`.
fn sibling(target: &Path, ext: &str, i: u32) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut name = OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(".flash-{}-{nanos}-{i}.{ext}", std::process::id()));
    target.with_file_name(name)
}

/// Creates a new file next to `target` that did not exist before.
fn create_sibling(target: &Path, ext: &str, mode: u32) -> io::Result<(PathBuf, File)> {
    let mut last = None;
    for i in 0..100 {
        let path = sibling(target, ext, i);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        match options.open(&path) {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free temp name")))
}

/// Writes the BOM and the text's chunks, returning the size and hash.
fn write_content(file: &File, text: &Text, bom: bool) -> io::Result<(u64, u64)> {
    let mut out = BufWriter::with_capacity(64 << 10, file);
    let mut hash = Fnv::new();
    let mut size = 0u64;
    let mut put = |bytes: &[u8], out: &mut BufWriter<&File>| -> io::Result<()> {
        hash.update(bytes);
        size += bytes.len() as u64;
        out.write_all(bytes)
    };
    if bom {
        put(BOM, &mut out)?;
    }
    for chunk in text.chunks() {
        put(chunk.as_bytes(), &mut out)?;
    }
    out.flush()?;
    drop(out);
    Ok((size, hash.finish()))
}

/// Gives `file` the owner and group in `of`; `false` if that is
/// impossible.
#[cfg(unix)]
fn keep_owner(file: &File, of: &fs::Metadata) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let mine = file.metadata()?;
    if mine.uid() == of.uid() && mine.gid() == of.gid() {
        return Ok(true);
    }
    Ok(std::os::unix::fs::fchown(file, Some(of.uid()), Some(of.gid())).is_ok())
}

#[cfg(not(unix))]
fn keep_owner(_: &File, _: &fs::Metadata) -> io::Result<bool> {
    Ok(true)
}

fn atomic(
    target: &Path,
    text: &Text,
    bom: bool,
    existing: Option<&fs::Metadata>,
    hook: &mut dyn FnMut(Phase) -> io::Result<()>,
) -> Result<Saved, SaveError> {
    hook(Phase::CreateTemp).map_err(io_error(Phase::CreateTemp))?;
    // A new file gets the default permissions (0o666 less the umask); a
    // replacement starts private and gets the target's below.
    let mode = if existing.is_some() { 0o600 } else { 0o666 };
    let (temp, file) = create_sibling(target, "tmp", mode).map_err(io_error(Phase::CreateTemp))?;
    // From here on, any failure removes the temp file.
    let fail = |phase: Phase, e: io::Error| {
        let _ = fs::remove_file(&temp);
        io_error(phase)(e)
    };
    if let Some(m) = existing {
        let kept = match hook(Phase::Ownership) {
            Ok(()) => keep_owner(&file, m).map_err(|e| fail(Phase::Ownership, e))?,
            Err(_) => false,
        };
        if !kept {
            drop(file);
            let _ = fs::remove_file(&temp);
            return in_place(target, text, bom, SaveMethod::InPlaceOwnership, hook);
        }
    }
    let (size, hash) = hook(Phase::Write)
        .and_then(|()| write_content(&file, text, bom))
        .map_err(|e| fail(Phase::Write, e))?;
    hook(Phase::Sync)
        .and_then(|()| file.sync_all())
        .map_err(|e| fail(Phase::Sync, e))?;
    if let Some(m) = existing {
        hook(Phase::Permissions)
            .and_then(|()| file.set_permissions(m.permissions()))
            .map_err(|e| fail(Phase::Permissions, e))?;
    }
    drop(file);
    hook(Phase::Rename)
        .and_then(|()| fs::rename(&temp, target))
        .map_err(|e| fail(Phase::Rename, e))?;
    // The new content is in place; flushing the directory entry is best
    // effort (some file systems refuse it).
    let dir_synced = hook(Phase::SyncDir).and_then(|()| sync_dir(target)).is_ok();
    Ok(Saved {
        target: target.to_path_buf(),
        method: SaveMethod::Atomic,
        disk: disk_state(target, size, hash),
        dir_synced,
    })
}

fn in_place(
    target: &Path,
    text: &Text,
    bom: bool,
    method: SaveMethod,
    hook: &mut dyn FnMut(Phase) -> io::Result<()>,
) -> Result<Saved, SaveError> {
    // 1. A flushed copy of the original next to it.
    hook(Phase::Backup).map_err(io_error(Phase::Backup))?;
    let (backup, mut copy) =
        create_sibling(target, "bak", 0o600).map_err(io_error(Phase::Backup))?;
    let made = File::open(target)
        .and_then(|mut original| io::copy(&mut original, &mut copy))
        .and_then(|_| copy.sync_all());
    drop(copy);
    if let Err(e) = made {
        let _ = fs::remove_file(&backup);
        return Err(io_error(Phase::Backup)(e));
    }
    // 2. Truncate and write the target itself (same inode: every hard link
    //    sees it; owner and permissions unchanged).
    let written = hook(Phase::WriteInPlace).and_then(|()| {
        let file = OpenOptions::new().write(true).truncate(true).open(target)?;
        let sizes = write_content(&file, text, bom)?;
        file.sync_all()?;
        Ok(sizes)
    });
    let (size, hash) = match written {
        Ok(s) => s,
        Err(e) => {
            return Err(SaveError::InPlace {
                backup,
                kind: e.kind(),
                message: e.to_string(),
            });
        }
    };
    // 3. The backup is no longer needed.
    let _ = fs::remove_file(&backup);
    Ok(Saved {
        target: target.to_path_buf(),
        method,
        disk: disk_state(target, size, hash),
        dir_synced: true,
    })
}

#[cfg(unix)]
fn sync_dir(target: &Path) -> io::Result<()> {
    match target.parent() {
        Some(dir) => File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

/// The state of `target` just written: size and hash as written, mtime
/// from the file system.
fn disk_state(target: &Path, size: u64, hash: u64) -> DiskState {
    DiskState {
        size,
        modified: fs::metadata(target).and_then(|m| m.modified()).ok(),
        hash,
    }
}
