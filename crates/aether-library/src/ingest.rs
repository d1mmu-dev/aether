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
//!   `expected_size` must match exactly. Cancellation is checked before every chunk.
//! * **Commit:** a `.aether-ingest-<uuid>.part` sibling (created `O_EXCL`, same folder, so same
//!   filesystem) is fsynced, then renamed with `renameat2(RENAME_NOREPLACE)`; where the
//!   filesystem lacks that (EINVAL), `linkat` + `unlinkat`. Anything else fails closed: there is
//!   no check-then-rename fallback. An existing file, folder or (dangling) symlink at the target
//!   is [`IngestError::AlreadyExists`]; of concurrent writers to one name exactly one wins.
//! * **Cleanup:** on any failure (including a panic) only this call's own part file is removed,
//!   by its exact name relative to the pinned folder. Unknown `.part` files are never touched.
//!
//! Not covered: a local user who owns the roots can still rename whole folders around while a
//! copy runs; the descriptors keep the write inside the folder that was opened, but which
//! *name* that folder has afterwards is theirs to decide. Folders created before a failure are
//! left in place (they are empty or hold other files).
//!
//! The `linkat` fallback is not exercised by the tests (the test filesystem supports
//! `RENAME_NOREPLACE`).
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
        let root = sys::open_root(src_root)?;
        let mut file = sys::open_source(&root, src)?;
        copy_into(dest_root, dest, &mut file, limits, cancel)
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
        let root = sys::open_root(dest_root)?;
        let (dirs, leaf) = dest.split_leaf();
        let dir = sys::make_dirs(root, dirs)?;
        let mut part = sys::Part::create(&dir)?;
        let got = copy_bounded(src, part.file(), limits, cancel)?;
        part.commit(leaf)?;
        Ok(got)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (dest_root, dest, src, limits, cancel);
        Err(IngestError::Unsupported)
    }
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
    })
}

#[cfg(target_os = "linux")]
mod sys {
    use super::*;
    use std::ffi::CString;
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
        dir: &'a OwnedFd,
        name: CString,
        file: File,
        live: bool,
    }

    impl<'a> Part<'a> {
        pub fn create(dir: &'a OwnedFd) -> Result<Self, IngestError> {
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
                dir,
                name,
                file,
                live: true,
            })
        }

        pub fn file(&mut self) -> &mut File {
            &mut self.file
        }

        /// fsync, then give the part the name `leaf` without replacing anything.
        pub fn commit(mut self, leaf: &FileName) -> Result<(), IngestError> {
            self.file.sync_all()?;
            let to = cstr(leaf.as_str().as_bytes())?;
            let d = self.dir.as_raw_fd();
            // SAFETY: valid descriptors and NUL-terminated names.
            let r = unsafe {
                libc::renameat2(
                    d,
                    self.name.as_ptr(),
                    d,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if r != 0 {
                let e = io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::EINVAL) {
                    return Err(e.into());
                }
                // No RENAME_NOREPLACE on this filesystem: link (fails on any existing name),
                // then drop the part's name. Without hard links either, fail closed.
                // SAFETY: as above; flags 0 = do not follow.
                let r = unsafe { libc::linkat(d, self.name.as_ptr(), d, to.as_ptr(), 0) };
                if r != 0 {
                    let e = io::Error::last_os_error();
                    return Err(match e.raw_os_error() {
                        Some(libc::EPERM) | Some(libc::EOPNOTSUPP) => IngestError::Unsupported,
                        _ => e.into(),
                    });
                }
                // The target now exists; the part name is removed by the drop below.
                self.remove();
            }
            self.live = false;
            // Make the new name durable.
            // SAFETY: `d` stays owned by `self.dir`; fsync does not take ownership.
            cvt(unsafe { libc::fsync(d) })?;
            Ok(())
        }

        fn remove(&mut self) {
            // SAFETY: valid descriptor and NUL-terminated name of the file this call created.
            unsafe { libc::unlinkat(self.dir.as_raw_fd(), self.name.as_ptr(), 0) };
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
