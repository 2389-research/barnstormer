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
    "audio/x-flac",
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

/// Map a mime type to the mux `MediaKind` variant the LLM client expects,
/// or `None` for text and unknown formats. Used by the summarizer dispatch
/// to decide whether to send a media block.
pub fn media_kind_from_mime(mime: &str) -> Option<mux::llm::MediaKind> {
    use mux::llm::MediaKind;
    let normalized = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    if normalized.starts_with("image/") {
        Some(MediaKind::Image)
    } else if normalized == "application/pdf" {
        Some(MediaKind::Document)
    } else if normalized.starts_with("audio/") {
        Some(MediaKind::Audio)
    } else if normalized.starts_with("video/") {
        Some(MediaKind::Video)
    } else {
        None
    }
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

/// Build a [`crate::summarizer::SummarizerInput`] from disk for the given
/// attachment, branching on its stored mime type. Used by upload, notes-update,
/// and manual resummarize flows so disk→input dispatch lives in one place.
///
/// - `image/svg+xml` → `Svg { markup, raster_path }` (raster_path Some if the
///   sibling `rasterized.png` exists; None means raster cache is missing and
///   the summarizer will degrade to markup-only)
/// - Other media (image/audio/video/PDF) → `Media { kind, mime, path }` with
///   `MediaSource::Path` so mux reads the file at request time
/// - Anything else (text/*, application/json, etc.) → `Text { content }` with
///   the file read as UTF-8
pub fn build_summarizer_input(
    home: &Path,
    spec_id: Ulid,
    attachment: &barnstormer_core::state::ContextAttachment,
) -> anyhow::Result<crate::summarizer::SummarizerInput> {
    let dir = attachment_dir(home, spec_id, attachment.attachment_id);
    let path = dir.join(&attachment.filename);
    // Normalize mime: strip parameters (e.g. "; charset=utf-8"), trim, lowercase.
    // Same convention as is_whitelisted_mime / media_kind_from_mime.
    let raw = attachment.mime_type.to_ascii_lowercase();
    let mime = raw.split(';').next().unwrap_or(&raw).trim().to_string();

    if mime == "image/svg+xml" {
        let markup = std::fs::read_to_string(&path)?;
        let raster = dir.join("rasterized.png");
        let raster_path = if raster.exists() { Some(raster) } else { None };
        return Ok(crate::summarizer::SummarizerInput::Svg {
            markup,
            raster_path,
        });
    }
    if let Some(kind) = media_kind_from_mime(&mime) {
        return Ok(crate::summarizer::SummarizerInput::Media { kind, mime, path });
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(crate::summarizer::SummarizerInput::Text { content })
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
        assert_eq!(
            sniff_mime(bytes, "ignored.bin").as_deref(),
            Some("image/png")
        );
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
        assert_eq!(sniff_mime(bytes, "x.svg").as_deref(), Some("image/svg+xml"));
    }

    #[test]
    fn sniff_mime_detects_svg_with_xml_decl() {
        let bytes = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>";
        assert_eq!(sniff_mime(bytes, "x.svg").as_deref(), Some("image/svg+xml"));
    }

    #[test]
    fn sniff_mime_falls_back_to_text_for_utf8() {
        let bytes = b"# heading\n\nplain markdown";
        assert_eq!(sniff_mime(bytes, "x.md").as_deref(), Some("text/markdown"));
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

    #[test]
    fn sniffed_fixtures_pass_whitelist() {
        // Catches the class of bug where infer emits one mime spelling but the
        // whitelist only contains another (e.g. audio/x-flac vs audio/flac).
        let cases: &[(&[u8], &str)] = &[
            (include_bytes!("../tests/fixtures/tiny.png"), "tiny.png"),
            (include_bytes!("../tests/fixtures/tiny.pdf"), "tiny.pdf"),
            (include_bytes!("../tests/fixtures/tiny.wav"), "tiny.wav"),
            (include_bytes!("../tests/fixtures/tiny.mp4"), "tiny.mp4"),
        ];
        for (bytes, name) in cases {
            let mime = sniff_mime(bytes, name).expect(name);
            assert!(
                is_whitelisted_mime(&mime),
                "fixture {name} sniffed as {mime} but whitelist rejected it"
            );
        }
    }

    #[test]
    fn media_kind_for_image_mimes() {
        use mux::llm::MediaKind;
        assert_eq!(media_kind_from_mime("image/png"), Some(MediaKind::Image));
        assert_eq!(media_kind_from_mime("image/heic"), Some(MediaKind::Image));
        assert_eq!(
            media_kind_from_mime("image/svg+xml"),
            Some(MediaKind::Image)
        );
    }

    #[test]
    fn media_kind_for_pdf() {
        use mux::llm::MediaKind;
        assert_eq!(
            media_kind_from_mime("application/pdf"),
            Some(MediaKind::Document)
        );
    }

    #[test]
    fn media_kind_for_audio_mimes() {
        use mux::llm::MediaKind;
        assert_eq!(media_kind_from_mime("audio/mpeg"), Some(MediaKind::Audio));
        assert_eq!(media_kind_from_mime("audio/mp4"), Some(MediaKind::Audio));
        assert_eq!(media_kind_from_mime("audio/x-aiff"), Some(MediaKind::Audio));
        assert_eq!(media_kind_from_mime("audio/x-flac"), Some(MediaKind::Audio));
    }

    #[test]
    fn media_kind_for_video_mimes() {
        use mux::llm::MediaKind;
        assert_eq!(media_kind_from_mime("video/mp4"), Some(MediaKind::Video));
        assert_eq!(media_kind_from_mime("video/x-m4v"), Some(MediaKind::Video));
    }

    #[test]
    fn media_kind_returns_none_for_text() {
        assert_eq!(media_kind_from_mime("text/plain"), None);
        assert_eq!(media_kind_from_mime("text/markdown"), None);
    }

    #[test]
    fn media_kind_normalizes_input() {
        use mux::llm::MediaKind;
        // Strips parameters and lowercases — same convention as is_whitelisted_mime.
        assert_eq!(
            media_kind_from_mime("IMAGE/PNG; charset=utf-8"),
            Some(MediaKind::Image)
        );
    }

    #[test]
    fn flac_magic_bytes_pass_whitelist() {
        // FLAC magic: "fLaC" + minimal stream info block
        let bytes = b"fLaC\x80\x00\x00\x22\x10\x00\x10\x00\x00\x00\x00\x00\x00\x00\x00\x00\x0a\xc4\x42\xf0\x00\x00\x00\x00";
        let mime = sniff_mime(bytes, "x.flac").expect("infer should detect FLAC");
        assert!(
            is_whitelisted_mime(&mime),
            "FLAC sniffed as {mime} but whitelist rejected it"
        );
    }

    #[test]
    fn build_input_for_text_attachment() {
        use crate::summarizer::SummarizerInput;
        use barnstormer_core::state::ContextAttachment;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let spec_id = ulid::Ulid::new();
        let att_id = ulid::Ulid::new();
        let path = attachment_path(home, spec_id, att_id, "x.md");
        write_bytes(&path, b"hi").unwrap();
        let att = ContextAttachment {
            attachment_id: att_id,
            filename: "x.md".into(),
            mime_type: "text/markdown".into(),
            size_bytes: 2,
            summary: None,
            user_notes: None,
            added_at: chrono::Utc::now(),
            removed: false,
            summary_error: None,
        };
        let input = build_summarizer_input(home, spec_id, &att).unwrap();
        match input {
            SummarizerInput::Text { content } => assert_eq!(content, "hi"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn build_input_for_image_attachment_uses_path() {
        use crate::summarizer::SummarizerInput;
        use barnstormer_core::state::ContextAttachment;
        use mux::llm::MediaKind;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let spec_id = ulid::Ulid::new();
        let att_id = ulid::Ulid::new();
        let path = attachment_path(home, spec_id, att_id, "x.png");
        write_bytes(&path, &[0x89, 0x50, 0x4E, 0x47]).unwrap();
        let att = ContextAttachment {
            attachment_id: att_id,
            filename: "x.png".into(),
            mime_type: "image/png".into(),
            size_bytes: 4,
            summary: None,
            user_notes: None,
            added_at: chrono::Utc::now(),
            removed: false,
            summary_error: None,
        };
        let input = build_summarizer_input(home, spec_id, &att).unwrap();
        match input {
            SummarizerInput::Media { kind, .. } => assert_eq!(kind, MediaKind::Image),
            other => panic!("expected Media, got {other:?}"),
        }
    }

    #[test]
    fn build_input_for_svg_uses_dual_form_when_raster_present() {
        use crate::summarizer::SummarizerInput;
        use barnstormer_core::state::ContextAttachment;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let spec_id = ulid::Ulid::new();
        let att_id = ulid::Ulid::new();
        let dir = attachment_dir(home, spec_id, att_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.svg"), b"<svg></svg>").unwrap();
        std::fs::write(dir.join("rasterized.png"), [0x89, 0x50, 0x4E, 0x47]).unwrap();
        let att = ContextAttachment {
            attachment_id: att_id,
            filename: "x.svg".into(),
            mime_type: "image/svg+xml".into(),
            size_bytes: 11,
            summary: None,
            user_notes: None,
            added_at: chrono::Utc::now(),
            removed: false,
            summary_error: None,
        };
        let input = build_summarizer_input(home, spec_id, &att).unwrap();
        match input {
            SummarizerInput::Svg {
                raster_path: Some(_),
                ..
            } => {}
            other => panic!("expected Svg with raster, got {other:?}"),
        }
    }

    #[test]
    fn build_input_for_svg_falls_back_when_raster_missing() {
        use crate::summarizer::SummarizerInput;
        use barnstormer_core::state::ContextAttachment;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let spec_id = ulid::Ulid::new();
        let att_id = ulid::Ulid::new();
        let dir = attachment_dir(home, spec_id, att_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.svg"), b"<svg></svg>").unwrap();
        let att = ContextAttachment {
            attachment_id: att_id,
            filename: "x.svg".into(),
            mime_type: "image/svg+xml".into(),
            size_bytes: 11,
            summary: None,
            user_notes: None,
            added_at: chrono::Utc::now(),
            removed: false,
            summary_error: None,
        };
        let input = build_summarizer_input(home, spec_id, &att).unwrap();
        match input {
            SummarizerInput::Svg {
                raster_path: None, ..
            } => {}
            other => panic!("expected Svg without raster, got {other:?}"),
        }
    }

    #[test]
    fn build_input_for_audio_attachment() {
        use crate::summarizer::SummarizerInput;
        use barnstormer_core::state::ContextAttachment;
        use mux::llm::MediaKind;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let spec_id = ulid::Ulid::new();
        let att_id = ulid::Ulid::new();
        let path = attachment_path(home, spec_id, att_id, "x.mp3");
        write_bytes(&path, &[0x49, 0x44, 0x33]).unwrap();
        let att = ContextAttachment {
            attachment_id: att_id,
            filename: "x.mp3".into(),
            mime_type: "audio/mpeg".into(),
            size_bytes: 3,
            summary: None,
            user_notes: None,
            added_at: chrono::Utc::now(),
            removed: false,
            summary_error: None,
        };
        let input = build_summarizer_input(home, spec_id, &att).unwrap();
        match input {
            SummarizerInput::Media { kind, .. } => assert_eq!(kind, MediaKind::Audio),
            other => panic!("expected Media (Audio), got {other:?}"),
        }
    }
}
