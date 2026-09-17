//! # `paths.rs` — Path sanitization and collision policy (PRD FR-2.8, FR-2.10,
//! SEC-1)
//!
//! This module is the security boundary for everything a remote peer sends us.
//! A received `rel_path` is attacker-controlled: it may contain `..`, absolute
//! paths, drive letters, NUL bytes, or Windows device names. Nothing here
//! trusts it.
//!
//! SEC-1 is called the single highest-severity risk in the design, and the
//! defence is deliberately layered: [`sanitize_rel_path`] rebuilds the path
//! from scratch out of individually-sanitized segments, and
//! [`ensure_within`] then re-checks the joined result before any file handle is
//! opened. Either alone would be sufficient in theory; both are present because
//! a path-traversal bug here writes arbitrary files anywhere the process can
//! reach.
//!
//! This is also the only module in the backend with unit tests, for the same
//! reason — see the `tests` module at the foot of the file.
//!
//! Project: LocalDrop — zero-configuration LAN file and text transfer
//! Author:  Emmanuel Paul <pauldukz@gmail.com>

use std::path::{Component, Path, PathBuf};

use crate::error::{TransferError, TransferResult};
use crate::settings::CollisionPolicy;

/// Windows reserved device names (FR-2.10). Matched case-insensitively against
/// the stem, so `CON.txt` is caught as well as `CON`.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters that are illegal in Windows filenames, plus the path separators
/// we split on ourselves.
const ILLEGAL_CHARS: [char; 9] = ['<', '>', ':', '"', '|', '?', '*', '/', '\\'];

/// Per-component byte limit on NTFS and most Unix filesystems.
const MAX_COMPONENT_BYTES: usize = 255;

/// Turn one attacker-supplied path segment into a safe filename.
///
/// Returns `None` if nothing survives sanitization (e.g. the segment was `..`
/// or entirely control characters).
fn sanitize_component(raw: &str) -> Option<String> {
    // Traversal segments are rejected outright rather than escaped. There is no
    // legitimate reason for a manifest to contain one, and neutralizing rather
    // than honouring them is what makes the rebuild in `sanitize_rel_path`
    // structurally unable to climb out of the download directory.
    if raw.is_empty() || raw == "." || raw == ".." {
        return None;
    }

    // One pass, two transformations:
    //   - control characters (including NUL) are dropped entirely, since a NUL
    //     can truncate a path at the OS boundary and make the name that gets
    //     written differ from the one that was checked;
    //   - characters illegal on Windows, plus both path separators, become
    //     underscores. The separators matter most: a segment containing one
    //     would silently reintroduce a directory level the caller did not
    //     account for in its depth counting.
    let mut cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if ILLEGAL_CHARS.contains(&c) { '_' } else { c })
        .collect();

    // Windows strips trailing dots and spaces, which would let `evil.` and
    // `evil` collide after the fact.
    //
    // A loop rather than a single trim because a name may end in several of
    // either, in any order (`"evil. . ."`), and all must go.
    while cleaned.ends_with('.') || cleaned.ends_with(' ') {
        cleaned.pop();
    }

    // Re-checked, because the filtering and trimming above can *produce* an
    // empty or traversal-shaped string from input that was neither — `".."`
    // written as `"..​"` with a control character, for instance.
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return None;
    }

    // FR-2.10 — reserved device names. Matched on the stem and uppercased,
    // because Windows treats `CON`, `con.txt` and `CoN.log` alike: all of them
    // address the console device rather than creating a file. Prefixed rather
    // than rejected, so the entry still transfers under a usable name.
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        cleaned = format!("_{cleaned}");
    }

    // Guard against pathological names blowing past filesystem limits.
    // 255 bytes is the per-component limit on NTFS and most Unix filesystems.
    //
    // Cut on a character boundary, not at byte 255. `String::truncate` panics
    // if the index falls inside a multi-byte character, and `rel_path` is
    // attacker-controlled — a 400-byte name of emoji would otherwise panic this
    // task from the wire. FR-2.11 requires emoji names to work, so this is
    // reachable from ordinary use as well as from a hostile sender.
    if cleaned.len() > MAX_COMPONENT_BYTES {
        // The largest boundary at or below the limit. A boundary always exists
        // at 0, so the search cannot fail; `unwrap_or(0)` satisfies the
        // compiler without assuming it.
        let cut = (0..=MAX_COMPONENT_BYTES)
            .rev()
            .find(|&i| cleaned.is_char_boundary(i))
            .unwrap_or(0);
        cleaned.truncate(cut);

        // Truncation can leave a trailing dot or space again, or empty the
        // string outright if the first character alone exceeded the limit.
        while cleaned.ends_with('.') || cleaned.ends_with(' ') {
            cleaned.pop();
        }
        if cleaned.is_empty() {
            return None;
        }
    }

    Some(cleaned)
}

/// Sanitize a single filename, e.g. one reported by Android's ContentResolver.
///
/// Provider-supplied names are not trusted any more than names off the wire.
pub fn sanitize_file_name(raw: &str) -> Option<String> {
    // `rsplit` takes the last segment, so a provider that hands back a full
    // path — or something path-shaped — yields only its filename. Both
    // separators, because the string's origin is not known to be host-native.
    sanitize_component(raw.rsplit(['/', '\\']).next().unwrap_or(raw))
}

/// SEC-1 — convert a received `rel_path` into a relative path that cannot
/// escape the download directory.
///
/// Splits on both separators regardless of host OS, because an Android sender
/// and a Windows receiver disagree about which one is a separator.
pub fn sanitize_rel_path(rel: &str) -> TransferResult<PathBuf> {
    // Bounded before any work: an empty path addresses nothing, and 4096 is
    // past any real path while keeping the loop below cheap on hostile input.
    if rel.is_empty() || rel.len() > 4096 {
        return Err(TransferError::MalformedPayload);
    }

    // Built up from nothing rather than filtered down. This is the crux of
    // SEC-1: because the result only ever grows by `push`ing individually
    // sanitized segments — never by adopting anything from the input verbatim —
    // it cannot acquire a root, a drive letter, or a parent reference. A bare
    // `C:` or a UNC fragment becomes an ordinary directory name.
    let mut out = PathBuf::new();
    let mut depth = 0usize;

    for segment in rel.split(['/', '\\']) {
        if segment.is_empty() {
            continue; // collapses `a//b` and leading `/`
        }
        match sanitize_component(segment) {
            Some(safe) => {
                out.push(safe);
                depth += 1;
            }
            None => {
                // `..` and friends are dropped rather than honored. If the
                // entire path reduces to nothing, we reject below.
                continue;
            }
        }
        // Depth cap, checked inside the loop so it trips before a deeply
        // nested path is fully built. Guards against a manifest designed to
        // exhaust `MAX_PATH` or the filesystem's own nesting limit.
        if depth > 64 {
            return Err(TransferError::MalformedPayload);
        }
    }

    // Everything was dropped as unsafe — e.g. `../../..`. Rejecting rather
    // than returning an empty path matters: an empty relative path joined to
    // the download directory *is* the download directory, so a caller would
    // then try to open the directory itself as a file.
    if out.as_os_str().is_empty() {
        return Err(TransferError::MalformedPayload);
    }
    Ok(out)
}

/// SEC-1 second gate — belt and braces.
///
/// `sanitize_rel_path` should already make escape impossible; this verifies the
/// final joined path still resolves inside `root` before any handle is opened.
/// Uses logical normalization because the target does not exist yet and so
/// cannot be canonicalized.
pub fn ensure_within(root: &Path, candidate: &Path) -> TransferResult<()> {
    // Both sides normalized, so the comparison is between two resolved forms.
    // Normalizing only the candidate would fail whenever the configured
    // download directory itself contained a `.` or `..` segment.
    let normalized = normalize(candidate);
    let root_normalized = normalize(root);

    // `starts_with` on `Path` compares whole components, not characters, so
    // `/home/user/Downloads-evil` does not match a root of
    // `/home/user/Downloads` — which a string prefix test would have allowed.
    if normalized.starts_with(&root_normalized) {
        Ok(())
    } else {
        Err(TransferError::MalformedPayload)
    }
}

/// Resolve `.` and `..` lexically without touching the filesystem.
///
/// Lexical rather than `canonicalize` because the target does not exist yet —
/// that is the whole point of checking before opening it. The trade-off is that
/// symlinks are not resolved, which is acceptable here: FR-2.3 skips symlinks
/// on the sending side, and the receiver only ever creates directories it has
/// itself sanitized.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            // `..` pops the previous component. `pop` on an empty path is a
            // no-op rather than an error, so a leading `..` cannot climb above
            // the start — it simply has nothing to remove.
            Component::ParentDir => {
                out.pop();
            }
            // `.` contributes nothing.
            Component::CurDir => {}
            // Normal components, the root, and Windows prefixes all pass
            // through unchanged. Grouped rather than enumerated because the
            // handling is identical and a new component kind should be kept,
            // not silently dropped.
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// FR-2.8 — apply the configured collision policy.
///
/// `Ok(None)` means "skip this entry".
pub fn resolve_collision(target: PathBuf, policy: CollisionPolicy) -> Option<PathBuf> {
    // The common case: no collision, so the policy is irrelevant. Checked first
    // so a normal transfer does no extra work.
    if !target.exists() {
        return Some(target);
    }

    // Selection over the three policies (FR-2.8).
    match policy {
        // Nothing to compute — the caller opens the path with truncation.
        CollisionPolicy::Overwrite => Some(target),
        // `None` is the caller's signal to skip the entry entirely; the
        // per-entry prelude byte (§6.3) is what makes that possible mid-stream.
        CollisionPolicy::Skip => None,
        CollisionPolicy::Rename => {
            // Split into directory, stem, and extension so the counter can be
            // inserted before the extension — `report (2).pdf`, not
            // `report.pdf (2)`, which would break file-type association.
            let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
            let stem = target
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                // A path with no stem should be impossible after sanitization,
                // but a literal beats an unwrap in a receive path.
                .unwrap_or_else(|| "file".into());
            let ext = target
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                // Extensionless files are entirely normal; no dot is appended.
                .unwrap_or_default();

            // Starts at 2, matching the convention users expect: the original
            // is implicitly "1". Bounded rather than unbounded, because each
            // iteration is a filesystem stat and a directory with thousands of
            // collisions would otherwise stall the transfer.
            for n in 2..10_000u32 {
                let candidate = parent.join(format!("{stem} ({n}){ext}"));
                if !candidate.exists() {
                    return Some(candidate);
                }
            }

            // 9,998 variants all taken. Treated as a skip rather than an error:
            // one impossible filename should not fail the whole batch.
            None
        }
    }
}

/// FR-4.4 — optional subfoldering by sender and/or file type.
pub fn organize_dir(
    root: &Path,
    sender_alias: &str,
    file_name: &Path,
    by_sender: bool,
    by_type: bool,
) -> PathBuf {
    let mut dir = root.to_path_buf();

    // Two independent, composable options — both on yields
    // `root/Galaxy S24/PNG/`. Order is fixed: sender outside type, because a
    // user browsing by device is the more common intent.
    if by_sender {
        // The alias comes from the peer's heartbeat, so it is remote input and
        // gets the same sanitization as a filename. An alias of entirely
        // illegal characters falls back to a literal rather than vanishing,
        // which would silently write into the root instead.
        let safe = sanitize_component(sender_alias).unwrap_or_else(|| "Unknown".into());
        dir.push(safe);
    }

    if by_type {
        // Uppercased so `.PNG` and `.png` share one folder. The `filter` is
        // needed as well as the `map`: a name ending in a bare dot yields an
        // empty extension, and pushing "" would silently do nothing, putting
        // the file one level above where the user expects it.
        let ext = file_name
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| "OTHER".into());
        dir.push(ext);
    }
    dir
}

/// §7.1 / §6.6 — Windows `MAX_PATH` escape hatch.
///
/// Only applied to absolute paths; the prefix is meaningless otherwise, and
/// applying it to a relative path would produce an invalid path.
#[cfg(windows)]
pub fn extended(path: &Path) -> PathBuf {
    let s = path.as_os_str().to_string_lossy();

    // Two reasons to leave the path alone: it already carries the prefix
    // (applying it twice produces an invalid path), or it is relative (the
    // prefix requires a fully-qualified path and would corrupt a relative one).
    if s.starts_with("\\\\?\\") || !path.is_absolute() {
        return path.to_path_buf();
    }

    // UNC paths need a different form of the prefix, not just the prefix. This
    // arm is reachable because the download directory can be a network share.
    if s.starts_with("\\\\") {
        // UNC share: \\server\share -> \\?\UNC\server\share
        // `&s[1..]` drops one of the two leading backslashes, since `UNC` takes
        // its place in the extended form.
        PathBuf::from(format!("\\\\?\\UNC{}", &s[1..]))
    } else {
        PathBuf::from(format!("\\\\?\\{s}"))
    }
}

/// No-op off Windows: every other platform this could target has no
/// `MAX_PATH` equivalent, so callers apply `extended` unconditionally.
#[cfg(not(windows))]
pub fn extended(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// Convert an Android SAF *tree* URI into a filesystem path, when one exists.
///
/// Android's folder picker returns e.g.
/// `content://com.android.externalstorage.documents/tree/primary%3ADownload%2FKit`
/// which is not a path and cannot be walked or written with ordinary file APIs.
/// The `externalstorage` provider encodes `<volume>:<relative path>`, so for
/// volumes we can actually reach we can recover a real path:
///
/// - `primary:Download/Kit` -> `/storage/emulated/0/Download/Kit`
/// - `1A2B-3C4D:Backup`     -> `/storage/1A2B-3C4D/Backup`
///
/// Returns `None` for providers that are not backed by a filesystem at all
/// (Drive, Dropbox, MediaStore document ids). Callers must treat that as "not
/// supported" rather than guessing, and must still verify writability: this
/// resolves a path, it does not prove access.
pub fn content_tree_to_path(uri: &str) -> Option<PathBuf> {
    content_uri_to_path(uri, "tree/")
}

/// The same mapping for a *document* URI — a single picked file.
///
/// This matters for more than convenience: a resolved path carries the real
/// filename, whereas an opaque provider id does not, and a file-transfer app
/// that renames everything to `shared-1763...` is not doing its job. Files
/// picked from device storage resolve here; the media and downloads providers
/// use opaque ids and return `None`.
pub fn content_document_to_path(uri: &str) -> Option<PathBuf> {
    content_uri_to_path(uri, "document/")
}

/// Shared implementation for the tree and document URI mappings.
///
/// Written as a chain of `?` early-returns: every step can legitimately fail on
/// a URI from a provider that is not filesystem-backed, and the caller's
/// contract is a single `None` for all of them.
fn content_uri_to_path(uri: &str, segment: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("content://")?;
    let (authority, tail) = rest.split_once('/')?;

    // Only the platform's external-storage provider maps onto real files.
    // Checked by exact authority rather than by pattern: Drive, Dropbox and
    // MediaStore all speak this URI shape but are not backed by paths, and
    // guessing at one would produce a path that silently fails to open.
    if authority != "com.android.externalstorage.documents" {
        return None;
    }

    // A tree URI may be followed by a document segment; take the first match.
    // `skip_while` walks to the requested marker, `nth(1)` then takes the
    // segment immediately after it — the encoded `<volume>:<path>` pair.
    let encoded = tail
        .split('/')
        .skip_while(|s| *s != segment.trim_end_matches('/'))
        .nth(1)?;
    let decoded = percent_decode(encoded);
    let (volume, relative) = decoded.split_once(':')?;

    // Two volume shapes, and they mount in different places — the emulated
    // primary volume versus a removable card under its UUID.
    let root = if volume == "primary" {
        PathBuf::from("/storage/emulated/0")
    } else {
        // Removable volumes are mounted under their UUID.
        PathBuf::from("/storage").join(volume)
    };

    // The volume root itself, picked with no subdirectory. Returned directly
    // because `sanitize_rel_path` rejects an empty path by design.
    if relative.is_empty() {
        return Some(root);
    }

    // The relative part is attacker-influenced only in the sense that the user
    // picked it, but reuse the same hardening as received paths.
    let safe = sanitize_rel_path(relative).ok()?;
    Some(root.join(safe))
}

/// Minimal percent-decoding for SAF URIs, which only escape ASCII.
///
/// Hand-rolled rather than pulling in a URL-decoding crate: the input is
/// Android's own encoding of `<volume>:<path>`, so only `%3A` and `%2F` occur
/// in practice.
///
/// Manual index arithmetic rather than a `for` loop, because a `%XX` escape
/// consumes three bytes and the cursor has to advance by three.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        // `i + 2 < bytes.len()` keeps the slice below in bounds, so a truncated
        // escape at the end of the string is copied literally rather than
        // panicking.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            // A `%` not followed by two hex digits is a literal `%` — the
            // `if let` falls through to the byte-copy below rather than
            // discarding it.
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    // Lossy, because decoded bytes are not guaranteed to be valid UTF-8 and a
    // replacement character in a path is better than failing the pick.
    String::from_utf8_lossy(&out).into_owned()
}

/// The `.part` staging name for an in-flight file (FR-2.4).
///
/// Appended to the whole filename rather than replacing the extension, so
/// `report.pdf` becomes `report.pdf.part`. Using `with_extension` would yield
/// `report.part` and lose the original extension when the file is renamed back.
///
/// Note the `.part` file is staged *beside* its destination, which is AND-7's
/// outstanding gap on Android — this function is the one place to change.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

/// SEC-1's test suite — the only unit tests in the backend, because this is the
/// only module where a logic error grants a remote peer arbitrary file writes.
///
/// Each test is a table of hostile inputs asserted against one property, so a
/// regression names the specific class of attack that got through rather than
/// just "sanitization failed".
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        // Every one of these must land inside the root, not above it.
        for evil in [
            "../../Windows/System32/evil.dll",
            "..\\..\\evil.exe",
            "/etc/passwd",
            "C:\\Windows\\evil.dll",
            "a/../../../b.txt",
        ] {
            let safe = sanitize_rel_path(evil).expect("should sanitize, not error");
            assert!(
                !safe.to_string_lossy().contains(".."),
                "{evil} leaked `..`: {safe:?}"
            );
            assert!(!safe.is_absolute(), "{evil} stayed absolute: {safe:?}");
            let root = Path::new("/downloads");
            assert!(ensure_within(root, &root.join(&safe)).is_ok());
        }
    }

    #[test]
    fn rejects_paths_that_reduce_to_nothing() {
        assert!(sanitize_rel_path("..").is_err());
        assert!(sanitize_rel_path("../..").is_err());
        assert!(sanitize_rel_path("").is_err());
    }

    #[test]
    fn neutralizes_reserved_names() {
        assert_eq!(sanitize_component("CON").unwrap(), "_CON");
        assert_eq!(sanitize_component("con.txt").unwrap(), "_con.txt");
        assert_eq!(sanitize_component("LPT9.log").unwrap(), "_LPT9.log");
        assert_eq!(sanitize_component("console.txt").unwrap(), "console.txt");
    }

    #[test]
    fn strips_control_chars_and_trailing_dots() {
        assert_eq!(sanitize_component("ev\u{0}il.txt").unwrap(), "evil.txt");
        assert_eq!(sanitize_component("report...").unwrap(), "report");
    }

    #[test]
    fn preserves_unicode_names() {
        // FR-2.11 — emoji and non-ASCII names round-trip unchanged.
        assert_eq!(sanitize_component("café 🎉.png").unwrap(), "café 🎉.png");
        let p = sanitize_rel_path("Fotos/año/niño.jpg").unwrap();
        assert_eq!(p, PathBuf::from("Fotos").join("año").join("niño.jpg"));
    }

    #[test]
    fn truncates_long_names_on_a_char_boundary() {
        // Regression: `String::truncate(255)` panics when byte 255 falls inside
        // a multi-byte character. `rel_path` comes off the wire, so this was
        // reachable from a remote peer — and from any genuinely long emoji
        // filename, which FR-2.11 requires to work.
        let long_emoji = "🎉".repeat(100); // 400 bytes, no boundary at 255
        let cleaned = sanitize_component(&long_emoji).expect("must not panic or vanish");
        assert!(cleaned.len() <= 255);
        // Cut cleanly: 63 × 4 = 252 bytes is the last boundary at or below 255.
        assert_eq!(cleaned.chars().count(), 63);
        assert!(cleaned.chars().all(|c| c == '🎉'));

        // A long ASCII name still fills the limit exactly.
        let long_ascii = "a".repeat(300);
        assert_eq!(sanitize_component(&long_ascii).unwrap().len(), 255);

        // Truncation must not leave a trailing dot, which Windows would strip
        // and thereby reintroduce a collision.
        let dotted = format!("{}...", "b".repeat(260));
        let cleaned = sanitize_component(&dotted).unwrap();
        assert!(!cleaned.ends_with('.'));
    }

    #[test]
    fn resolves_saf_tree_uris() {
        assert_eq!(
            content_tree_to_path(
                "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FKit"
            ),
            Some(
                PathBuf::from("/storage/emulated/0")
                    .join("Download")
                    .join("Kit")
            )
        );
        assert_eq!(
            content_tree_to_path(
                "content://com.android.externalstorage.documents/tree/1A2B-3C4D%3ABackup"
            ),
            Some(PathBuf::from("/storage").join("1A2B-3C4D").join("Backup"))
        );
        // Whole-volume grant.
        assert_eq!(
            content_tree_to_path("content://com.android.externalstorage.documents/tree/primary%3A"),
            Some(PathBuf::from("/storage/emulated/0"))
        );
    }

    #[test]
    fn rejects_non_filesystem_providers() {
        // Cloud and media providers have no path; callers must not guess one.
        for uri in [
            "content://com.google.android.apps.docs.storage/tree/abc",
            "content://com.android.providers.media.documents/document/image%3A1234",
            "content://com.android.providers.downloads.documents/tree/msf%3A99",
            "not-a-uri",
        ] {
            assert_eq!(content_tree_to_path(uri), None, "{uri} should not resolve");
        }
    }

    #[test]
    fn ensure_within_catches_escapes() {
        let root = Path::new("/downloads");
        assert!(ensure_within(root, Path::new("/downloads/a/b.txt")).is_ok());
        assert!(ensure_within(root, Path::new("/downloads/../etc/passwd")).is_err());
        assert!(ensure_within(root, Path::new("/etc/passwd")).is_err());
    }
}
