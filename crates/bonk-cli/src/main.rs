mod flatten;
mod image;
mod pack;
use crate::flatten::flatten_layers;
use crate::image::export_image;
use crate::pack::assemble;
use crate::pack::build_squashfs;
use anyhow::{Context, Result, bail};
use bonk_common::human_size;
use clap::Parser;
use std::path::Path;

macro_rules! log {
    ($quiet:expr, $($arg:tt)*) => {
        if !$quiet {
            eprintln!($($arg)*);
        }
    };
}

#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "bonk",
    version = env!("CARGO_PKG_VERSION"),
    about = "Smash a Docker image into a single self-contained executable",
    long_about = "bonk exports a Docker image, flattens its layers into a SquashFS rootfs,
and assembles a single static binary that runs the container via bwrap.
The output binary has zero runtime dependencies on the target machine.

EXAMPLES:
  bonk alpine:latest
  ./alpine echo \"ooga booga\"

  bonk python:3.12-slim -o python3
  scp python3 someserver:
  ssh someserver ./python3 -c \"print('hello')\"",
    after_help = "Both `bonk` and `bonk-runner` must be on PATH or in the same directory."
)]
pub struct Cli {
    /// Docker image to pack (e.g. alpine:latest, ubuntu:22.04, myrepo/myimage:1.0)
    #[arg(value_name = "IMAGE")]
    image: String,

    /// Output binary path [default: ./<image-name>]
    #[arg(short, long, value_name = "FILE")]
    output: Option<String>,

    /// Path to a static bwrap binary to embed [default: search PATH]
    #[arg(long, value_name = "PATH")]
    bwrap_path: Option<String>,

    /// Path to a static unsquashfs binary to embed [default: search PATH]
    #[arg(long, value_name = "PATH")]
    unsquashfs_path: Option<String>,

    /// Suppress progress output
    #[arg(short, long)]
    quiet: bool,
}

pub fn extract_binary_name(image: &str) -> Result<String> {
    let name = image.split('/').next_back().unwrap_or(image);
    Ok(name.split(':').next().unwrap_or(name).to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let output = match cli.output {
        Some(o) => o,
        None => extract_binary_name(&cli.image).context("failed to extract binary")?,
    };
    if output.is_empty() {
        bail!("output binary name cannot be empty");
    }
    let tempdir = tempfile::tempdir().context("failed to create temp directory")?;

    log!(cli.quiet, "bonk: packing {} → {}", cli.image, output);
    log!(cli.quiet, "bonk: [1/5] exporting image...");
    let image_dir = export_image(&cli.image, tempdir.path()).context("failed to export image")?;
    log!(cli.quiet, "bonk: [2/5] parsing manifest...");
    let (config, layer_paths) = image::parse_image(&image_dir)?;
    log!(
        cli.quiet,
        "bonk: [3/5] flattening {} layer(s)...",
        layer_paths.len()
    );
    let rootfs_path = tempdir.path().join("rootfs");
    flatten_layers(&layer_paths, &rootfs_path).context("failed to flatten image layers")?;
    log!(cli.quiet, "bonk: [4/5] compressing rootfs...");
    let payload = build_squashfs(&rootfs_path).context("failed to build squashfs")?;
    log!(cli.quiet, "bonk: [5/5] assembling binary...");
    let total = assemble(
        &output,
        &payload,
        &config,
        cli.bwrap_path.as_deref().map(Path::new),
        cli.unsquashfs_path.as_deref().map(Path::new),
    )
    .context("failed to assemble binary")?;
    log!(
        cli.quiet,
        "bonk: done — wrote {} ({})",
        output,
        human_size(total)
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_binary_name() {
        let cases = [
            ("alpine:latest", "alpine"),
            ("ubuntu:22.04", "ubuntu"),
            ("myrepo/myimage:1.0", "myimage"),
            ("myimage", "myimage"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                extract_binary_name(input).unwrap(),
                expected,
                "input: {input}"
            );
        }
    }
}
