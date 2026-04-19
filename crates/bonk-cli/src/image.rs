use anyhow::{Context, Result, bail};
use bonk_common::ContainerConfig;
use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};
use tar::Archive;

/// Image export and parsing logic.
/// Parses the `DOCKER` environment variable into a program name and prefix args.
/// Falls back to `"docker"` with no prefix args if the variable is not set.
fn parse_docker_cmd(docker_cmd: &str) -> (&str, Vec<&str>) {
    let mut parts = docker_cmd.split_whitespace();
    let program = parts.next().unwrap_or("docker");
    let prefix_args = parts.collect();
    (program, prefix_args)
}

pub fn get_export_image_path(image: &str, workdir: &Path) -> Result<PathBuf> {
    if image.is_empty() {
        bail!("image name must not be empty");
    }
    let image_dir = workdir.join(image.replace(['/', ':'], "_"));
    std::fs::create_dir_all(&image_dir).context("failed to create image dir")?;
    Ok(image_dir)
}

pub fn export_image(image: &str, workdir: &Path) -> Result<PathBuf> {
    let image_output_dir = get_export_image_path(image, workdir)?;
    let output = image_output_dir.join("image.tar");
    let docker_cmd = env::var("DOCKER").unwrap_or_else(|_| "docker".to_string());
    let (program, prefix_args) = parse_docker_cmd(&docker_cmd);
    let status = std::process::Command::new(program)
        .args(&prefix_args)
        .args(["save", "-o"])
        .arg(&output)
        .arg(image)
        .status()
        .context("failed to execute docker save")?;
    if !status.success() {
        bail!("docker save failed with status: {}", status);
    }
    let file = std::fs::File::open(&output).context("failed to open exported image tarball")?;
    let mut archive = Archive::new(file);
    archive
        .unpack(&image_output_dir)
        .context("failed to unpack image tarball")?;
    std::fs::remove_file(&output).context("failed to remove image tarball")?;
    Ok(image_output_dir)
}

/// Image manifest parsing and layer flattening logic.
/// TODO: move this to a separate module for better organization.
///

#[derive(Deserialize)]
struct DockerManifest {
    #[serde(rename = "Config")]
    config: String,
    #[serde(rename = "Layers")]
    layers: Vec<String>,
}

#[derive(Deserialize)]
struct DockerImageConfig {
    #[serde(rename = "config")]
    config: Option<DockerContainerConfig>,
}

#[derive(Deserialize)]
struct DockerContainerConfig {
    #[serde(rename = "Entrypoint")]
    entrypoint: Option<Vec<String>>,
    #[serde(rename = "Cmd")]
    cmd: Option<Vec<String>>,
    #[serde(rename = "Env")]
    env: Option<Vec<String>>,
    #[serde(rename = "WorkingDir")]
    working_dir: Option<String>,
    #[serde(rename = "User")]
    user: Option<String>,
}

pub fn parse_image(image_dir: &Path) -> Result<(ContainerConfig, Vec<PathBuf>)> {
    let manifest_path = image_dir.join("manifest.json");
    let manifest_data = std::fs::read(&manifest_path).context("failed to read manifest.json")?;
    let manifest: Vec<DockerManifest> =
        serde_json::from_slice(&manifest_data).context("failed to parse manifest.json")?;
    if manifest.len() != 1 {
        bail!(
            "expected exactly 1 image in manifest.json, found {}",
            manifest.len()
        );
    }
    let manifest = manifest.into_iter().next().unwrap();
    let config_path = image_dir.join(&manifest.config);
    let config_data = std::fs::read(&config_path).context("failed to read config file")?;
    let config: DockerImageConfig =
        serde_json::from_slice(&config_data).context("failed to parse config file")?;
    let container_config = match config.config {
        Some(c) => ContainerConfig {
            entrypoint: c.entrypoint.unwrap_or_default(),
            cmd: c.cmd.unwrap_or_default(),
            env: c.env.unwrap_or_else(|| ContainerConfig::default().env),
            working_dir: c
                .working_dir
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/".to_string()),
            user: c.user.filter(|s| !s.is_empty()),
        },
        None => ContainerConfig::default(),
    };
    let layer_paths = manifest.layers.iter().map(|l| image_dir.join(l)).collect();
    Ok((container_config, layer_paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_image_fixture(
        tempdir: &tempfile::TempDir,
        manifest: &str,
        config_name: &str,
        config: &str,
    ) {
        fs::write(tempdir.path().join("manifest.json"), manifest).unwrap();
        fs::write(tempdir.path().join(config_name), config).unwrap();
    }

    #[test]
    fn test_parse_docker_cmd_default() {
        let (program, prefix_args) = parse_docker_cmd("docker");
        assert_eq!(program, "docker");
        assert!(prefix_args.is_empty());
    }

    #[test]
    fn test_parse_docker_cmd_empty() {
        let (program, prefix_args) = parse_docker_cmd("");
        assert_eq!(program, "docker");
        assert!(prefix_args.is_empty());
    }

    #[test]
    fn test_parse_docker_cmd_with_prefix() {
        let (program, prefix_args) = parse_docker_cmd("sudo docker");
        assert_eq!(program, "sudo");
        assert_eq!(prefix_args, vec!["docker"]);
    }

    #[test]
    fn test_parse_docker_cmd_with_multiple_prefix_args() {
        let (program, prefix_args) = parse_docker_cmd("sudo -E docker");
        assert_eq!(program, "sudo");
        assert_eq!(prefix_args, vec!["-E", "docker"]);
    }

    #[test]
    fn test_get_export_image_path() {
        let path = get_export_image_path("alpine:latest", Path::new("/tmp/work")).unwrap();
        assert_eq!(path, Path::new("/tmp/work/alpine_latest"));
    }

    #[test]
    fn test_get_export_image_path_with_registry() {
        let path = get_export_image_path("myrepo/myimage:1.0", Path::new("/tmp/work")).unwrap();
        assert_eq!(path, Path::new("/tmp/work/myrepo_myimage_1.0"));
    }

    #[test]
    fn test_get_export_image_path_empty_image() {
        let result = get_export_image_path("", Path::new("/tmp/work"));
        assert!(result.is_err(), "expected error for empty image name");
    }

    #[test]
    fn test_parse_image_reads_layers_and_normalizes_empty_fields() {
        let tempdir = tempfile::tempdir().unwrap();
        write_image_fixture(
            &tempdir,
            r#"[{"Config":"config.json","Layers":["layer1.tar","nested/layer2.tar"]}]"#,
            "config.json",
            r#"{"config":{"Entrypoint":["/bin/sh"],"Cmd":["-c","echo hi"],"Env":["A=1"],"WorkingDir":"","User":""}}"#,
        );

        let (config, layers) = parse_image(tempdir.path()).unwrap();

        assert_eq!(config.entrypoint, vec!["/bin/sh"]);
        assert_eq!(config.cmd, vec!["-c", "echo hi"]);
        assert_eq!(config.env, vec!["A=1"]);
        assert_eq!(config.working_dir, "/");
        assert_eq!(config.user, None);
        assert_eq!(
            layers,
            vec![
                tempdir.path().join("layer1.tar"),
                tempdir.path().join("nested/layer2.tar"),
            ]
        );
    }

    #[test]
    fn test_parse_image_defaults_when_config_section_is_missing() {
        let tempdir = tempfile::tempdir().unwrap();
        write_image_fixture(
            &tempdir,
            r#"[{"Config":"config.json","Layers":[]}]"#,
            "config.json",
            r#"{"config":null}"#,
        );

        let (config, layers) = parse_image(tempdir.path()).unwrap();

        assert!(config.entrypoint.is_empty());
        assert!(config.cmd.is_empty());
        assert!(config.env.is_empty());
        assert_eq!(config.working_dir, "/");
        assert_eq!(config.user, None);
        assert!(layers.is_empty());
    }

    #[test]
    fn test_parse_image_rejects_manifests_with_multiple_images() {
        let tempdir = tempfile::tempdir().unwrap();
        write_image_fixture(
            &tempdir,
            r#"[{"Config":"config.json","Layers":[]},{"Config":"config.json","Layers":[]}]"#,
            "config.json",
            r#"{"config":null}"#,
        );

        let error = parse_image(tempdir.path()).unwrap_err();

        assert!(format!("{error:#}").contains("expected exactly 1 image"));
    }
}
