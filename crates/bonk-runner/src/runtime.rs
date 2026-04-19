use anyhow::{bail, Context, Result};


pub struct VolumeMount {
    pub host: String,
    pub guest: String,
    pub read_only: bool,
}

impl VolumeMount {
    /// Parse a volume spec of the form `HOST:GUEST[:ro]`.
    pub fn parse(spec: &str) -> Self {
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        let host = parts.first().copied().unwrap_or("").to_string();
        let guest = parts.get(1).copied().unwrap_or("").to_string();
        let read_only = parts.get(2).copied() == Some("ro");
        VolumeMount { host, guest, read_only }
    }
}

fn resolve_cmd(config: &bonk_common::ContainerConfig, extra_args: &[String]) -> Result<Vec<String>> {
    if !extra_args.is_empty() {
        Ok(extra_args.to_vec())
    } else if !config.entrypoint.is_empty() && !config.cmd.is_empty() {
        let mut cmd = config.entrypoint.clone();
        cmd.extend(config.cmd.clone());
        Ok(cmd)
    } else if !config.entrypoint.is_empty() {
        Ok(config.entrypoint.clone())
    } else if !config.cmd.is_empty() {
        Ok(config.cmd.clone())
    } else {
        bail!("no command specified in image config and no extra args provided");
    }
}

pub fn run(
    rootfs: &std::path::Path,
    config: &bonk_common::ContainerConfig,
    extra_args: &[String],
    volumes: &[VolumeMount],
    bwrap_path: Option<&std::path::Path>,
    stdin_is_tty: bool,
) -> Result<std::process::ExitStatus> {
    let bwrap_bin = match bwrap_path {
        Some(path) => path.to_path_buf(),
        None => {
            if let Ok(p) = std::env::var("BONK_BWRAP") {
                std::path::PathBuf::from(p)
            } else {
                which::which("bwrap").ok().context("failed to find bwrap in PATH")?
            }
        }
    };
    let bwrap_supports_overlay = std::process::Command::new(&bwrap_bin)
        .arg("--overlay-src").arg("/").arg("--tmp-overlay").arg("/").arg("--").arg("true")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !bwrap_supports_overlay {
        eprintln!("warning: bwrap does not support overlay mode, falling back to bind mounts");
    }
    let mut cmd = std::process::Command::new(bwrap_bin);
    if bwrap_supports_overlay {
        cmd.arg("--overlay-src").arg(rootfs).arg("--tmp-overlay").arg("/");
    } else {
        cmd.arg("--bind").arg(rootfs).arg("/");
    }
    cmd.arg("--dev").arg("/dev");
    cmd.arg("--proc").arg("/proc");
    cmd.arg("--tmpfs").arg("/tmp");
    cmd.arg("--tmpfs").arg("/run");
    cmd.arg("--hostname").arg("bonk");
    cmd.arg("--ro-bind").arg("/etc/resolv.conf").arg("/etc/resolv.conf");

    if unsafe { libc::getuid() } == 0 {
        cmd.arg("--unshare-ipc")
           .arg("--unshare-pid")
           .arg("--unshare-uts")
           .arg("--unshare-cgroup");
    } else {
        cmd.arg("--unshare-all")
           .arg("--share-net")
           .arg("--uid").arg("0")
           .arg("--gid").arg("0");
    }

    for vol in volumes {
        if vol.read_only {
            cmd.arg("--ro-bind").arg(&vol.host).arg(&vol.guest);
        } else {
            cmd.arg("--bind").arg(&vol.host).arg(&vol.guest);
        }
    }

    if !config.env.is_empty() {
        for env in &config.env {
            let (key, value) = env.split_once('=').unwrap_or((env.as_str(), ""));
            cmd.arg("--setenv").arg(key).arg(value);
        }
    }
    if stdin_is_tty {
        cmd.arg("--new-session");
    }
    cmd.arg("--clearenv");
    if let Ok(term) = std::env::var("TERM") {
        cmd.arg("--setenv").arg("TERM").arg(term);
    }
    cmd.arg("--chdir").arg(&config.working_dir);
    cmd.arg("--").args(resolve_cmd(config, extra_args)?);
    cmd.status().context("failed to execute bwrap")

}
