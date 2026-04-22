use anyhow::{Context, Result};
use std::path::Path;
use which::which;

/// Make the squashfs payload available at `dest`.
///
/// First tries a direct kernel `mount -t squashfs -o loop,ro` (works when
/// the caller is root — e.g. inside a container host or after `sudo`).
/// Falls back to unsquashfs extraction otherwise. Returns `true` if the
/// squashfs was mounted (rootfs is read-only), `false` if extracted.
///
/// On mount: `sqfs_path` is kept (the kernel holds a reference to it).
/// On extraction: `sqfs_path` is removed after successful extraction.
pub fn mount_or_extract(
    payload: &[u8],
    sqfs_path: &Path,
    dest: &Path,
    unsquashfs: Option<&Path>,
) -> Result<bool> {
    std::fs::write(sqfs_path, payload).context("failed to write squashfs payload")?;
    std::fs::create_dir_all(dest).context("failed to create rootfs mountpoint")?;

    if try_squashfs_mount(sqfs_path, dest).is_ok() {
        return Ok(true);
    }

    extract_via_unsquashfs(sqfs_path, dest, unsquashfs)?;
    let _ = std::fs::remove_file(sqfs_path);
    Ok(false)
}

/// Attempt a kernel squashfs loop mount. Requires the caller to be root.
pub fn try_squashfs_mount(sqfs_path: &Path, dest: &Path) -> Result<()> {
    let output = std::process::Command::new("mount")
        .arg("-t")
        .arg("squashfs")
        .arg("-o")
        .arg("loop,ro")
        .arg(sqfs_path)
        .arg(dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("failed to run mount")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            anyhow::bail!("mount failed with status: {}", output.status);
        } else {
            anyhow::bail!("mount failed ({}): {}", output.status, stderr);
        }
    }
    Ok(())
}

/// Return `true` if `path` is currently listed as a squashfs mount in
/// `/proc/mounts`.
pub fn is_squashfs_mounted(path: &Path) -> bool {
    let Ok(data) = std::fs::read_to_string("/proc/mounts") else {
        return false;
    };
    let path_str = path.to_string_lossy();
    data.lines().any(|line| {
        let mut parts = line.splitn(4, ' ');
        let _dev = parts.next();
        let mountpoint = parts.next().unwrap_or("");
        let fstype = parts.next().unwrap_or("");
        mountpoint == path_str.as_ref() && fstype == "squashfs"
    })
}

fn extract_via_unsquashfs(sqfs_path: &Path, dest: &Path, unsquashfs: Option<&Path>) -> Result<()> {
    let bin = match unsquashfs {
        Some(p) => p.to_path_buf(),
        None => which("unsquashfs")
            .ok()
            .context("failed to find unsquashfs in PATH")?,
    };
    let status = std::process::Command::new(bin)
        .arg("-f")
        .arg("-d")
        .arg(dest)
        .arg(sqfs_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to execute unsquashfs")?;
    if !status.success() {
        anyhow::bail!("unsquashfs failed with status: {}", status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fake_unsquashfs(dir: &Path, script_body: &str) -> std::path::PathBuf {
        let script = dir.join("fake-unsquashfs.sh");
        fs::write(&script, format!("#!/bin/sh\nset -eu\n{script_body}\n")).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    #[test]
    fn test_extraction_fallback_populates_dest_and_removes_sqfs() {
        let tmp = tempfile::tempdir().unwrap();
        let sqfs_path = tmp.path().join("rootfs.sqfs");
        let dest = tmp.path().join("rootfs");
        let script = fake_unsquashfs(
            tmp.path(),
            "dest=\"$3\"; payload=\"$4\"\n\
             test -f \"$payload\"\n\
             printf '%s' extracted > \"$dest/rootfs.txt\"\n\
             exit 0",
        );

        let mounted = mount_or_extract(b"data", &sqfs_path, &dest, Some(&script)).unwrap();

        assert!(!mounted);
        assert_eq!(fs::read(dest.join("rootfs.txt")).unwrap(), b"extracted");
        assert!(!sqfs_path.exists());
    }

    #[test]
    fn test_extraction_error_propagates() {
        let tmp = tempfile::tempdir().unwrap();
        let sqfs_path = tmp.path().join("rootfs.sqfs");
        let dest = tmp.path().join("rootfs");
        let script = fake_unsquashfs(tmp.path(), "exit 9");

        let err = mount_or_extract(b"data", &sqfs_path, &dest, Some(&script)).unwrap_err();
        assert!(format!("{err:#}").contains("unsquashfs failed with status"));
    }
}
