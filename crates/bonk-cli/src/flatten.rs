use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use flate2::read::GzDecoder;

fn determine_file_format(path: &Path) -> Result<&'static str> {
    let mut file = std::fs::File::open(path).context("failed to open layer file for format detection")?;
    let mut buf = [0u8; 2];
    file.read_exact(&mut buf).context("failed to read layer file")?;
    match &buf {
        [0x1F, 0x8B] => Ok("gzip"),
        [0x28, 0xB5] => Ok("zstd"),
        _ => Ok("raw"),
    }
}

fn open_layer(path: &Path) -> Result<Box<dyn Read>> {
    let file = File::open(path).context("failed to open layer file")?;
    let file_format = determine_file_format(path)?;
    let reader: Box<dyn Read> =  match file_format {
        "gzip" => Box::new(GzDecoder::new(file)),
        "zstd" => Box::new(zstd::Decoder::new(file).context("failed to create zstd decoder")?),
        "raw" => Box::new(file),
        _ => anyhow::bail!("unsupported layer file format: {}", file_format),
    };
    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_file_with_bytes(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f
    }

    #[test]
    fn test_determine_file_format_gzip() {
        let f = tmp_file_with_bytes(&[0x1F, 0x8B, 0x00]);
        assert_eq!(determine_file_format(f.path()).unwrap(), "gzip");
    }

    #[test]
    fn test_determine_file_format_zstd() {
        let f = tmp_file_with_bytes(&[0x28, 0xB5, 0x00]);
        assert_eq!(determine_file_format(f.path()).unwrap(), "zstd");
    }

    #[test]
    fn test_determine_file_format_raw() {
        let f = tmp_file_with_bytes(&[0x00, 0x00, 0x00]);
        assert_eq!(determine_file_format(f.path()).unwrap(), "raw");
    }

    #[test]
    fn test_determine_file_format_too_short() {
        let f = tmp_file_with_bytes(&[0x1F]);
        assert!(determine_file_format(f.path()).is_err());
    }

    #[test]
    fn test_open_layer_raw() {
        let f = tmp_file_with_bytes(b"hello raw");
        let mut reader = open_layer(f.path()).unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello raw");
    }

    #[test]
    fn test_open_layer_gzip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(b"hello gzip").unwrap();
        let compressed = enc.finish().unwrap();
        let f = tmp_file_with_bytes(&compressed);
        let mut reader = open_layer(f.path()).unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello gzip");
    }

    #[test]
    fn test_open_layer_zstd() {
        let compressed = zstd::encode_all(b"hello zstd" as &[u8], 0).unwrap();
        let f = tmp_file_with_bytes(&compressed);
        let mut reader = open_layer(f.path()).unwrap();
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello zstd");
    }
}
