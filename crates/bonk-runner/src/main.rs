mod mount;
mod runtime;

use anyhow::{Context, Result, bail};

use runtime::VolumeMount;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use bonk_common::{FOOTER_SIZE, Footer};

macro_rules! log {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}

/// Single marker file. Content is "mount" or "extract" to record which
/// strategy was used on the cold start.
const MARKER: &str = ".bonk-ready";

struct EmbeddedTools {
    bwrap: Option<PathBuf>,
    unsquashfs: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let bin_name = args
        .first()
        .and_then(|s| std::path::Path::new(s).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("<binary>");

    if args.iter().any(|a| a == "--version" || a == "-V") {
        eprintln!("{} (bonk-runner {})", bin_name, env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("A bonk-generated container binary.");
        eprintln!();
        eprintln!("USAGE:");
        eprintln!("  {bin_name} [OPTIONS] [-- CMD [ARGS...]]");
        eprintln!();
        eprintln!("OPTIONS:");
        eprintln!("  -v, --volume HOST:GUEST[:ro]   Bind-mount a host path into the container.");
        eprintln!("                                 Append :ro for a read-only mount. Repeatable.");
        eprintln!("  --mount                        Mount the embedded squashfs rootfs and exit.");
        eprintln!("                                 Requires root. Run once with sudo to enable");
        eprintln!("                                 persistent squashfs mounts for all subsequent");
        eprintln!("                                 invocations (no extraction needed).");
        eprintln!("  -q, --quiet                    Suppress progress output.");
        eprintln!("  -V, --version                  Print version and exit.");
        eprintln!("  -h, --help                     Print this help and exit.");
        eprintln!("  --                             Treat all following arguments as CMD.");
        eprintln!();
        eprintln!("ARGS:");
        eprintln!("  CMD [ARGS...]   Command to run inside the container.");
        eprintln!("                  Overrides the image's default CMD. Without --, the first");
        eprintln!("                  unrecognised argument and everything after it becomes CMD.");
        eprintln!();
        eprintln!("ENVIRONMENT:");
        eprintln!("  BONK_BWRAP=<path>   Override the embedded bwrap binary.");
        eprintln!();
        eprintln!("EXAMPLES:");
        eprintln!("  {bin_name} echo hello");
        eprintln!("  sudo {bin_name} --mount && {bin_name} echo hello");
        eprintln!("  {bin_name} -v /data:/data -- python3 /data/script.py");
        eprintln!("  {bin_name} -v /etc/passwd:/etc/passwd:ro id");
        std::process::exit(0);
    }

    let mut volumes: Vec<VolumeMount> = Vec::new();
    let mut extra_args: Vec<String> = Vec::new();
    let mut quiet = false;
    let mut do_mount_only = false;
    let mut saw_sep = false;
    let stdin_is_tty = std::io::stdin().is_terminal();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if saw_sep {
            extra_args.push(arg.clone());
        } else if arg == "--" {
            saw_sep = true;
        } else if arg == "--mount" {
            do_mount_only = true;
        } else if arg == "-q" || arg == "--quiet" {
            quiet = true;
        } else if arg == "-v" || arg == "--volume" {
            i += 1;
            if let Some(spec) = args.get(i) {
                volumes.push(VolumeMount::parse(spec));
            }
        } else if let Some(spec) = arg.strip_prefix("-v") {
            volumes.push(VolumeMount::parse(spec));
        } else {
            extra_args.push(arg.clone());
            saw_sep = true;
        }
        i += 1;
    }

    let exe_data = std::fs::read("/proc/self/exe").context("failed to read own binary")?;
    if exe_data.len() < FOOTER_SIZE {
        bail!("binary too small to contain bonk footer");
    }
    let footer = Footer::from_bytes(&exe_data)
        .ok_or_else(|| anyhow::anyhow!("not a bonk binary — footer magic does not match"))?;
    let payload = &exe_data
        [footer.payload_offset as usize..(footer.payload_offset + footer.payload_size) as usize];
    let config_data = &exe_data
        [footer.config_offset() as usize..(footer.config_offset() + footer.config_size) as usize];
    let config: bonk_common::ContainerConfig =
        serde_json::from_slice(config_data).context("failed to parse config JSON")?;

    let mut hasher = DefaultHasher::new();
    payload[..4096.min(payload.len())].hash(&mut hasher);
    payload.len().hash(&mut hasher);
    let key: u64 = hasher.finish();
    let cache_dir = PathBuf::from(format!("/tmp/bonk-{:016x}", key));
    let rootfs_path = cache_dir.join("rootfs");
    let sqfs_path = cache_dir.join("rootfs.sqfs");
    let marker = cache_dir.join(MARKER);

    // --mount: privileged setup step, meant to be run via sudo.
    // Mounts the squashfs, pre-creates bin/, chowns the cache dir back to
    // the invoking user (SUDO_UID/SUDO_GID) so unprivileged runs can write
    // tool binaries there. The squashfs mountpoint itself stays root-owned.
    if do_mount_only {
        // Validate cache_dir is not a pre-placed symlink before operating as root.
        if cache_dir.exists() && cache_dir.symlink_metadata()?.file_type().is_symlink() {
            anyhow::bail!(
                "cache dir {} is a symlink — refusing to operate as root",
                cache_dir.display()
            );
        }
        let bin_dir = cache_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).context("failed to create bin dir")?;
        std::fs::create_dir_all(&rootfs_path).context("failed to create rootfs mountpoint")?;
        log!(quiet, "bonk: writing squashfs...");
        std::fs::write(&sqfs_path, payload).context("failed to write squashfs payload")?;
        log!(
            quiet,
            "bonk: mounting squashfs at {}...",
            rootfs_path.display()
        );
        mount::try_squashfs_mount(&sqfs_path, &rootfs_path)
            .context("mount failed — are you running as root?")?;
        std::fs::write(&marker, b"mount").context("failed to write marker")?;
        // Chown cache artifacts back to the invoking user.
        // chown cache_dir itself (non-recursively) so unprivileged runs can
        // create/remove files in it without touching the squashfs mountpoint.
        if let (Ok(uid), Ok(gid)) = (std::env::var("SUDO_UID"), std::env::var("SUDO_GID")) {
            let owner = format!("{uid}:{gid}");
            let _ = std::process::Command::new("chown")
                .arg("-R")
                .arg(&owner)
                .arg(&bin_dir)
                .arg(&sqfs_path)
                .arg(&marker)
                .status();
            // Chown the cache dir itself separately (not -R, to avoid touching
            // the squashfs mountpoint inside it).
            let _ = std::process::Command::new("chown")
                .arg(&owner)
                .arg(&cache_dir)
                .status();
        }
        log!(
            quiet,
            "bonk: mounted — subsequent invocations will use the cached mount"
        );
        return Ok(());
    }

    // Read the marker to determine prior strategy ("mount" or "extract").
    let prior_strategy = std::fs::read_to_string(&marker).ok();

    let rootfs_readonly = match prior_strategy.as_deref() {
        Some("mount") => {
            if !mount::is_squashfs_mounted(&rootfs_path) {
                // Mount gone (e.g. after reboot) — try to re-mount
                log!(quiet, "bonk: squashfs mount gone — re-mounting...");
                std::fs::write(&sqfs_path, payload)
                    .context("failed to write squashfs for re-mount")?;
                mount::try_squashfs_mount(&sqfs_path, &rootfs_path).with_context(|| {
                    format!(
                        "squashfs mount disappeared and re-mount failed.\n\
                         Run `sudo {} --mount` to restore it.",
                        bin_name
                    )
                })?;
                log!(quiet, "bonk: re-mounted successfully");
            } else {
                log!(quiet, "bonk: using cached squashfs mount");
            }
            true
        }
        Some("extract") => {
            log!(quiet, "bonk: using cached rootfs");
            false
        }
        _ => {
            // Cold start
            let _ = std::fs::remove_dir_all(&cache_dir);
            // Always create cache_dir first — mount_or_extract writes sqfs_path
            // into it regardless of whether tools are embedded.
            std::fs::create_dir_all(&cache_dir).context("failed to create cache dir")?;
            log!(quiet, "bonk: [1/2] preparing rootfs...");
            let tools = extract_embedded_tools(&footer, &exe_data, &cache_dir)?;
            let mounted = mount::mount_or_extract(
                payload,
                &sqfs_path,
                &rootfs_path,
                tools.unsquashfs.as_deref(),
            )?;
            let strategy = if mounted { "mount" } else { "extract" };
            std::fs::write(&marker, strategy).context("failed to write marker")?;
            log!(quiet, "bonk: [2/2] starting container");
            mounted
        }
    };

    let tools = extract_embedded_tools(&footer, &exe_data, &cache_dir)?;
    let status = runtime::run(
        &rootfs_path,
        &config,
        &extra_args,
        &volumes,
        tools.bwrap.as_deref(),
        stdin_is_tty,
        rootfs_readonly,
    )?;
    std::process::exit(status.code().unwrap_or(1));
}

fn extract_embedded_tools(
    footer: &Footer,
    exe_data: &[u8],
    cache_dir: &Path,
) -> Result<EmbeddedTools> {
    if !footer.has_embedded_tools() {
        return Ok(EmbeddedTools {
            bwrap: None,
            unsquashfs: None,
        });
    }

    let bin_dir = cache_dir.join("bin");
    std::fs::create_dir_all(&bin_dir).context("failed to create bin dir")?;

    let bwrap_path = bin_dir.join("bwrap");
    let unsquashfs_path = bin_dir.join("unsquashfs");

    if !bwrap_path.exists() {
        let start = footer.bwrap_offset() as usize;
        std::fs::write(
            &bwrap_path,
            &exe_data[start..start + footer.bwrap_size as usize],
        )
        .context("failed to write bwrap")?;
        set_executable(&bwrap_path)?;
    }

    if !unsquashfs_path.exists() {
        let start = footer.unsquashfs_offset() as usize;
        std::fs::write(
            &unsquashfs_path,
            &exe_data[start..start + footer.unsquashfs_size as usize],
        )
        .context("failed to write unsquashfs")?;
        set_executable(&unsquashfs_path)?;
    }

    Ok(EmbeddedTools {
        bwrap: Some(bwrap_path),
        unsquashfs: Some(unsquashfs_path),
    })
}

fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}
