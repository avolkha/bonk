use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Safely joins `rootfs` with a path from a tar entry, normalizing away any
/// `..`, absolute, or prefix components that would escape the rootfs.
/// Returns `None` if the path contains any such components.
fn safe_join(rootfs: &Path, tar_path: &Path) -> Option<PathBuf> {
    let mut result = rootfs.to_path_buf();
    for component in tar_path.components() {
        match component {
            Component::Normal(c) => result.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(result)
}

/// Verifies that `target` is strictly inside `rootfs` after resolving symlinks.
/// Returns `Ok(canonical)` if safe, or `Err` if the path escapes or can't be
/// resolved. Both `rootfs` and `target` must exist when this is called.
fn verify_inside_rootfs(rootfs: &Path, target: &Path) -> Result<PathBuf> {
    let canonical_root = std::fs::canonicalize(rootfs).context("failed to canonicalize rootfs")?;
    let canonical_target =
        std::fs::canonicalize(target).context("failed to canonicalize target")?;
    if canonical_target.starts_with(&canonical_root) {
        Ok(canonical_target)
    } else {
        anyhow::bail!(
            "path escapes rootfs: {} resolves to {}",
            target.display(),
            canonical_target.display()
        )
    }
}

fn determine_file_format(path: &Path) -> Result<&'static str> {
    let mut file =
        std::fs::File::open(path).context("failed to open layer file for format detection")?;
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf)
        .context("failed to read layer file")?;
    match &buf {
        [0x1F, 0x8B] => Ok("gzip"),
        [0x28, 0xB5] => Ok("zstd"),
        _ => Ok("raw"),
    }
}

fn open_layer(path: &Path) -> Result<Box<dyn Read>> {
    let file = File::open(path).context("failed to open layer file")?;
    let file_format = determine_file_format(path)?;
    let reader: Box<dyn Read> = match file_format {
        "gzip" => Box::new(GzDecoder::new(file)),
        "zstd" => Box::new(zstd::Decoder::new(file).context("failed to create zstd decoder")?),
        "raw" => Box::new(file),
        _ => anyhow::bail!("unsupported layer file format: {}", file_format),
    };
    Ok(reader)
}

/// Handles an opaque whiteout (`.wh..wh..opq`): removes all contents of the
/// corresponding directory in `rootfs` but keeps the directory itself, so that
/// new entries in the same layer can still be placed inside it.
fn apply_opaque_whiteout(rootfs: &Path, whiteout_path: &Path) -> Result<()> {
    let parent = whiteout_path.parent().unwrap_or_else(|| Path::new(""));
    let dir = match safe_join(rootfs, parent) {
        Some(p) => p,
        None => {
            eprintln!(
                "warning: skipping unsafe opaque whiteout path: {}",
                whiteout_path.display()
            );
            return Ok(());
        }
    };
    if dir.exists() {
        // Verify the directory itself is inside rootfs (could follow a symlink)
        if let Err(e) = verify_inside_rootfs(rootfs, &dir) {
            eprintln!(
                "warning: skipping opaque whiteout ({}): {}",
                whiteout_path.display(),
                e
            );
            return Ok(());
        }
        for entry in std::fs::read_dir(&dir).context("failed to read dir for opaque whiteout")? {
            let entry = entry.context("failed to read dir entry for opaque whiteout")?;
            let child = entry.path();
            // Each child is already under a verified dir, but double-check
            // in case of symlink tricks within the directory
            if verify_inside_rootfs(rootfs, &child).is_err() {
                eprintln!(
                    "warning: skipping opaque whiteout child outside rootfs: {}",
                    child.display()
                );
                continue;
            }
            if child.is_dir() {
                std::fs::remove_dir_all(&child)
                    .context("failed to remove subdir in opaque whiteout")?;
            } else {
                std::fs::remove_file(&child).context("failed to remove file in opaque whiteout")?;
            }
        }
    }
    Ok(())
}

/// Handles a regular whiteout (`.wh.<name>`): removes the named file or
/// directory from `rootfs`.
fn apply_regular_whiteout(rootfs: &Path, whiteout_path: &Path) -> Result<()> {
    let parent = whiteout_path.parent().unwrap_or_else(|| Path::new(""));
    let filename = whiteout_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let real_name = filename.strip_prefix(".wh.").unwrap_or(filename);
    // Reject if real_name is ".." or empty (prevents escaping via .wh... entries)
    if real_name == ".." || real_name == "." || real_name.is_empty() {
        eprintln!(
            "warning: skipping whiteout with invalid name: {}",
            whiteout_path.display()
        );
        return Ok(());
    }
    let base = match safe_join(rootfs, parent) {
        Some(p) => p,
        None => {
            eprintln!(
                "warning: skipping unsafe whiteout path: {}",
                whiteout_path.display()
            );
            return Ok(());
        }
    };
    let target = base.join(real_name);
    if target.exists() {
        // Verify the resolved path is inside rootfs (catches symlink escapes)
        if let Err(e) = verify_inside_rootfs(rootfs, &target) {
            eprintln!(
                "warning: skipping whiteout ({}): {}",
                whiteout_path.display(),
                e
            );
            return Ok(());
        }
        if target.is_dir() {
            std::fs::remove_dir_all(&target).context("failed to remove directory for whiteout")?;
        } else {
            std::fs::remove_file(&target).context("failed to remove file for whiteout")?;
        }
    }
    Ok(())
}

fn apply_layer(layer_tar: Box<dyn Read>, rootfs: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(layer_tar);
    for entry in archive
        .entries()
        .context("failed to read layer tar entries")?
    {
        let mut entry = entry.context("failed to read layer tar entry")?;
        let path = entry
            .path()
            .context("failed to get layer tar entry path")?
            .into_owned();
        let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if filename == ".wh..wh..opq" {
            apply_opaque_whiteout(rootfs, &path)?;
        } else if filename.starts_with(".wh.") {
            apply_regular_whiteout(rootfs, &path)?;
        } else {
            if let Err(e) = entry.unpack_in(rootfs) {
                eprintln!("warning: failed to extract {}: {}", path.display(), e);
            }
        }
    }
    Ok(())
}

pub fn flatten_layers(layers: &[PathBuf], rootfs: &Path) -> Result<()> {
    std::fs::create_dir_all(rootfs).context("failed to create rootfs directory")?;
    for layer in layers {
        let reader = open_layer(layer)?;
        apply_layer(reader, rootfs)
            .context(format!("failed to apply layer: {}", layer.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── whiteout helpers ────────────────────────────────────────────────────

    #[test]
    fn test_regular_whiteout_dotdot_injection_is_rejected() {
        // A tar entry named ".wh..." strips to ".." — must not delete parent of rootfs
        let outer = tempfile::TempDir::new().unwrap();
        let rootfs = outer.path().join("rootfs");
        std::fs::create_dir_all(&rootfs).unwrap();
        let sentinel = outer.path().join("important");
        std::fs::write(&sentinel, b"keep me").unwrap();

        apply_regular_whiteout(&rootfs, Path::new(".wh...")).unwrap();

        assert!(
            sentinel.exists(),
            ".. injection must not delete files outside rootfs"
        );
    }

    #[test]
    fn test_regular_whiteout_symlink_escape_is_rejected() {
        let rootfs = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let victim = outside.path().join("secret");
        std::fs::write(&victim, b"top secret").unwrap();

        // Create a symlink inside rootfs that points outside
        let link = rootfs.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        // Whiteout trying to delete through the symlink
        apply_regular_whiteout(rootfs.path(), Path::new("escape/.wh.secret")).unwrap();

        assert!(
            victim.exists(),
            "symlink escape must not delete files outside rootfs"
        );
    }

    #[test]
    fn test_opaque_whiteout_symlink_escape_is_rejected() {
        let rootfs = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let victim = outside.path().join("important");
        std::fs::write(&victim, b"data").unwrap();

        // Symlink inside rootfs pointing outside
        let link = rootfs.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        apply_opaque_whiteout(rootfs.path(), Path::new("escape/.wh..wh..opq")).unwrap();

        assert!(
            victim.exists(),
            "symlink-based opaque whiteout must not delete outside rootfs"
        );
    }

    #[test]
    fn test_regular_whiteout_path_traversal_is_rejected() {
        let rootfs = tempfile::TempDir::new().unwrap();
        // Create a sentinel file outside rootfs to verify it is NOT touched
        let outside = tempfile::TempDir::new().unwrap();
        let victim = outside.path().join("sensitive");
        std::fs::write(&victim, b"secret").unwrap();

        // A crafted tar path that tries to escape via ..
        apply_regular_whiteout(rootfs.path(), Path::new("../../sensitive")).unwrap();

        assert!(
            victim.exists(),
            "path traversal should not delete files outside rootfs"
        );
    }

    #[test]
    fn test_opaque_whiteout_path_traversal_is_rejected() {
        let rootfs = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let victim = outside.path().join("sensitive");
        std::fs::write(&victim, b"secret").unwrap();

        apply_opaque_whiteout(rootfs.path(), Path::new("../../.wh..wh..opq")).unwrap();

        assert!(
            victim.exists(),
            "path traversal should not affect files outside rootfs"
        );
    }

    #[test]
    fn test_regular_whiteout_removes_file() {
        let rootfs = tempfile::TempDir::new().unwrap();
        let target = rootfs.path().join("etc/passwd");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"root:x:0:0").unwrap();

        apply_regular_whiteout(rootfs.path(), Path::new("etc/.wh.passwd")).unwrap();

        assert!(!target.exists(), "file should have been removed");
    }

    #[test]
    fn test_regular_whiteout_removes_directory() {
        let rootfs = tempfile::TempDir::new().unwrap();
        let target = rootfs.path().join("var/cache/apt");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("lists"), b"data").unwrap();

        apply_regular_whiteout(rootfs.path(), Path::new("var/cache/.wh.apt")).unwrap();

        assert!(!target.exists(), "directory should have been removed");
    }

    #[test]
    fn test_regular_whiteout_nonexistent_target_is_noop() {
        let rootfs = tempfile::TempDir::new().unwrap();
        // no file created — should succeed silently
        apply_regular_whiteout(rootfs.path(), Path::new("etc/.wh.shadow")).unwrap();
    }

    #[test]
    fn test_opaque_whiteout_clears_directory_contents() {
        let rootfs = tempfile::TempDir::new().unwrap();
        let dir = rootfs.path().join("var/cache");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file1.txt"), b"a").unwrap();
        std::fs::write(dir.join("file2.txt"), b"b").unwrap();
        std::fs::create_dir(dir.join("subdir")).unwrap();

        apply_opaque_whiteout(rootfs.path(), Path::new("var/cache/.wh..wh..opq")).unwrap();

        // Directory itself must still exist
        assert!(dir.exists(), "directory itself should be kept");
        // But all contents removed
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "directory should be empty"
        );
    }

    #[test]
    fn test_opaque_whiteout_nonexistent_dir_is_noop() {
        let rootfs = tempfile::TempDir::new().unwrap();
        apply_opaque_whiteout(rootfs.path(), Path::new("no/such/.wh..wh..opq")).unwrap();
    }

    #[test]
    fn test_opaque_whiteout_root_level() {
        // .wh..wh..opq at the root of the layer — parent is ""
        let rootfs = tempfile::TempDir::new().unwrap();
        std::fs::write(rootfs.path().join("leftover"), b"x").unwrap();

        apply_opaque_whiteout(rootfs.path(), Path::new(".wh..wh..opq")).unwrap();

        assert_eq!(std::fs::read_dir(rootfs.path()).unwrap().count(), 0);
    }

    // ── format detection / open_layer ───────────────────────────────────────

    fn tmp_file_with_bytes(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f
    }

    #[test]
    fn test_determine_file_format_gzip() {
        let f = tmp_file_with_bytes(&[0x1F, 0x8B, 0x00]);
        assert_eq!(determine_file_format(f.path()).unwrap(), "gzip");
    }

    #[test]
    fn test_determine_file_format_zstd() {
        let f = tmp_file_with_bytes(&[0x28, 0xB5, 0x00]);
        assert_eq!(determine_file_format(f.path()).unwrap(), "zstd");
    }

    #[test]
    fn test_determine_file_format_raw() {
        let f = tmp_file_with_bytes(&[0x00, 0x00, 0x00]);
        assert_eq!(determine_file_format(f.path()).unwrap(), "raw");
    }

    #[test]
    fn test_determine_file_format_too_short() {
        let f = tmp_file_with_bytes(&[0x1F]);
        assert!(determine_file_format(f.path()).is_err());
    }

    #[test]
    fn test_open_layer_raw() {
        let f = tmp_file_with_bytes(b"hello raw");
        let mut reader = open_layer(f.path()).unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello raw");
    }

    #[test]
    fn test_open_layer_gzip() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(b"hello gzip").unwrap();
        let compressed = enc.finish().unwrap();
        let f = tmp_file_with_bytes(&compressed);
        let mut reader = open_layer(f.path()).unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello gzip");
    }

    #[test]
    fn test_open_layer_zstd() {
        let compressed = zstd::encode_all(b"hello zstd" as &[u8], 0).unwrap();
        let f = tmp_file_with_bytes(&compressed);
        let mut reader = open_layer(f.path()).unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello zstd");
    }
}
