use anyhow::{Context, Result};
use which::which;

pub fn extract_rootfs(
    payload: &[u8],
    dest: &std::path::Path,
    unsquashfs: Option<&std::path::Path>,
) -> Result<()> {
    let payload_file = dest.join("payload.sqfs");
    std::fs::write(&payload_file, payload).context("failed to write payload")?;
    let unsquashfs_bin = match unsquashfs {
        Some(path) => path.to_path_buf(),
        None => which("unsquashfs")
            .ok()
            .context("failed to find unsquashfs in PATH")?,
    };
    let status = std::process::Command::new(unsquashfs_bin)
        .arg("-f")
        .arg("-d")
        .arg(dest)
        .arg(&payload_file)
        .status()
        .context("failed to execute unsquashfs")?;
    if !status.success() {
        anyhow::bail!("unsquashfs failed with status: {}", status);
    }
    std::fs::remove_file(&payload_file).context("failed to remove payload file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_fake_unsquashfs(dir: &std::path::Path, script_body: &str) -> std::path::PathBuf {
        let script = dir.join("fake-unsquashfs.sh");
        fs::write(&script, format!("#!/bin/sh\nset -eu\n{script_body}\n")).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    #[test]
    fn test_extract_rootfs_runs_unsquashfs_and_cleans_up_payload_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let script = write_fake_unsquashfs(
            tempdir.path(),
            "test \"$1\" = \"-f\"\n\
             test \"$2\" = \"-d\"\n\
             dest=\"$3\"\n\
             payload=\"$4\"\n\
             test -f \"$payload\"\n\
             printf '%s' extracted > \"$dest/rootfs.txt\"\n\
             exit 0",
        );

        extract_rootfs(b"payload-bytes", tempdir.path(), Some(&script)).unwrap();

        assert_eq!(
            fs::read(tempdir.path().join("rootfs.txt")).unwrap(),
            b"extracted"
        );
        assert!(!tempdir.path().join("payload.sqfs").exists());
    }

    #[test]
    fn test_extract_rootfs_returns_error_when_unsquashfs_fails() {
        let tempdir = tempfile::tempdir().unwrap();
        let script = write_fake_unsquashfs(tempdir.path(), "exit 9");

        let error = extract_rootfs(b"payload-bytes", tempdir.path(), Some(&script)).unwrap_err();

        assert!(format!("{error:#}").contains("unsquashfs failed with status"));
        assert!(tempdir.path().join("payload.sqfs").exists());
    }
}
