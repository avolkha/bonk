mod image;
mod flatten;
mod pack;
use anyhow::{bail, Context, Result};
use clap::Parser;
use crate::image::export_image;

#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "bonk",
    about = "Squash a Docker image into a standalone executable",
    long_about = "bonk takes a Docker image and produces a single self-contained Linux \
                  binary that runs the container using bubblewrap (bwrap) for sandboxing.\n\n\
                  Example:\n  bonk alpine:latest\n  ./alpine echo hello"
)]
pub struct Cli{
    // Docker image to squash (e.g. alpine:latest, ubuntu:22.04)
    image: String,
    // Output binary path (default: ./<image_name>)
    output: Option<String>,

    // Path to a static bwrap binary to embed (overrides automatic search)
    bwrap_path: Option<String>,
    // Path to a static unsquashfs binary to embed (overrides automatic search)
    unsquashfs_path: Option<String>
}

pub fn extract_binary_name(image: &str) -> Result<String> {
    let name = image.split('/').last().unwrap_or(image);
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

    println!("Image: {}", cli.image);
    println!("Output: {}", output);
    eprintln!("Exporting image {}... ", cli.image);
    let image_dir = export_image(&cli.image, tempdir.path()).context("failed to export image")?;
    eprintln!("Parsing image manifest... ");
    let (config, layer_paths) = image::parse_image(&image_dir)?;
    eprintln!("Flattening image layers... ");
    eprintln!("Compressing rootfs with mksquashfs... ");
    eprintln!("Assembling binary... ");
    eprintln!("→ {}", output);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_binary_name() {
        let cases = [
            ("alpine:latest",       "alpine"),
            ("ubuntu:22.04",        "ubuntu"),
            ("myrepo/myimage:1.0",  "myimage"),
            ("myimage",             "myimage"),
        ];
        for (input, expected) in cases {
            assert_eq!(extract_binary_name(input).unwrap(), expected, "input: {input}");
        }
    }
}
