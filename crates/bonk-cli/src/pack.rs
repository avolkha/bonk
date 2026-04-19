use anyhow::{bail, Context, Result};
use bonk_common::{Footer, FOOTER_SIZE};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use which::which;

pub fn build_squashfs(rootfs: &Path) -> Result<Vec<u8>> {
    let sqfs_path = rootfs.with_extension("sqfs");
    let _ = std::fs::remove_file(&sqfs_path);
    let status = std::process::Command::new("mksquashfs")
        .arg(rootfs)
        .arg(&sqfs_path)
        .arg("-comp")
        .arg("zstd")
        .arg("-noappend")
        .arg("-quiet")
        .status()
        .context("failed to run mksquashfs — is squashfs-tools installed?")?;
    if !status.success() {
        bail!("mksquashfs failed with exit code {:?}", status.code());
    }
    let bytes = std::fs::read(&sqfs_path)?;
    let _ = std::fs::remove_file(&sqfs_path);
    Ok(bytes)
}

pub fn assemble(
    output: &str,
    payload: &[u8],
    config: &bonk_common::ContainerConfig,
    bwrap_path: Option<&Path>,
    unsquashfs_path: Option<&Path>,
) -> Result<usize> {
    let runner = get_runner_bytes(None)?;
    let bwrap = get_tool_bytes("bwrap", bwrap_path)?;
    let unsquashfs = get_tool_bytes("unsquashfs", unsquashfs_path)?;
    let config_json =
        serde_json::to_vec(config).context("failed to serialize container config")?;

    let mut output_file = std::fs::File::create(output).context("failed to create output file")?;
    let footer = write_sections(
        &mut output_file,
        &runner,
        payload,
        &bwrap,
        &unsquashfs,
        &config_json,
    )?;
    output_file
        .write_all(&footer.to_bytes())
        .context("failed to write footer to output")?;

    output_file
        .set_permissions(std::fs::Permissions::from_mode(0o755))
        .context("failed to set output file permissions")?;

    Ok(footer.payload_offset as usize          // runner
        + footer.payload_size as usize
        + footer.bwrap_size as usize
        + footer.unsquashfs_size as usize
        + footer.config_size as usize
        + FOOTER_SIZE)
}

/// Write the five payload sections to `file` in canonical order and return
/// the populated [`Footer`]. The write order and footer fields are defined
/// in one place so they cannot diverge.
fn write_sections(
    file: &mut std::fs::File,
    runner: &[u8],
    payload: &[u8],
    bwrap: &[u8],
    unsquashfs: &[u8],
    config: &[u8],
) -> Result<Footer> {
    let mut write = |data: &[u8], label: &str| -> Result<u64> {
        file.write_all(data)
            .with_context(|| format!("failed to write {} to output", label))?;
        Ok(data.len() as u64)
    };

    // LAYOUT CONTRACT: the write order below must match the field order in
    // Footer (bonk-common/src/lib.rs). Each field stores the size of the
    // section written immediately before it. Reordering writes here without
    // updating Footer — or vice versa — will corrupt every binary bonk produces.
    let payload_offset    = write(runner,     "runner")?;
    let payload_size      = write(payload,    "payload")?;
    let bwrap_size        = write(bwrap,      "bwrap")?;
    let unsquashfs_size   = write(unsquashfs, "unsquashfs")?;
    let config_size       = write(config,     "config")?;

    Ok(Footer { payload_offset, payload_size, bwrap_size, unsquashfs_size, config_size })
}

/// Resolve the filesystem path of a tool binary by name.
///
/// Search order:
///   0. Explicit path provided via CLI flag
///   1. BONK_TOOLS_DIR environment variable
///   2. Plain sibling of the bonk binary (e.g. bonk-runner next to bonk)
///   3. tools/<arch>/ next to the bonk binary
///   4. tools/ next to the bonk binary (flat)
///   5. System PATH (via `which`)
pub fn get_tool_path(name: &str, override_path: Option<&Path>) -> Result<PathBuf> {
    // 0. Explicit CLI flag overrides everything
    if let Some(path) = override_path {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        bail!("--{}-path: file not found: {}", name, path.display());
    }

    let arch = std::env::consts::ARCH;

    // 1. Env var
    if let Ok(dir) = std::env::var("BONK_TOOLS_DIR") {
        let path = Path::new(&dir).join(name);
        if path.exists() {
            return Ok(path);
        }
    }

    // 2. Plain sibling of the bonk binary (covers target/debug/bonk-runner etc.)
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().unwrap_or(Path::new(".")).join(name);
        if sibling.exists() {
            return Ok(sibling);
        }
    }

    // 3&4. tools/ subdirectory next to current executable
    if let Ok(exe) = std::env::current_exe() {
        let exe_dir = exe.parent().unwrap_or(Path::new("."));

        let arch_path = exe_dir.join("tools").join(arch).join(name);
        if arch_path.exists() {
            return Ok(arch_path);
        }

        let flat_path = exe_dir.join("tools").join(name);
        if flat_path.exists() {
            return Ok(flat_path);
        }
    }

    // 5. System PATH
    if let Ok(path) = which(name) {
        return Ok(path);
    }

    bail!(
        "{} not found.\n\
         Set BONK_TOOLS_DIR=/path/to/dir, place it at tools/{}/{}\n\
         next to the bonk binary, or ensure it is on PATH.",
        name, arch, name,
    );
}

/// Read a tool binary into memory. Resolves the path via [`get_tool_path`].
pub fn get_tool_bytes(name: &str, override_path: Option<&Path>) -> Result<Vec<u8>> {
    let path = get_tool_path(name, override_path)?;
    std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))
}

/// Locate the bonk-runner binary. Delegates to `get_tool_bytes`.
pub fn get_runner_bytes(override_path: Option<&Path>) -> Result<Vec<u8>> {
    get_tool_bytes("bonk-runner", override_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // NOTE: tests that mutate env vars (BONK_TOOLS_DIR) are not parallel-safe.
    // Run the whole module with: cargo test -p bonk-cli -- pack::tests --test-threads=1

    #[test]
    fn test_override_path_resolves_correctly() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = get_tool_path("bwrap", Some(tmp.path())).unwrap();
        assert_eq!(path, tmp.path());
    }

    #[test]
    fn test_override_path_reads_correct_bytes() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"hello tool").unwrap();
        let bytes = get_tool_bytes("bwrap", Some(tmp.path())).unwrap();
        assert_eq!(bytes, b"hello tool");
    }

    #[test]
    fn test_override_path_missing_returns_error() {
        let result = get_tool_path("bwrap", Some(Path::new("/nonexistent/path/to/bwrap")));
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("--bwrap-path"), "expected --bwrap-path in: {msg}");
    }

    #[test]
    fn test_bonk_tools_dir_finds_binary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mytool"), b"tool bytes").unwrap();
        unsafe { std::env::set_var("BONK_TOOLS_DIR", dir.path()); }
        let path = get_tool_path("mytool", None).unwrap();
        unsafe { std::env::remove_var("BONK_TOOLS_DIR"); }
        assert_eq!(path, dir.path().join("mytool"));
    }

    #[test]
    fn test_bonk_tools_dir_set_but_file_absent_falls_through_to_path() {
        // Point BONK_TOOLS_DIR at an empty dir so step 1 misses,
        // then rely on step 5 (PATH) to find `cat`.
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("BONK_TOOLS_DIR", dir.path()); }
        let path = get_tool_path("cat", None).unwrap();
        unsafe { std::env::remove_var("BONK_TOOLS_DIR"); }
        assert!(path.exists());
    }

    #[test]
    fn test_path_search_finds_system_binary() {
        // `cat` is available on every Unix system.
        unsafe { std::env::remove_var("BONK_TOOLS_DIR"); }
        let path = get_tool_path("cat", None).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_sibling_of_exe_is_found() {
        // The test binary itself is a sibling of itself.
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap().to_string();
        unsafe { std::env::remove_var("BONK_TOOLS_DIR"); }
        let path = get_tool_path(&name, None).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_not_found_returns_helpful_error() {
        unsafe { std::env::remove_var("BONK_TOOLS_DIR"); }
        let result = get_tool_path("bonk-nonexistent-tool-xyz-abc", None);
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("BONK_TOOLS_DIR"), "expected BONK_TOOLS_DIR in: {msg}");
    }

    #[test]
    fn test_write_sections_writes_bytes_in_footer_order() {
        let tempdir = tempfile::tempdir().unwrap();
        let output = tempdir.path().join("out.bin");
        let mut file = std::fs::File::create(&output).unwrap();

        let footer = write_sections(
            &mut file,
            b"RUN",
            b"PAYLOAD",
            b"B",
            b"UNS",
            br#"{"cmd":["echo"]}"#,
        )
        .unwrap();
        drop(file);

        assert_eq!(footer.payload_offset, 3);
        assert_eq!(footer.payload_size, 7);
        assert_eq!(footer.bwrap_size, 1);
        assert_eq!(footer.unsquashfs_size, 3);
        assert_eq!(footer.config_size, 16);
        assert_eq!(
            std::fs::read(&output).unwrap(),
            b"RUNPAYLOADBUNS{\"cmd\":[\"echo\"]}"
        );
    }
}
