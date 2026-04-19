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
        None => which("unsquashfs").ok().context("failed to find unsquashfs in PATH")?,
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
