//! M7 Task 3: safe local-file ingestion under an explicit root (`aether_library::ingest`).
//! Every test uses its own temporary directory under `$TMPDIR`.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

use aether_library::ingest::{
    FileName, IngestError, Limits, RelPath, copy_into, ingest_file, remote_file_name,
};
use sha2::{Digest, Sha256};

const MIB: u64 = 1 << 20;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir under $TMPDIR")
}

/// `<tmp>/src` (download root) and `<tmp>/music` (destination root).
fn roots(t: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let s = t.path().join("src");
    let m = t.path().join("music");
    fs::create_dir(&s).unwrap();
    fs::create_dir(&m).unwrap();
    (s, m)
}

fn rel(s: &str) -> RelPath {
    RelPath::parse(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
}

fn limits(max: u64) -> Limits {
    Limits {
        max_bytes: max,
        expected_size: None,
    }
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// Every entry under `dir`, relative, sorted (symlinks not followed).
fn tree(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = walkdir::WalkDir::new(dir)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .map(|e| {
            e.unwrap()
                .path()
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    out.sort();
    out
}

fn parts(dir: &Path) -> Vec<String> {
    tree(dir)
        .into_iter()
        .filter(|p| p.contains(".aether-ingest-"))
        .collect()
}

// ---------- pure validators ----------

#[test]
fn file_name_rejects_unsafe_components() {
    for bad in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "C:",
        "c:track.flac",
        "nul\0byte",
        "tab\there",
        "new\nline",
        "del\u{7f}",
        "c1\u{85}",
        ".aether-ingest-x.part",
        &"x".repeat(256),
    ] {
        assert!(FileName::new(bad).is_err(), "accepted {bad:?}");
    }
    // 255 bytes of multi-byte UTF-8 boundary: 85 × 3 = 255 ok, one more byte too long.
    assert!(FileName::new(&"ż".repeat(127)).is_ok()); // 254 bytes
    assert!(FileName::new(&format!("{}a", "ż".repeat(127))).is_ok()); // 255
    assert!(FileName::new(&format!("{}ż", "ż".repeat(127))).is_err()); // 256
    for good in [
        "01 - Song: Remix.flac",
        "Ąę ünïcode",
        ".hidden",
        "a..b",
        "x y",
    ] {
        assert_eq!(FileName::new(good).unwrap().as_str(), good);
    }
}

#[test]
fn rel_path_rejects_absolute_windows_unc_traversal_and_empty() {
    for bad in [
        "",
        "/abs/file",
        "//server/share/f",
        "\\\\server\\share\\f",
        "C:\\Music\\f.flac",
        "C:/Music/f.flac",
        "a/../b",
        "../b",
        "a/./b",
        "a//b",
        "a/b/",
        "a\\b",
        "a/\u{1b}[31m/b",
        "a/b\0",
    ] {
        assert!(RelPath::parse(bad).is_err(), "accepted {bad:?}");
    }
    let p = rel("Artist/Album (2001)/01 - Song.flac");
    assert_eq!(p.components().len(), 3);
    assert_eq!(p.as_path(), Path::new("Artist/Album (2001)/01 - Song.flac"));
    let built = RelPath::new(vec![
        FileName::new("Artist").unwrap(),
        FileName::new("Album").unwrap(),
        FileName::new("x.flac").unwrap(),
    ])
    .unwrap();
    assert_eq!(built.as_path(), Path::new("Artist/Album/x.flac"));
    assert!(RelPath::new(vec![]).is_err());
}

#[test]
fn remote_file_name_takes_basename_without_repair() {
    let n = remote_file_name("@@abcde\\Music\\Artist\\Album\\01 - Song.flac").unwrap();
    assert_eq!(n.as_str(), "01 - Song.flac");
    assert_eq!(remote_file_name("a/b/c.mp3").unwrap().as_str(), "c.mp3");
    // Malformed basenames are refused, never cleaned up.
    for bad in ["dir\\", "dir\\..", "x\\.", "a\\b\u{1}c.flac", "", "\\"] {
        assert!(remote_file_name(bad).is_err(), "accepted {bad:?}");
    }
}

// ---------- filesystem ----------

#[test]
fn nested_copy_preserves_content_and_source() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::create_dir_all(src.join("slskd/user")).unwrap();
    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 31 % 251) as u8).collect();
    fs::write(src.join("slskd/user/in.flac"), &data).unwrap();
    let got = ingest_file(
        &src,
        &rel("slskd/user/in.flac"),
        &music,
        &rel("Artist/Album/01 - Song.flac"),
        Limits {
            max_bytes: data.len() as u64,
            expected_size: Some(data.len() as u64),
        },
        &no_cancel(),
    )
    .unwrap();
    let out = fs::read(music.join("Artist/Album/01 - Song.flac")).unwrap();
    assert_eq!(out, data);
    assert_eq!(got.bytes, data.len() as u64);
    assert_eq!(got.sha256, <[u8; 32]>::from(Sha256::digest(&data)));
    assert_eq!(fs::read(src.join("slskd/user/in.flac")).unwrap(), data);
    assert!(parts(&music).is_empty());
    assert_eq!(
        tree(&music),
        ["Artist", "Artist/Album", "Artist/Album/01 - Song.flac"]
    );
}

#[test]
fn existing_target_file_dir_and_symlinks_are_never_replaced() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), b"new").unwrap();
    let outside = t.path().join("outside");
    fs::write(&outside, b"outside").unwrap();
    fs::create_dir_all(music.join("A/dir.flac")).unwrap();
    fs::write(music.join("A/file.flac"), b"old").unwrap();
    symlink(&outside, music.join("A/link.flac")).unwrap();
    symlink(t.path().join("missing"), music.join("A/dangling.flac")).unwrap();
    for leaf in ["file.flac", "dir.flac", "link.flac", "dangling.flac"] {
        let r = ingest_file(
            &src,
            &rel("in"),
            &music,
            &rel(&format!("A/{leaf}")),
            limits(MIB),
            &no_cancel(),
        );
        assert!(
            matches!(r, Err(IngestError::AlreadyExists)),
            "{leaf}: {r:?}"
        );
    }
    assert_eq!(fs::read(music.join("A/file.flac")).unwrap(), b"old");
    assert!(music.join("A/dir.flac").is_dir());
    assert_eq!(fs::read(&outside).unwrap(), b"outside");
    assert!(!t.path().join("missing").exists());
    assert_eq!(
        fs::read_link(music.join("A/dangling.flac")).unwrap(),
        t.path().join("missing")
    );
    assert!(parts(&music).is_empty());
    assert_eq!(fs::read(src.join("in")).unwrap(), b"new");
}

#[test]
fn destination_parent_symlinks_are_refused_even_inside_root() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), b"x").unwrap();
    let outside = t.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(music.join("real")).unwrap();
    symlink(&outside, music.join("escape")).unwrap();
    symlink("real", music.join("inner")).unwrap(); // relative, stays inside the root
    symlink(t.path().join("nowhere"), music.join("dangling")).unwrap();
    fs::create_dir(music.join("ok")).unwrap();
    symlink(&outside, music.join("ok/deep")).unwrap();
    for dest in [
        "escape/f.flac",
        "inner/f.flac",
        "dangling/f.flac",
        "ok/deep/x/f.flac",
    ] {
        let r = ingest_file(
            &src,
            &rel("in"),
            &music,
            &rel(dest),
            limits(MIB),
            &no_cancel(),
        );
        assert!(matches!(r, Err(IngestError::Symlink)), "{dest}: {r:?}");
    }
    assert!(tree(&outside).is_empty());
    assert!(tree(&music.join("real")).is_empty());
    assert!(!t.path().join("nowhere").exists());
    assert!(parts(&music).is_empty());
}

#[test]
fn source_symlinks_and_parent_symlinks_are_refused() {
    let t = tmp();
    let (src, music) = roots(&t);
    let secret = t.path().join("secret");
    fs::write(&secret, b"secret").unwrap();
    fs::create_dir(t.path().join("outdir")).unwrap();
    fs::write(t.path().join("outdir/f"), b"secret").unwrap();
    fs::write(src.join("real"), b"x").unwrap();
    symlink(&secret, src.join("abs")).unwrap();
    symlink("real", src.join("rel")).unwrap();
    symlink(t.path().join("outdir"), src.join("dir")).unwrap();
    for s in ["abs", "rel", "dir/f"] {
        let r = ingest_file(
            &src,
            &rel(s),
            &music,
            &rel("f.flac"),
            limits(MIB),
            &no_cancel(),
        );
        assert!(matches!(r, Err(IngestError::Symlink)), "{s}: {r:?}");
    }
    assert!(tree(&music).is_empty());
}

#[test]
fn source_must_be_a_regular_file() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::create_dir(src.join("adir")).unwrap();
    let fifo =
        std::ffi::CString::new(src.join("fifo").into_os_string().into_encoded_bytes()).unwrap();
    // SAFETY: valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    for s in ["adir", "fifo"] {
        // A FIFO with no writer would block a blocking open; this must return at once.
        let r = ingest_file(
            &src,
            &rel(s),
            &music,
            &rel("f.flac"),
            limits(MIB),
            &no_cancel(),
        );
        assert!(matches!(r, Err(IngestError::NotRegularFile)), "{s}: {r:?}");
    }
    let r = ingest_file(
        &src,
        &rel("missing"),
        &music,
        &rel("f.flac"),
        limits(MIB),
        &no_cancel(),
    );
    assert!(matches!(r, Err(IngestError::NotFound)), "{r:?}");
    assert!(tree(&music).is_empty());
}

#[test]
fn hard_linked_source_is_copied_not_linked() {
    // Policy: a hard link inside the download root is accepted (slskd owns that tree); the
    // copy is a new inode, so the library never shares an inode with the source.
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("a"), b"shared").unwrap();
    fs::hard_link(src.join("a"), src.join("b")).unwrap();
    ingest_file(
        &src,
        &rel("b"),
        &music,
        &rel("f.flac"),
        limits(MIB),
        &no_cancel(),
    )
    .unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(fs::metadata(music.join("f.flac")).unwrap().nlink(), 1);
    assert_eq!(fs::read(music.join("f.flac")).unwrap(), b"shared");
}

#[test]
fn byte_cap_is_exact_and_oversize_leaves_nothing() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), vec![7u8; 1000]).unwrap();
    ingest_file(
        &src,
        &rel("in"),
        &music,
        &rel("ok.flac"),
        limits(1000),
        &no_cancel(),
    )
    .unwrap();
    let r = ingest_file(
        &src,
        &rel("in"),
        &music,
        &rel("big.flac"),
        limits(999),
        &no_cancel(),
    );
    assert!(
        matches!(r, Err(IngestError::TooLarge { limit: 999 })),
        "{r:?}"
    );
    assert_eq!(tree(&music), ["ok.flac"]);
}

/// A source with no size metadata that is far larger than the cap (64 MiB, so a missing cap
/// fails the test instead of hanging it): the streaming cap must stop it.
struct Huge {
    left: u64,
}
impl Read for Huge {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.left as usize);
        buf[..n].fill(1);
        self.left -= n as u64;
        Ok(n)
    }
}

#[test]
fn streaming_cap_stops_an_oversize_stream() {
    let t = tmp();
    let music = t.path().join("music");
    fs::create_dir(&music).unwrap();
    let r = copy_into(
        &music,
        &rel("A/f.flac"),
        &mut Huge { left: 64 * MIB },
        limits(100_000),
        &no_cancel(),
    );
    assert!(
        matches!(r, Err(IngestError::TooLarge { limit: 100_000 })),
        "{r:?}"
    );
    assert_eq!(tree(&music), ["A"]);
}

#[test]
fn expected_size_mismatch_cleans_up_both_ways() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), vec![1u8; 500]).unwrap();
    for expected in [499, 501] {
        let r = ingest_file(
            &src,
            &rel("in"),
            &music,
            &rel("f.flac"),
            Limits {
                max_bytes: MIB,
                expected_size: Some(expected),
            },
            &no_cancel(),
        );
        assert!(
            matches!(r, Err(IngestError::SizeMismatch { expected: e, actual: 500 }) if e == expected),
            "{expected}: {r:?}"
        );
    }
    assert!(tree(&music).is_empty());
}

/// Reads `ok` bytes, then fails.
struct FailAfter {
    left: usize,
}
impl Read for FailAfter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::Error::other("disk on fire"));
        }
        let n = buf.len().min(self.left);
        buf[..n].fill(2);
        self.left -= n;
        Ok(n)
    }
}

#[test]
fn read_failure_removes_only_our_part() {
    let t = tmp();
    let music = t.path().join("music");
    fs::create_dir_all(music.join("A")).unwrap();
    // Someone else's leftovers that look like ours: never touched.
    fs::write(music.join("A/.aether-ingest-foreign.part"), b"keep").unwrap();
    fs::write(music.join("A/other.part"), b"keep").unwrap();
    let r = copy_into(
        &music,
        &rel("A/f.flac"),
        &mut FailAfter { left: 200_000 },
        limits(MIB),
        &no_cancel(),
    );
    assert!(matches!(r, Err(IngestError::Io(_))), "{r:?}");
    assert_eq!(
        tree(&music),
        ["A", "A/.aether-ingest-foreign.part", "A/other.part"]
    );
}

/// Sets the cancel flag after the first read.
struct CancelAfterFirst<'a>(&'a AtomicBool);
impl Read for CancelAfterFirst<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.store(true, Ordering::SeqCst);
        buf.fill(3);
        Ok(buf.len())
    }
}

#[test]
fn cancellation_mid_stream_removes_the_part() {
    let t = tmp();
    let music = t.path().join("music");
    fs::create_dir(&music).unwrap();
    let cancel = AtomicBool::new(false);
    let r = copy_into(
        &music,
        &rel("A/f.flac"),
        &mut CancelAfterFirst(&cancel),
        limits(10 * MIB),
        &cancel,
    );
    assert!(matches!(r, Err(IngestError::Cancelled)), "{r:?}");
    assert_eq!(tree(&music), ["A"]);
}

#[test]
fn concurrent_writers_exactly_one_wins() {
    let t = tmp();
    let (src, music) = roots(&t);
    const N: usize = 8;
    for i in 0..N {
        fs::write(src.join(format!("in{i}")), vec![i as u8; 64 * 1024]).unwrap();
    }
    let barrier = Arc::new(Barrier::new(N));
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let (src, music, barrier) = (src.clone(), music.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                ingest_file(
                    &src,
                    &rel(&format!("in{i}")),
                    &music,
                    &rel("A/B/same.flac"),
                    limits(MIB),
                    &no_cancel(),
                )
                .map(|_| i)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let winners: Vec<usize> = results
        .iter()
        .filter_map(|r| r.as_ref().ok().copied())
        .collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    for r in &results {
        assert!(
            matches!(r, Ok(_) | Err(IngestError::AlreadyExists)),
            "{r:?}"
        );
    }
    let out = fs::read(music.join("A/B/same.flac")).unwrap();
    assert_eq!(out, vec![winners[0] as u8; 64 * 1024]);
    assert_eq!(tree(&music), ["A", "A/B", "A/B/same.flac"]);
}

#[test]
fn roots_must_be_existing_absolute_directories_without_symlinks() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), b"x").unwrap();
    symlink(&music, t.path().join("music-link")).unwrap();
    fs::write(t.path().join("plain"), b"x").unwrap();
    let go = |dest_root: &Path| {
        ingest_file(
            &src,
            &rel("in"),
            dest_root,
            &rel("f.flac"),
            limits(MIB),
            &no_cancel(),
        )
    };
    assert!(matches!(
        go(&t.path().join("nope")),
        Err(IngestError::NotFound)
    ));
    assert!(matches!(
        go(&t.path().join("music-link")),
        Err(IngestError::Symlink)
    ));
    assert!(matches!(
        go(&t.path().join("plain")),
        Err(IngestError::InvalidRoot)
    ));
    assert!(matches!(
        go(Path::new("relative/music")),
        Err(IngestError::InvalidRoot)
    ));
    // A symlinked source root is refused the same way.
    symlink(&src, t.path().join("src-link")).unwrap();
    let r = ingest_file(
        &t.path().join("src-link"),
        &rel("in"),
        &music,
        &rel("f.flac"),
        limits(MIB),
        &no_cancel(),
    );
    assert!(matches!(r, Err(IngestError::Symlink)), "{r:?}");
    assert!(tree(&music).is_empty());
}

#[test]
fn intermediate_file_in_destination_path_is_refused() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), b"x").unwrap();
    fs::write(music.join("A"), b"file not dir").unwrap();
    let r = ingest_file(
        &src,
        &rel("in"),
        &music,
        &rel("A/f.flac"),
        limits(MIB),
        &no_cancel(),
    );
    assert!(matches!(r, Err(IngestError::NotADirectory)), "{r:?}");
    assert_eq!(fs::read(music.join("A")).unwrap(), b"file not dir");
}

#[test]
fn existing_destination_directories_are_reused_and_siblings_kept() {
    let t = tmp();
    let (src, music) = roots(&t);
    fs::write(src.join("in"), b"x").unwrap();
    fs::create_dir_all(music.join("A/B")).unwrap();
    fs::write(music.join("A/B/old.flac"), b"old").unwrap();
    ingest_file(
        &src,
        &rel("in"),
        &music,
        &rel("A/B/new.flac"),
        limits(MIB),
        &no_cancel(),
    )
    .unwrap();
    assert_eq!(tree(&music), ["A", "A/B", "A/B/new.flac", "A/B/old.flac"]);
    assert_eq!(fs::read(music.join("A/B/old.flac")).unwrap(), b"old");
}
