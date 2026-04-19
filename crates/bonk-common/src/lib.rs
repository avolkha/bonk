pub const FOOTER_MAGIC: u64 = 0xB04B_B04B_B04B_0002;

pub const FOOTER_SIZE: usize = 56; // 7 × u64

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContainerConfig {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub working_dir: String,
    pub user: Option<String>,
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            entrypoint: vec![],
            cmd: vec![],
            env: vec![],
            working_dir: "/".to_string(),
            user: None,
        }
    }
}

pub struct Footer {
    pub payload_offset: u64,
    pub payload_size: u64,
    pub config_size: u64,
    pub bwrap_size: u64,
    pub unsquashfs_size: u64,
}

impl Footer {
    /// Returns `true` if both `bwrap_size` and `unsquashfs_size` are nonzero.
    ///
    /// # Examples
    ///
    /// ```
    /// use bonk_common::Footer;
    ///
    /// let with_tools = Footer { payload_offset: 0, payload_size: 0, config_size: 0, bwrap_size: 400, unsquashfs_size: 500 };
    /// assert!(with_tools.has_embedded_tools());
    ///
    /// let no_tools = Footer { payload_offset: 0, payload_size: 0, config_size: 0, bwrap_size: 0, unsquashfs_size: 0 };
    /// assert!(!no_tools.has_embedded_tools());
    /// ```
    pub fn has_embedded_tools(&self) -> bool {
        self.bwrap_size > 0 && self.unsquashfs_size > 0
    }
    pub fn bwrap_offset(&self) -> u64 {
        self.payload_offset + self.payload_size
    }
    pub fn unsquashfs_offset(&self) -> u64 {
        self.bwrap_offset() + self.bwrap_size
    }
    pub fn config_offset(&self) -> u64 {
        self.unsquashfs_offset() + self.unsquashfs_size
    }
    /// Serializes the footer into a [`FOOTER_SIZE`]-byte little-endian buffer.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = vec![0u8; FOOTER_SIZE];
        buf[0..8].copy_from_slice(&self.payload_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.payload_size.to_le_bytes());
        buf[16..24].copy_from_slice(&self.config_size.to_le_bytes());
        buf[24..32].copy_from_slice(&self.bwrap_size.to_le_bytes());
        buf[32..40].copy_from_slice(&self.unsquashfs_size.to_le_bytes());
        // reserved for future use
        buf[40..48].copy_from_slice(&0u64.to_le_bytes());
        buf[48..56].copy_from_slice(&FOOTER_MAGIC.to_le_bytes());
        buf
    }
    /// Parses a `Footer` from the tail of a byte slice.
    ///
    /// Returns `None` if the slice is too short or the magic number doesn't match.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < FOOTER_SIZE {
            return None;
        }
        let tail = &data[data.len() - FOOTER_SIZE..];
        let magic = u64::from_le_bytes(tail[48..56].try_into().ok()?);
        if magic != FOOTER_MAGIC {
            return None;
        }
        Some(Footer {
            payload_offset: u64::from_le_bytes(tail[0..8].try_into().ok()?),
            payload_size: u64::from_le_bytes(tail[8..16].try_into().ok()?),
            config_size: u64::from_le_bytes(tail[16..24].try_into().ok()?),
            bwrap_size: u64::from_le_bytes(tail[24..32].try_into().ok()?),
            unsquashfs_size: u64::from_le_bytes(tail[32..40].try_into().ok()?),
        })
    }
}

pub fn human_size(bytes: usize) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = UNITS[0];
    for &u in UNITS.iter() {
        unit = u;
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
    }
    format!("{:.1} {}", size, unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_footer_no_tools_roundtrip() {
        let footer = Footer {
            payload_offset: 1000,
            payload_size: 2000,
            config_size: 300,
            bwrap_size: 0,
            unsquashfs_size: 0,
        };
        let parsed = Footer::from_bytes(&footer.to_bytes()).unwrap();
        assert_eq!(parsed.payload_offset, 1000);
        assert_eq!(parsed.payload_size, 2000);
        assert_eq!(parsed.config_size, 300);
        assert!(!parsed.has_embedded_tools());
    }

    #[test]
    fn test_footer_with_tools_roundtrip() {
        let footer = Footer {
            payload_offset: 1000,
            payload_size: 2000,
            config_size: 300,
            bwrap_size: 400,
            unsquashfs_size: 500,
        };
        let bytes = footer.to_bytes();
        assert_eq!(bytes.len(), FOOTER_SIZE);
        let parsed = Footer::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.bwrap_size, 400);
        assert_eq!(parsed.unsquashfs_size, 500);
    }

    #[test]
    fn test_footer_too_small_returns_none() {
        assert!(Footer::from_bytes(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_footer_wrong_magic_returns_none() {
        // All-zero buffer has no valid magic number
        assert!(Footer::from_bytes(&[0u8; FOOTER_SIZE]).is_none());
    }

    #[test]
    fn test_human_size_bytes() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(512), "512.0 B");
        assert_eq!(human_size(1023), "1023.0 B");
    }

    #[test]
    fn test_human_size_kilobytes() {
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
    }

    #[test]
    fn test_human_size_megabytes() {
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 2), "2.0 MB");
    }

    #[test]
    fn test_human_size_gigabytes() {
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
    }
}
