//! Safe local-file ingestion (M7 Task 3, D-293): copies one finished file from an explicit
//! source root (the download folder) to a path under an explicit destination root
//! (`MUSIC_PATH`) without following symlinks, without ever replacing an existing entry and
//! without leaving anything but the committed file behind.
//!
//! * **Names are validated, never repaired.** [`FileName`] is one path component, [`RelPath`]
//!   a relative path of them. Remote (Soulseek) text only enters through
//!   [`remote_file_name`], which takes the basename and refuses it if malformed; turning
//!   metadata into names (sanitizing) is the caller's explicit job.
//! * **Anchored resolution (Linux).** Each root is opened once with `openat2(...,
//!   RESOLVE_NO_SYMLINKS)` over its whole absolute path. Everything below is resolved from that
//!   descriptor with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS`, one
//!   component at a time for destination folders (created with `mkdirat`, then reopened the
//!   same way), so a symlink anywhere — escaping or not, dangling or not — is refused, and
//!   swapping a folder for a symlink between checks cannot redirect the write (by
//!   construction; the tests cover symlinks placed beforehand, not a live swap race).
//! * **Source:** opened `O_RDONLY | O_NONBLOCK | O_NOFOLLOW` (a FIFO cannot block the open) and
//!   must be a regular file by `fstat` of that descriptor. Hard links are accepted: the copy is a
//!   new inode, the source is only read and never removed.
//! * **Bounded copy:** at most `max_bytes` are written; reading one byte more is
//!   [`IngestError::TooLarge`], independent of the size the source claims. An optional
//!   `expected_size` must match exactly. Cancellation is checked before every chunk and once
//!   more after the part's fsync, immediately before publication.
//! * **Commit:** a `.aether-ingest-<uuid>.part` sibling (created `O_EXCL`, same folder, so same
//!   filesystem) is fsynced, then renamed with `renameat2(RENAME_NOREPLACE)`; where the
//!   filesystem lacks that (EINVAL), `linkat` + `unlinkat`. Anything else fails closed: there is
//!   no check-then-rename fallback. An existing file, folder or (dangling) symlink at the target
//!   is [`IngestError::AlreadyExists`]; of concurrent writers to one name exactly one wins.
//! * **Publication** (the successful `renameat2` or `linkat`) is the commit and cancellation
//!   linearization point. Before it, every error (including [`IngestError::Cancelled`]) means
//!   nothing was published. After it, nothing is rolled back and cancellation is ignored: a
//!   folder fsync failure is [`IngestError::PublishedNotDurable`] carrying the receipt, and an
//!   unremovable fallback part is reported in [`Ingested::leftover_part`]. Callers (the
//!   download job) must treat a cancel that races the commit as decided by this outcome, not
//!   by having asked to cancel.
//! * **Cleanup:** on any failure before publication (including a panic) only this call's own
//!   part file is removed, best effort, by its exact name relative to the pinned folder.
//!   Unknown `.part` files are never touched.
//!
//! Not covered: a local user who owns the roots can still rename whole folders around while a
//! copy runs; the descriptors keep the write inside the folder that was opened, but which
//! *name* that folder has afterwards is theirs to decide. Folders created before a failure are
//! left in place (they are empty or hold other files).
//!
//! The `linkat` fallback and post-publication faults are exercised by this module's unit tests
//! through a private table of the commit syscalls ([`sys::Ops`]); production always uses the
//! real calls.
//!
//! Synchronous: async callers use `spawn_blocking`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

/// Longest component, in bytes (Linux `NAME_MAX`).
const NAME_MAX: usize = 255;
/// Copy buffer.
const CHUNK: usize = 64 * 1024;
/// Reserved prefix of this module's temporary files; refused as a real name.
const PART_PREFIX: &str = ".aether-ingest-";

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("invalid path: {0}")]
    InvalidPath(&'static str),
    #[error("the root must be an absolute path to an existing directory")]
    InvalidRoot,
    #[error("no such file or directory")]
    NotFound,
    #[error("a symbolic link is in the path")]
    Symlink,
    #[error("a path component is not a directory")]
    NotADirectory,
    #[error("the source is not a regular file")]
    NotRegularFile,
    #[error("the target already exists")]
    AlreadyExists,
    #[error("the file is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("cancelled")]
    Cancelled,
    #[error("the filesystem does not support a safe no-replace commit")]
    Unsupported,
    /// The file **is published** under its final name with the contents in `receipt`, but the
    /// folder fsync after publication failed, so the new name may not survive a crash. Not a
    /// failure to copy: do not retry blindly (the retry would see `AlreadyExists`); reconcile
    /// the destination against `receipt` instead.
    #[error("published, but the folder fsync failed: {source}")]
    PublishedNotDurable {
        receipt: Ingested,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Io(io::Error),
}

impl From<io::Error> for IngestError {
    fn from(e: io::Error) -> Self {
        #[cfg(target_os = "linux")]
        match e.raw_os_error() {
            // ELOOP: RESOLVE_NO_SYMLINKS/O_NOFOLLOW hit a link; EXDEV: RESOLVE_BENEATH escape.
            Some(libc::ELOOP) | Some(libc::EXDEV) => return Self::Symlink,
            Some(libc::ENOENT) => return Self::NotFound,
            Some(libc::ENOTDIR) => return Self::NotADirectory,
            Some(libc::EEXIST) => return Self::AlreadyExists,
            _ => {}
        }
        Self::Io(e)
    }
}

/// One validated path component: 1..=255 bytes, not `.`/`..`, no `/`, `\`, NUL or other
/// control character, no Windows drive prefix (`C:`) and not this module's part prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName(String);

impl FileName {
    pub fn new(s: &str) -> Result<Self, IngestError> {
        let bad = IngestError::InvalidPath;
        if s.is_empty() {
            return Err(bad("empty component"));
        }
        if s.len() > NAME_MAX {
            return Err(bad("component longer than 255 bytes"));
        }
        if s == "." || s == ".." {
            return Err(bad("dot component"));
        }
        if s.chars().any(|c| c == '/' || c == '\\' || c.is_control()) {
            return Err(bad("separator or control character"));
        }
        let b = s.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return Err(bad("drive prefix"));
        }
        if s.starts_with(PART_PREFIX) {
            return Err(bad("reserved name"));
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A non-empty relative path of [`FileName`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelPath(Vec<FileName>);

impl RelPath {
    pub fn new(components: Vec<FileName>) -> Result<Self, IngestError> {
        if components.is_empty() {
            return Err(IngestError::InvalidPath("empty path"));
        }
        Ok(Self(components))
    }

    /// A local `/`-separated relative path. Absolute, empty, `.`/`..` and doubled or trailing
    /// separators are refused, not normalized.
    pub fn parse(s: &str) -> Result<Self, IngestError> {
        if s.starts_with('/') {
            return Err(IngestError::InvalidPath("absolute path"));
        }
        Self::new(s.split('/').map(FileName::new).collect::<Result<_, _>>()?)
    }

    pub fn components(&self) -> &[FileName] {
        &self.0
    }

    pub fn as_path(&self) -> PathBuf {
        self.0.iter().map(FileName::as_str).collect()
    }

    fn split_leaf(&self) -> (&[FileName], &FileName) {
        let (leaf, dirs) = self.0.split_last().expect("RelPath is never empty");
        (dirs, leaf)
    }
}

/// The basename of a remote (Soulseek, `\`- or `/`-separated) file path, refused when it is not
/// a valid [`FileName`]. The rest of the remote path is never used as a local path.
pub fn remote_file_name(remote: &str) -> Result<FileName, IngestError> {
    FileName::new(remote.rsplit(['\\', '/']).next().unwrap_or_default())
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most bytes copied; one more is [`IngestError::TooLarge`].
    pub max_bytes: u64,
    /// When set, the copy must have exactly this many bytes.
    pub expected_size: Option<u64>,
}

/// The committed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    pub bytes: u64,
    pub sha256: [u8; 32],
    /// Set when the hard-link fallback published the file but could not remove this call's
    /// part name: that hidden name is a second link to the published file and is left behind.
    pub leftover_part: Option<LeftoverPart>,
}

/// A part file left behind after a successful publication (see [`Ingested::leftover_part`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftoverPart {
    /// Its name in the target file's folder (`.aether-ingest-<uuid>.part`).
    pub name: String,
    /// Why `unlinkat` failed.
    pub error: io::ErrorKind,
    pub os_error: Option<i32>,
}

/// Copies `src_root/src` to `dest_root/dest` (see the module docs).
pub fn ingest_file(
    src_root: &Path,
    src: &RelPath,
    dest_root: &Path,
    dest: &RelPath,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Ingested, IngestError> {
    #[cfg(target_os = "linux")]
    {
        ingest_file_with(&sys::REAL, src_root, src, dest_root, dest, limits, cancel)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (src_root, src, dest_root, dest, limits, cancel);
        Err(IngestError::Unsupported)
    }
}

/// Streams `src` into `dest_root/dest` with the same folder, limit and commit rules.
pub fn copy_into(
    dest_root: &Path,
    dest: &RelPath,
    src: &mut dyn Read,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Ingested, IngestError> {
    #[cfg(target_os = "linux")]
    {
        copy_into_with(&sys::REAL, dest_root, dest, src, limits, cancel)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (dest_root, dest, src, limits, cancel);
        Err(IngestError::Unsupported)
    }
}

#[cfg(target_os = "linux")]
fn ingest_file_with(
    ops: &'static sys::Ops,
    src_root: &Path,
    src: &RelPath,
    dest_root: &Path,
    dest: &RelPath,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Ingested, IngestError> {
    let root = sys::open_root(src_root)?;
    let mut file = sys::open_source(&root, src)?;
    copy_into_with(ops, dest_root, dest, &mut file, limits, cancel)
}

#[cfg(target_os = "linux")]
fn copy_into_with(
    ops: &'static sys::Ops,
    dest_root: &Path,
    dest: &RelPath,
    src: &mut dyn Read,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Ingested, IngestError> {
    let root = sys::open_root(dest_root)?;
    let (dirs, leaf) = dest.split_leaf();
    let dir = sys::make_dirs(root, dirs)?;
    let mut part = sys::Part::create(ops, &dir)?;
    let got = copy_bounded(src, part.file(), limits, cancel)?;
    part.commit(leaf, cancel, got)
}

/// Copies at most `limits.max_bytes` (reading at most one byte more to detect overflow).
fn copy_bounded(
    src: &mut dyn Read,
    out: &mut dyn Write,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<Ingested, IngestError> {
    let mut buf = vec![0u8; CHUNK];
    let mut hash = Sha256::new();
    let mut total: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(IngestError::Cancelled);
        }
        // Room left plus one byte, so an oversize source is seen without reading it all.
        let room = limits.max_bytes.saturating_sub(total).saturating_add(1);
        let want = usize::try_from(room).map_or(CHUNK, |r| r.min(CHUNK));
        let n = match src.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(IngestError::Io(e)),
        };
        total += n as u64;
        if total > limits.max_bytes {
            return Err(IngestError::TooLarge {
                limit: limits.max_bytes,
            });
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(IngestError::Io)?;
    }
    if let Some(expected) = limits.expected_size
        && expected != total
    {
        return Err(IngestError::SizeMismatch {
            expected,
            actual: total,
        });
    }
    Ok(Ingested {
        bytes: total,
        sha256: hash.finalize().into(),
        leftover_part: None,
    })
}

#[cfg(target_os = "linux")]
mod sys {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;

    const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;
    /// Everything below a root.
    const BENEATH: u64 = RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS;

    /// `struct open_how` (linux/openat2.h).
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }

    fn cstr(s: &[u8]) -> Result<CString, IngestError> {
        CString::new(s).map_err(|_| IngestError::InvalidPath("NUL byte"))
    }

    fn openat2(
        dir: RawFd,
        path: &CString,
        flags: i32,
        resolve: u64,
    ) -> Result<OwnedFd, IngestError> {
        let how = OpenHow {
            flags: (flags | libc::O_CLOEXEC) as u64,
            mode: 0,
            resolve,
        };
        // EAGAIN: the kernel saw a concurrent rename/mount during a BENEATH lookup; retry a
        // bounded number of times, then give up.
        for _ in 0..16 {
            // SAFETY: `path` and `how` are valid for the duration of the call; the size matches
            // the struct passed.
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_openat2,
                    dir,
                    path.as_ptr(),
                    &how,
                    std::mem::size_of::<OpenHow>(),
                )
            };
            if fd >= 0 {
                // SAFETY: a successful openat2 returns a fresh descriptor that we now own.
                return Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) });
            }
            let e = io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::EINTR) | Some(libc::EAGAIN) => continue,
                // No openat2 (Linux < 5.6, seccomp): fail closed.
                Some(libc::ENOSYS) => return Err(IngestError::Unsupported),
                _ => return Err(e.into()),
            }
        }
        Err(IngestError::Io(io::Error::from_raw_os_error(libc::EAGAIN)))
    }

    fn cvt(r: libc::c_int) -> io::Result<()> {
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// The exact syscalls of the commit step. Production always uses [`REAL`]; the module's
    /// unit tests substitute single entries to inject faults (no global or environment hook).
    pub struct Ops {
        /// fsync of the part before publication.
        pub sync_part: fn(&File) -> io::Result<()>,
        /// `renameat2(dir, from, dir, to, RENAME_NOREPLACE)`.
        pub rename_noreplace: fn(RawFd, &CStr, &CStr) -> io::Result<()>,
        /// `linkat(dir, from, dir, to, 0)` (never follows).
        pub link: fn(RawFd, &CStr, &CStr) -> io::Result<()>,
        /// `unlinkat(dir, name, 0)`.
        pub unlink: fn(RawFd, &CStr) -> io::Result<()>,
        /// fsync of the folder after publication.
        pub sync_dir: fn(RawFd) -> io::Result<()>,
    }

    pub static REAL: Ops = Ops {
        sync_part: File::sync_all,
        rename_noreplace: |d, from, to| {
            // SAFETY: valid descriptor and NUL-terminated names.
            cvt(unsafe {
                libc::renameat2(d, from.as_ptr(), d, to.as_ptr(), libc::RENAME_NOREPLACE)
            })
        },
        // SAFETY: valid descriptor and NUL-terminated names; flags 0 = do not follow.
        link: |d, from, to| cvt(unsafe { libc::linkat(d, from.as_ptr(), d, to.as_ptr(), 0) }),
        // SAFETY: valid descriptor and NUL-terminated name.
        unlink: |d, name| cvt(unsafe { libc::unlinkat(d, name.as_ptr(), 0) }),
        // SAFETY: the caller keeps the descriptor owned; fsync does not take ownership.
        sync_dir: |d| cvt(unsafe { libc::fsync(d) }),
    };

    /// A root: absolute, an existing directory, no symlink anywhere in its path.
    pub fn open_root(path: &Path) -> Result<OwnedFd, IngestError> {
        if !path.is_absolute() {
            return Err(IngestError::InvalidRoot);
        }
        let p = cstr(path.as_os_str().as_bytes())?;
        openat2(
            libc::AT_FDCWD,
            &p,
            libc::O_RDONLY | libc::O_DIRECTORY,
            RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
        )
        .map_err(|e| match e {
            IngestError::NotADirectory => IngestError::InvalidRoot,
            e => e,
        })
    }

    /// The source under `root`: a regular file, opened without following or blocking.
    pub fn open_source(root: &OwnedFd, rel: &RelPath) -> Result<File, IngestError> {
        let p = cstr(rel.as_path().as_os_str().as_bytes())?;
        let fd = openat2(
            root.as_raw_fd(),
            &p,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY,
            BENEATH,
        )?;
        let file = File::from(fd);
        if !file.metadata()?.file_type().is_file() {
            return Err(IngestError::NotRegularFile);
        }
        Ok(file)
    }

    /// Opens (creating as needed) each folder of `dirs` below `root`, one component at a
    /// time, never through a symlink. Returns the deepest one.
    pub fn make_dirs(root: OwnedFd, dirs: &[FileName]) -> Result<OwnedFd, IngestError> {
        let mut cur = root;
        for d in dirs {
            let name = cstr(d.as_str().as_bytes())?;
            // SAFETY: valid descriptor and NUL-terminated name.
            let created = match cvt(unsafe { libc::mkdirat(cur.as_raw_fd(), name.as_ptr(), 0o755) })
            {
                Ok(()) => true,
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => false,
                Err(e) => return Err(e.into()),
            };
            let next = openat2(
                cur.as_raw_fd(),
                &name,
                // No O_NOFOLLOW: with O_DIRECTORY it turns a symlink into ENOTDIR;
                // RESOLVE_NO_SYMLINKS already refuses it (ELOOP).
                libc::O_RDONLY | libc::O_DIRECTORY,
                BENEATH,
            )?;
            if created {
                // The new entry must survive a crash once the file in it is committed.
                File::from(cur).sync_all()?;
            }
            cur = next;
        }
        Ok(cur)
    }

    /// This call's own temporary file; removed on drop unless committed.
    pub struct Part<'a> {
        ops: &'static Ops,
        dir: &'a OwnedFd,
        name: CString,
        file: File,
        live: bool,
    }

    impl<'a> Part<'a> {
        pub fn create(ops: &'static Ops, dir: &'a OwnedFd) -> Result<Self, IngestError> {
            let name = cstr(format!("{PART_PREFIX}{}.part", uuid::Uuid::new_v4()).as_bytes())?;
            // SAFETY: valid descriptor and NUL-terminated name; O_CREAT|O_EXCL never follows.
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o644 as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error().into());
            }
            // SAFETY: fresh descriptor we own.
            let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            Ok(Self {
                ops,
                dir,
                name,
                file,
                live: true,
            })
        }

        pub fn file(&mut self) -> &mut File {
            &mut self.file
        }

        /// fsync, check cancellation, then give the part the name `leaf` without replacing
        /// anything (the publication point), then fsync the folder.
        pub fn commit(
            mut self,
            leaf: &FileName,
            cancel: &AtomicBool,
            mut got: Ingested,
        ) -> Result<Ingested, IngestError> {
            (self.ops.sync_part)(&self.file)?;
            let to = cstr(leaf.as_str().as_bytes())?;
            let d = self.dir.as_raw_fd();
            // Last cancellation check, after the (possibly slow) fsync and immediately before
            // the publishing syscall. A cancel arriving later loses the race: the file is
            // published and the caller gets the published outcome.
            if cancel.load(Ordering::SeqCst) {
                return Err(IngestError::Cancelled);
            }
            if let Err(e) = (self.ops.rename_noreplace)(d, &self.name, &to) {
                if e.raw_os_error() != Some(libc::EINVAL) {
                    return Err(e.into());
                }
                // No RENAME_NOREPLACE on this filesystem: link (fails on any existing name),
                // then drop the part's name. Without hard links either, fail closed.
                if let Err(e) = (self.ops.link)(d, &self.name, &to) {
                    return Err(match e.raw_os_error() {
                        Some(libc::EPERM) | Some(libc::EOPNOTSUPP) => IngestError::Unsupported,
                        _ => e.into(),
                    });
                }
                // Published: from here on nothing is rolled back. Drop the part's name; if that
                // fails it stays as a second link to the published file and is reported.
                self.live = false;
                if let Err(e) = (self.ops.unlink)(d, &self.name) {
                    got.leftover_part = Some(LeftoverPart {
                        name: self.name.to_string_lossy().into_owned(),
                        error: e.kind(),
                        os_error: e.raw_os_error(),
                    });
                }
            }
            self.live = false;
            // Make the new name durable. A failure here does not unpublish the file.
            match (self.ops.sync_dir)(d) {
                Ok(()) => Ok(got),
                Err(source) => Err(IngestError::PublishedNotDurable {
                    receipt: got,
                    source,
                }),
            }
        }

        fn remove(&mut self) {
            // Best effort: the name is the file this call created.
            let _ = (self.ops.unlink)(self.dir.as_raw_fd(), &self.name);
        }
    }

    impl Drop for Part<'_> {
        fn drop(&mut self) {
            if self.live {
                self.remove();
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    //! Fault injection into the commit syscalls. Each test uses its own `Ops` and statics, so
    //! nothing leaks between parallel tests or into production.
    use super::sys::{Ops, REAL};
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, symlink};

    const DATA: &[u8] = b"published bytes";

    fn errno(e: i32) -> io::Error {
        io::Error::from_raw_os_error(e)
    }

    /// Ops whose `renameat2` reports EINVAL (no `RENAME_NOREPLACE`), forcing the real
    /// `linkat` + `unlinkat` fallback.
    const NO_NOREPLACE: Ops = Ops {
        rename_noreplace: |_, _, _| Err(errno(libc::EINVAL)),
        ..REAL_COPY
    };
    const REAL_COPY: Ops = Ops {
        sync_part: REAL.sync_part,
        rename_noreplace: REAL.rename_noreplace,
        link: REAL.link,
        unlink: REAL.unlink,
        sync_dir: REAL.sync_dir,
    };

    fn music() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().expect("tempdir under $TMPDIR");
        let m = t.path().join("music");
        fs::create_dir_all(m.join("A")).unwrap();
        (t, m)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn go(
        ops: &'static Ops,
        m: &Path,
        leaf: &str,
        cancel: &AtomicBool,
    ) -> Result<Ingested, IngestError> {
        copy_into_with(
            ops,
            m,
            &RelPath::parse(&format!("A/{leaf}")).unwrap(),
            &mut &DATA[..],
            Limits {
                max_bytes: 1 << 20,
                expected_size: Some(DATA.len() as u64),
            },
            cancel,
        )
    }

    fn digest() -> [u8; 32] {
        Sha256::digest(DATA).into()
    }

    #[test]
    fn hard_link_fallback_publishes_without_replacing_and_keeps_the_source() {
        static OPS: Ops = NO_NOREPLACE;
        let (t, m) = music();
        let src = t.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("in"), DATA).unwrap();
        let got = ingest_file_with(
            &OPS,
            &src,
            &RelPath::parse("in").unwrap(),
            &m,
            &RelPath::parse("A/new.flac").unwrap(),
            Limits {
                max_bytes: 1 << 20,
                expected_size: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            (got.bytes, got.sha256, got.leftover_part),
            (DATA.len() as u64, digest(), None)
        );
        assert_eq!(fs::read(m.join("A/new.flac")).unwrap(), DATA);
        assert_eq!(
            fs::metadata(m.join("A/new.flac")).unwrap().nlink(),
            1,
            "part name removed"
        );
        assert_eq!(fs::read(src.join("in")).unwrap(), DATA);

        // linkat refuses every existing kind of target (EEXIST): nothing is replaced.
        let outside = t.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        fs::write(m.join("A/file.flac"), b"old").unwrap();
        fs::create_dir(m.join("A/dir.flac")).unwrap();
        symlink(&outside, m.join("A/link.flac")).unwrap();
        symlink(t.path().join("missing"), m.join("A/dangling.flac")).unwrap();
        for leaf in [
            "file.flac",
            "dir.flac",
            "link.flac",
            "dangling.flac",
            "new.flac",
        ] {
            let r = go(&OPS, &m, leaf, &AtomicBool::new(false));
            assert!(
                matches!(r, Err(IngestError::AlreadyExists)),
                "{leaf}: {r:?}"
            );
        }
        assert_eq!(fs::read(m.join("A/file.flac")).unwrap(), b"old");
        assert!(m.join("A/dir.flac").is_dir());
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(!t.path().join("missing").exists());
        assert_eq!(
            entries(&m.join("A")),
            [
                "dangling.flac",
                "dir.flac",
                "file.flac",
                "link.flac",
                "new.flac"
            ]
        );

        // No hard links either: fail closed, nothing published, part removed.
        static NO_LINKS: Ops = Ops {
            link: |_, _, _| Err(errno(libc::EPERM)),
            ..NO_NOREPLACE
        };
        let r = go(&NO_LINKS, &m, "x.flac", &AtomicBool::new(false));
        assert!(matches!(r, Err(IngestError::Unsupported)), "{r:?}");
        assert!(!m.join("A/x.flac").exists());
        assert_eq!(entries(&m.join("A")).len(), 5);
    }

    #[test]
    fn hard_link_fallback_reports_a_part_it_could_not_remove() {
        static OPS: Ops = Ops {
            unlink: |_, _| Err(errno(libc::EIO)),
            ..NO_NOREPLACE
        };
        let (_t, m) = music();
        let got = go(&OPS, &m, "f.flac", &AtomicBool::new(false)).expect("published");
        assert_eq!((got.bytes, got.sha256), (DATA.len() as u64, digest()));
        let left = got.leftover_part.expect("the leftover part is reported");
        assert_eq!(
            (left.error, left.os_error),
            (errno(libc::EIO).kind(), Some(libc::EIO))
        );
        assert!(
            left.name.starts_with(PART_PREFIX) && left.name.ends_with(".part"),
            "{left:?}"
        );
        // The final file is complete; the reported part is a second link to it, not a copy.
        assert_eq!(
            entries(&m.join("A")),
            [left.name.clone(), "f.flac".to_owned()]
        );
        assert_eq!(fs::read(m.join("A/f.flac")).unwrap(), DATA);
        let (a, b) = (
            fs::metadata(m.join("A/f.flac")).unwrap(),
            fs::metadata(m.join("A").join(&left.name)).unwrap(),
        );
        assert_eq!((a.ino(), a.nlink()), (b.ino(), 2));
    }

    #[test]
    fn folder_fsync_failure_after_publication_is_published_not_durable() {
        static OPS: Ops = Ops {
            sync_dir: |_| Err(errno(libc::EIO)),
            ..REAL_COPY
        };
        static FALLBACK: Ops = Ops {
            sync_dir: |_| Err(errno(libc::EIO)),
            unlink: |_, _| Err(errno(libc::EACCES)),
            ..NO_NOREPLACE
        };
        for (ops, leftover) in [(&OPS, false), (&FALLBACK, true)] {
            let (_t, m) = music();
            let r = go(ops, &m, "f.flac", &AtomicBool::new(false));
            let Err(IngestError::PublishedNotDurable { receipt, source }) = r else {
                panic!("{r:?}");
            };
            assert_eq!(source.raw_os_error(), Some(libc::EIO));
            assert_eq!(
                (receipt.bytes, receipt.sha256),
                (DATA.len() as u64, digest())
            );
            assert_eq!(receipt.leftover_part.is_some(), leftover);
            // Never rolled back: the final file exists and matches the receipt.
            let out = fs::read(m.join("A/f.flac")).unwrap();
            assert_eq!(<[u8; 32]>::from(Sha256::digest(&out)), receipt.sha256);
            assert_eq!(entries(&m.join("A")).len(), 1 + leftover as usize);
        }
    }

    #[test]
    fn part_fsync_failure_before_publication_publishes_nothing() {
        static OPS: Ops = Ops {
            sync_part: |_| Err(errno(libc::EIO)),
            ..REAL_COPY
        };
        let (_t, m) = music();
        let r = go(&OPS, &m, "f.flac", &AtomicBool::new(false));
        assert!(
            matches!(&r, Err(IngestError::Io(e)) if e.raw_os_error() == Some(libc::EIO)),
            "{r:?}"
        );
        assert!(entries(&m.join("A")).is_empty());
    }

    #[test]
    fn cancel_during_part_fsync_publishes_nothing_but_after_publication_is_too_late() {
        static EARLY: AtomicBool = AtomicBool::new(false);
        static CANCEL_IN_FSYNC: Ops = Ops {
            sync_part: |f| {
                EARLY.store(true, Ordering::SeqCst);
                f.sync_all()
            },
            ..REAL_COPY
        };
        let (_t, m) = music();
        let r = go(&CANCEL_IN_FSYNC, &m, "f.flac", &EARLY);
        assert!(matches!(r, Err(IngestError::Cancelled)), "{r:?}");
        assert!(entries(&m.join("A")).is_empty());

        // A cancel arriving after publication does not undo it: the outcome stays published.
        static LATE: AtomicBool = AtomicBool::new(false);
        static CANCEL_AFTER: Ops = Ops {
            sync_dir: |d| {
                LATE.store(true, Ordering::SeqCst);
                (REAL.sync_dir)(d)
            },
            ..REAL_COPY
        };
        let got = go(&CANCEL_AFTER, &m, "f.flac", &LATE).expect("published before the cancel");
        assert_eq!(got.sha256, digest());
        assert_eq!(fs::read(m.join("A/f.flac")).unwrap(), DATA);
        assert_eq!(entries(&m.join("A")), ["f.flac"]);
    }
}
