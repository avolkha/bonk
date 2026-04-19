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
        eprintln!("  {bin_name} -v /data:/data -- python3 /data/script.py");
        eprintln!("  {bin_name} -v /etc/passwd:/etc/passwd:ro id");
        std::process::exit(0);
    }

    let mut volumes: Vec<VolumeMount> = Vec::new();
    let mut extra_args: Vec<String> = Vec::new();
    let mut quiet = false;
    let mut saw_sep = false;
    let stdin_is_tty = std::io::stdin().is_terminal();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if saw_sep {
            extra_args.push(arg.clone());
        } else if arg == "--" {
            saw_sep = true;
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
    let marker = cache_dir.join(".bonk-ready");

    let (bwrap_path, _unsquashfs_path) = if !marker.exists() {
        let _ = std::fs::remove_dir_all(&cache_dir);
        std::fs::create_dir_all(&rootfs_path).context("failed to create rootfs dir")?;
        let paths = extract_embedded_tools(&footer, &exe_data, &cache_dir)?;
        log!(quiet, "bonk: [1/2] extracting rootfs...");
        mount::extract_rootfs(payload, &rootfs_path, paths.1.as_deref())?;
        std::fs::write(&marker, b"").context("failed to write marker")?;
        log!(quiet, "bonk: [2/2] starting container");
        paths
    } else {
        log!(quiet, "bonk: using cached rootfs");
        log!(quiet, "bonk: starting container");
        extract_embedded_tools(&footer, &exe_data, &cache_dir)?
    };

    let status = runtime::run(
        &rootfs_path,
        &config,
        &extra_args,
        &volumes,
        bwrap_path.as_deref(),
        stdin_is_tty,
    )?;
    std::process::exit(status.code().unwrap_or(1));
}

fn extract_embedded_tools(
    footer: &Footer,
    exe_data: &[u8],
    cache_dir: &Path,
) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    if !footer.has_embedded_tools() {
        return Ok((None, None));
    }

    let bin_dir = cache_dir.join("bin");
    std::fs::create_dir_all(&bin_dir).context("failed to create bin dir")?;

    let bwrap_path = bin_dir.join("bwrap");
    let unsquashfs_path = bin_dir.join("unsquashfs");

    if !bwrap_path.exists() {
        let start = footer.bwrap_offset() as usize;
        let end = start + footer.bwrap_size as usize;
        std::fs::write(&bwrap_path, &exe_data[start..end]).context("failed to write bwrap")?;
        set_executable(&bwrap_path)?;
    }

    if !unsquashfs_path.exists() {
        let start = footer.unsquashfs_offset() as usize;
        let end = start + footer.unsquashfs_size as usize;
        std::fs::write(&unsquashfs_path, &exe_data[start..end])
            .context("failed to write unsquashfs")?;
        set_executable(&unsquashfs_path)?;
    }

    Ok((Some(bwrap_path), Some(unsquashfs_path)))
}

fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}
