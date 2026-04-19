use anyhow::{Context, Result, bail};

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
        VolumeMount {
            host,
            guest,
            read_only,
        }
    }
}

fn resolve_cmd(
    config: &bonk_common::ContainerConfig,
    extra_args: &[String],
) -> Result<Vec<String>> {
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
                which::which("bwrap")
                    .ok()
                    .context("failed to find bwrap in PATH")?
            }
        }
    };
    let bwrap_supports_overlay = std::process::Command::new(&bwrap_bin)
        .arg("--overlay-src")
        .arg("/")
        .arg("--tmp-overlay")
        .arg("/")
        .arg("--")
        .arg("true")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !bwrap_supports_overlay {
        eprintln!("warning: bwrap does not support overlay mode, falling back to bind mounts");
    }
    let mut cmd = std::process::Command::new(bwrap_bin);
    if bwrap_supports_overlay {
        cmd.arg("--overlay-src")
            .arg(rootfs)
            .arg("--tmp-overlay")
            .arg("/");
    } else {
        cmd.arg("--bind").arg(rootfs).arg("/");
    }
    cmd.arg("--dev").arg("/dev");
    cmd.arg("--proc").arg("/proc");
    cmd.arg("--tmpfs").arg("/tmp");
    cmd.arg("--tmpfs").arg("/run");
    cmd.arg("--hostname").arg("bonk");
    cmd.arg("--ro-bind")
        .arg("/etc/resolv.conf")
        .arg("/etc/resolv.conf");

    if unsafe { libc::getuid() } == 0 {
        cmd.arg("--unshare-ipc")
            .arg("--unshare-pid")
            .arg("--unshare-uts")
            .arg("--unshare-cgroup");
    } else {
        cmd.arg("--unshare-all")
            .arg("--share-net")
            .arg("--uid")
            .arg("0")
            .arg("--gid")
            .arg("0");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn make_config() -> bonk_common::ContainerConfig {
        bonk_common::ContainerConfig {
            entrypoint: vec!["/bin/sh".into()],
            cmd: vec!["-c".into(), "echo from image".into()],
            env: vec!["KEY=value".into(), "EMPTY".into()],
            working_dir: "/work".into(),
            user: None,
        }
    }

    fn write_fake_bwrap(
        dir: &Path,
        log_path: &Path,
        overlay_probe_success: bool,
        exit_code: i32,
    ) -> std::path::PathBuf {
        let script = dir.join("fake-bwrap.sh");
        let probe_exit = if overlay_probe_success { 0 } else { 1 };
        fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 set -eu\n\
                 if [ \"$#\" -ge 5 ] && [ \"$1\" = \"--overlay-src\" ] && [ \"$2\" = \"/\" ] && [ \"$3\" = \"--tmp-overlay\" ] && [ \"$4\" = \"/\" ] && [ \"$5\" = \"--\" ]; then\n\
                 \texit {probe_exit}\n\
                 fi\n\
                 printf '%s\\n' \"$@\" > '{}'\n\
                 exit {exit_code}\n",
                log_path.display(),
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }

    fn read_args(log_path: &Path) -> Vec<String> {
        fs::read_to_string(log_path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn assert_contains_sequence(args: &[String], expected: &[&str]) {
        assert!(
            args.windows(expected.len()).any(|window| window
                .iter()
                .map(String::as_str)
                .eq(expected.iter().copied())),
            "expected sequence {:?} in {:?}",
            expected,
            args
        );
    }

    #[test]
    fn test_volume_mount_parse_variants() {
        let mount = VolumeMount::parse("/host:/guest:ro");
        assert_eq!(mount.host, "/host");
        assert_eq!(mount.guest, "/guest");
        assert!(mount.read_only);

        let mount = VolumeMount::parse("/host:/guest");
        assert_eq!(mount.host, "/host");
        assert_eq!(mount.guest, "/guest");
        assert!(!mount.read_only);

        let mount = VolumeMount::parse("host-only");
        assert_eq!(mount.host, "host-only");
        assert_eq!(mount.guest, "");
        assert!(!mount.read_only);
    }

    #[test]
    fn test_resolve_cmd_prefers_extra_args_and_falls_back_through_image_config() {
        let config = make_config();

        assert_eq!(
            resolve_cmd(&config, &["echo".into(), "override".into()]).unwrap(),
            vec!["echo".to_string(), "override".to_string()]
        );
        assert_eq!(
            resolve_cmd(
                &bonk_common::ContainerConfig {
                    cmd: vec!["echo".into()],
                    ..bonk_common::ContainerConfig::default()
                },
                &[],
            )
            .unwrap(),
            vec!["echo".to_string()]
        );
        assert!(resolve_cmd(&bonk_common::ContainerConfig::default(), &[]).is_err());
    }

    #[test]
    fn test_run_uses_overlay_and_passes_expected_arguments() {
        let tempdir = tempfile::tempdir().unwrap();
        let log_path = tempdir.path().join("args.log");
        let bwrap = write_fake_bwrap(tempdir.path(), &log_path, true, 7);
        let rootfs = tempdir.path().join("rootfs");
        fs::create_dir_all(&rootfs).unwrap();
        let volumes = vec![VolumeMount::parse("/tmp/host:/guest:ro")];

        let status = run(
            &rootfs,
            &make_config(),
            &["echo".into(), "hi".into()],
            &volumes,
            Some(&bwrap),
            true,
        )
        .unwrap();

        assert_eq!(status.code(), Some(7));

        let args = read_args(&log_path);
        assert_contains_sequence(
            &args,
            &[
                "--overlay-src",
                rootfs.to_str().unwrap(),
                "--tmp-overlay",
                "/",
            ],
        );
        assert_contains_sequence(&args, &["--ro-bind", "/tmp/host", "/guest"]);
        assert_contains_sequence(&args, &["--setenv", "KEY", "value"]);
        assert_contains_sequence(&args, &["--setenv", "EMPTY", ""]);
        assert_contains_sequence(&args, &["--new-session", "--clearenv"]);
        assert_contains_sequence(&args, &["--chdir", "/work", "--", "echo", "hi"]);
    }

    #[test]
    fn test_run_falls_back_to_bind_mount_when_overlay_probe_fails() {
        let tempdir = tempfile::tempdir().unwrap();
        let log_path = tempdir.path().join("args.log");
        let bwrap = write_fake_bwrap(tempdir.path(), &log_path, false, 0);
        let rootfs = tempdir.path().join("rootfs");
        fs::create_dir_all(&rootfs).unwrap();

        let status = run(
            &rootfs,
            &bonk_common::ContainerConfig {
                cmd: vec!["echo".into(), "from-image".into()],
                ..bonk_common::ContainerConfig::default()
            },
            &[],
            &[],
            Some(&bwrap),
            false,
        )
        .unwrap();

        assert!(status.success());

        let args = read_args(&log_path);
        assert_contains_sequence(&args, &["--bind", rootfs.to_str().unwrap(), "/"]);
        assert!(!args.iter().any(|arg| arg == "--overlay-src"));
        assert_contains_sequence(&args, &["--", "echo", "from-image"]);
    }
}
