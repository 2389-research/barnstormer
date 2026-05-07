// ABOUTME: Disk storage helpers for context attachments — path layout, filename
// ABOUTME: sanitization, UTF-8 detection, and read/write helpers.

use std::io;
use std::path::{Path, PathBuf};
use ulid::Ulid;

pub fn sanitize_filename(raw: &str) -> String {
    // Strip any directory components, then replace control chars and known
    // path-dangerous chars with '_'. Empty result becomes "file".
    let base = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.trim().is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

pub fn attachment_dir(home: &Path, spec_id: Ulid, attachment_id: Ulid) -> PathBuf {
    home.join("specs")
        .join(spec_id.to_string())
        .join("context")
        .join(attachment_id.to_string())
}

pub fn attachment_path(home: &Path, spec_id: Ulid, attachment_id: Ulid, filename: &str) -> PathBuf {
    attachment_dir(home, spec_id, attachment_id).join(filename)
}

pub fn is_utf8_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

const WHITELIST_MIME: &[&str] = &[
    // Images
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/heic",
    "image/heif",
    "image/svg+xml",
    // Documents
    "application/pdf",
    // Audio
    "audio/wav",
    "audio/x-wav",
    "audio/mpeg",
    "audio/mp4",
    "audio/x-aiff",
    "audio/aiff",
    "audio/flac",
    // Video
    "video/mp4",
    "video/x-m4v",
    "video/quicktime",
    "video/webm",
    // Text — listed explicitly so callers checking `is_whitelisted_mime("text/plain")`
    // get the right answer; the `text/*` catch-all below also accepts them.
    "text/plain",
    "text/markdown",
    "text/html",
    "text/csv",
    "text/x-yaml",
    "application/json",
    "application/yaml",
];

pub fn is_whitelisted_mime(mime: &str) -> bool {
    let normalized = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    WHITELIST_MIME.iter().any(|w| *w == normalized) || normalized.starts_with("text/")
}

pub fn sniff_mime(bytes: &[u8], filename: &str) -> Option<String> {
    // SVG carve-out must run before infer::get, because infer recognizes the
    // `<?xml ...` prolog as generic `text/xml` and would shadow the SVG signal.
    if let Ok(s) = std::str::from_utf8(bytes) {
        let trimmed = s.trim_start();
        if trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && s.contains("<svg")) {
            return Some("image/svg+xml".to_string());
        }
    }
    // Magic-byte sniff for binaries
    if let Some(kind) = infer::get(bytes) {
        return Some(kind.mime_type().to_string());
    }
    // UTF-8 text fallback: pick mime from extension where useful
    if std::str::from_utf8(bytes).is_ok() {
        let ext = std::path::Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        return Some(match ext.as_str() {
            "md" | "markdown" => "text/markdown".to_string(),
            "csv" => "text/csv".to_string(),
            "yaml" | "yml" => "text/x-yaml".to_string(),
            "json" => "application/json".to_string(),
            "html" | "htm" => "text/html".to_string(),
            _ => "text/plain".to_string(),
        });
    }
    None
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

    #[test]
    fn sniff_mime_detects_png() {
        let bytes = include_bytes!("../tests/fixtures/tiny.png");
        assert_eq!(sniff_mime(bytes, "ignored.bin").as_deref(), Some("image/png"));
    }

    #[test]
    fn sniff_mime_detects_pdf() {
        let bytes = include_bytes!("../tests/fixtures/tiny.pdf");
        assert_eq!(
            sniff_mime(bytes, "ignored.bin").as_deref(),
            Some("application/pdf")
        );
    }

    #[test]
    fn sniff_mime_detects_svg_via_content() {
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"></svg>";
        assert_eq!(
            sniff_mime(bytes, "x.svg").as_deref(),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn sniff_mime_detects_svg_with_xml_decl() {
        let bytes = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        assert_eq!(
            sniff_mime(bytes, "x.svg").as_deref(),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn sniff_mime_falls_back_to_text_for_utf8() {
        let bytes = b"# heading\n\nplain markdown";
        assert_eq!(
            sniff_mime(bytes, "x.md").as_deref(),
            Some("text/markdown")
        );
    }

    #[test]
    fn sniff_mime_returns_none_for_unrecognized_binary() {
        let bytes = &[0xff, 0xfe, 0x00, 0x01, 0x02, 0x03];
        assert!(sniff_mime(bytes, "x.bin").is_none());
    }

    #[test]
    fn whitelist_accepts_supported_kinds() {
        assert!(is_whitelisted_mime("image/png"));
        assert!(is_whitelisted_mime("image/heic"));
        assert!(is_whitelisted_mime("image/svg+xml"));
        assert!(is_whitelisted_mime("application/pdf"));
        assert!(is_whitelisted_mime("audio/mpeg"));
        assert!(is_whitelisted_mime("audio/mp4")); // M4A
        assert!(is_whitelisted_mime("audio/x-aiff"));
        assert!(is_whitelisted_mime("audio/flac"));
        assert!(is_whitelisted_mime("video/mp4"));
        assert!(is_whitelisted_mime("video/x-m4v"));
        assert!(is_whitelisted_mime("video/quicktime"));
        assert!(is_whitelisted_mime("text/plain"));
        assert!(is_whitelisted_mime("text/markdown"));
    }

    #[test]
    fn whitelist_rejects_dangerous_kinds() {
        assert!(!is_whitelisted_mime("application/x-msdownload"));
        assert!(!is_whitelisted_mime("application/zip"));
        assert!(!is_whitelisted_mime("application/octet-stream"));
    }
}
