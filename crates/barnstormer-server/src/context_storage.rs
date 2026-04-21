// ABOUTME: Disk storage helpers for context attachments — path layout, filename
// ABOUTME: sanitization, UTF-8 detection, and read/write helpers.

use std::io;
use std::path::{Path, PathBuf};
use ulid::Ulid;

pub fn sanitize_filename(raw: &str) -> String {
    // Strip any directory components, then replace control chars and known
    // path-dangerous chars with '_'. Empty result becomes "file".
    let base = Path::new(raw).file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | '\0') { '_' } else { c })
        .collect();
    if cleaned.trim().is_empty() { "file".to_string() } else { cleaned }
}

pub fn attachment_dir(home: &Path, spec_id: Ulid, attachment_id: Ulid) -> PathBuf {
    home.join("specs").join(spec_id.to_string()).join("context").join(attachment_id.to_string())
}

pub fn attachment_path(home: &Path, spec_id: Ulid, attachment_id: Ulid, filename: &str) -> PathBuf {
    attachment_dir(home, spec_id, attachment_id).join(filename)
}

pub fn is_utf8_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

pub fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

pub fn read_text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_components() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.md"), "c.md");
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
    }

    #[test]
    fn sanitize_handles_empty() {
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename("   "), "file");
    }

    #[test]
    fn sanitize_replaces_control_chars() {
        assert_eq!(sanitize_filename("a\nb.txt"), "a_b.txt");
    }

    #[test]
    fn utf8_detection_works() {
        assert!(is_utf8_text(b"hello"));
        assert!(is_utf8_text("héllo".as_bytes()));
        assert!(!is_utf8_text(&[0xff, 0xfe, 0x00, 0x01]));
    }

    #[test]
    fn write_and_read_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/c.txt");
        write_bytes(&path, b"hi").unwrap();
        let got = read_text(&path).unwrap();
        assert_eq!(got, "hi");
    }
}
