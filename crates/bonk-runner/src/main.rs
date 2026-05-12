mod mount;
mod runtime;

use anyhow::{Context, Result, bail};

use clap::Parser;
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

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

/// A bonk-generated container binary.
#[derive(Parser)]
#[command(
    disable_help_flag = false,
    disable_version_flag = false,
    version = env!("CARGO_PKG_VERSION"),
)]
struct Args {
    /// Set an environment variable inside the container.
    /// Appended after image vars; overrides image defaults. No host env vars leak implicitly.
    #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
    runtime_env: Vec<String>,

    /// Bind-mount a host path into the container. Append :ro for read-only.
    #[arg(short = 'v', long = "volume", value_name = "HOST:GUEST[:ro]")]
    volumes: Vec<String>,

    /// Privileged first-run setup: mount the embedded squashfs rootfs.
    /// Requires root (run with sudo). Subsequent plain invocations skip this step.
    #[arg(long)]
    mount: bool,

    /// Suppress progress output.
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Command to run inside the container (overrides the image's default CMD).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    extra_args: Vec<String>,
}

// ---------------------------------------------------------------------------
// Own-binary metadata
// ---------------------------------------------------------------------------

struct BinaryData {
    exe_data: Vec<u8>,
    footer: Footer,
    config: bonk_common::ContainerConfig,
}

impl BinaryData {
    fn load() -> Result<Self> {
        let exe_data = std::fs::read("/proc/self/exe").context("failed to read own binary")?;
        if exe_data.len() < FOOTER_SIZE {
            bail!("binary too small to contain bonk footer");
        }
        let footer = Footer::from_bytes(&exe_data)
            .ok_or_else(|| anyhow::anyhow!("not a bonk binary — footer magic does not match"))?;
        let config_data = &exe_data[footer.config_offset() as usize
            ..(footer.config_offset() + footer.config_size) as usize];
        let config: bonk_common::ContainerConfig =
            serde_json::from_slice(config_data).context("failed to parse config JSON")?;
        Ok(BinaryData {
            exe_data,
            footer,
            config,
        })
    }

    fn payload(&self) -> &[u8] {
        let start = self.footer.payload_offset as usize;
        let end = start + self.footer.payload_size as usize;
        &self.exe_data[start..end]
    }
}

// ---------------------------------------------------------------------------
// Cache paths
// ---------------------------------------------------------------------------

struct CachePaths {
    dir: PathBuf,
    rootfs: PathBuf,
    sqfs: PathBuf,
    marker: PathBuf,
}

impl CachePaths {
    fn new(binary: &BinaryData) -> Self {
        let payload = binary.payload();
        let mut hasher = DefaultHasher::new();
        payload[..4096.min(payload.len())].hash(&mut hasher);
        payload.len().hash(&mut hasher);
        let key: u64 = hasher.finish();
        let dir = PathBuf::from(format!("/tmp/bonk-{:016x}", key));
        let rootfs = dir.join("rootfs");
        let sqfs = dir.join("rootfs.sqfs");
        let marker = dir.join(MARKER);
        CachePaths {
            dir,
            rootfs,
            sqfs,
            marker,
        }
    }
}

// ---------------------------------------------------------------------------
// Privileged mount-only setup  (--mount)
// ---------------------------------------------------------------------------

fn run_mount_setup(cache: &CachePaths, payload: &[u8], quiet: bool) -> Result<()> {
    // Atomically create or validate cache_dir before writing as root.
    // Prevents TOCTOU symlink attacks under the world-writable /tmp.
    mount::init_cache_dir_as_root(&cache.dir)?;
    let bin_dir = cache.dir.join("bin");
    std::fs::create_dir_all(&bin_dir).context("failed to create bin dir")?;
    std::fs::create_dir_all(&cache.rootfs).context("failed to create rootfs mountpoint")?;
    log!(quiet, "bonk: writing squashfs...");
    std::fs::write(&cache.sqfs, payload).context("failed to write squashfs payload")?;
    log!(
        quiet,
        "bonk: mounting squashfs at {}...",
        cache.rootfs.display()
    );
    mount::try_squashfs_mount(&cache.sqfs, &cache.rootfs)
        .context("mount failed — are you running as root?")?;
    std::fs::write(&cache.marker, b"mount").context("failed to write marker")?;
    // Chown cache artifacts back to the invoking user.
    // chown cache_dir itself (non-recursively) so unprivileged runs can
    // create/remove files in it without touching the squashfs mountpoint.
    if let (Ok(uid), Ok(gid)) = (std::env::var("SUDO_UID"), std::env::var("SUDO_GID")) {
        let owner = format!("{uid}:{gid}");
        let _ = std::process::Command::new("chown")
            .arg("-R")
            .arg(&owner)
            .arg(&bin_dir)
            .arg(&cache.sqfs)
            .arg(&cache.marker)
            .status();
        // Chown the cache dir itself separately (not -R, to avoid touching
        // the squashfs mountpoint inside it).
        let _ = std::process::Command::new("chown")
            .arg(&owner)
            .arg(&cache.dir)
            .status();
    }
    log!(
        quiet,
        "bonk: mounted — subsequent invocations will use the cached mount"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Rootfs strategy selection  (warm / cold start)
// ---------------------------------------------------------------------------

/// Ensures the rootfs is available and returns whether it is read-only
/// (i.e. kernel-mounted squashfs) or read-write (extracted directory).
fn prepare_rootfs(
    binary: &BinaryData,
    cache: &CachePaths,
    bin_name: &str,
    quiet: bool,
) -> Result<bool> {
    let payload = binary.payload();
    let prior_strategy = std::fs::read_to_string(&cache.marker).ok();

    match prior_strategy.as_deref() {
        Some("mount") => {
            if !mount::is_squashfs_mounted(&cache.rootfs) {
                // Mount gone (e.g. after reboot) — try to re-mount
                log!(quiet, "bonk: squashfs mount gone — re-mounting...");
                std::fs::write(&cache.sqfs, payload)
                    .context("failed to write squashfs for re-mount")?;
                mount::try_squashfs_mount(&cache.sqfs, &cache.rootfs).with_context(|| {
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
            Ok(true)
        }
        Some("extract") => {
            log!(quiet, "bonk: using cached rootfs");
            Ok(false)
        }
        _ => {
            // Cold start
            let _ = std::fs::remove_dir_all(&cache.dir);
            // Always create cache_dir first — mount_or_extract writes sqfs_path
            // into it regardless of whether tools are embedded.
            std::fs::create_dir_all(&cache.dir).context("failed to create cache dir")?;
            log!(quiet, "bonk: [1/2] preparing rootfs...");
            let tools = extract_embedded_tools(&binary.footer, &binary.exe_data, &cache.dir)?;
            let mounted = mount::mount_or_extract(
                payload,
                &cache.sqfs,
                &cache.rootfs,
                tools.unsquashfs.as_deref(),
            )?;
            let strategy = if mounted { "mount" } else { "extract" };
            std::fs::write(&cache.marker, strategy).context("failed to write marker")?;
            log!(quiet, "bonk: [2/2] starting container");
            Ok(mounted)
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let args = Args::parse();
    let binary = BinaryData::load()?;
    let cache = CachePaths::new(&binary);

    let bin_name = std::env::args()
        .next()
        .as_deref()
        .and_then(|s| std::path::Path::new(s).file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("<binary>")
        .to_string();
    let stdin_is_tty = std::io::stdin().is_terminal();
    let volumes: Vec<VolumeMount> = args.volumes.iter().map(|s| VolumeMount::parse(s)).collect();

    if args.mount {
        return run_mount_setup(&cache, binary.payload(), args.quiet);
    }

    let rootfs_readonly = prepare_rootfs(&binary, &cache, &bin_name, args.quiet)?;
    let tools = extract_embedded_tools(&binary.footer, &binary.exe_data, &cache.dir)?;
    let status = runtime::run(runtime::RunOpts {
        rootfs: &cache.rootfs,
        config: &binary.config,
        extra_args: &args.extra_args,
        volumes: &volumes,
        runtime_env: &args.runtime_env,
        bwrap_path: tools.bwrap.as_deref(),
        stdin_is_tty,
        rootfs_readonly,
    })?;
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
