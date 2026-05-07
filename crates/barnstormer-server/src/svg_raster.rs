// ABOUTME: Pure-Rust SVG → PNG rasterization helper used to feed multimodal
// ABOUTME: summarizers a rendered visual alongside the original SVG markup.

use anyhow::{Context, Result};

/// Rasterize an SVG document to PNG bytes.
///
/// Returns the encoded PNG on success. Returns an error on malformed SVG,
/// allocation failure, or PNG encoding failure — callers degrade to
/// markup-only summarization in that case.
pub fn rasterize_svg(markup: &str) -> Result<Vec<u8>> {
    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_str(markup, &opts).context("failed to parse SVG markup")?;

    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .context("failed to allocate pixmap for SVG rasterization")?;

    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .context("failed to encode rasterized SVG as PNG")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
    <rect width="16" height="16" fill="red"/>
</svg>"#;

    #[test]
    fn rasterize_emits_png_magic_bytes() {
        let png = rasterize_svg(TINY_SVG).unwrap();
        // PNG magic: \x89 P N G \r \n \x1a \n
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn rasterize_malformed_returns_error() {
        let err = rasterize_svg("<svg unterminated").unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("svg") || msg.contains("parse"),
            "expected svg/parse error message, got: {msg}"
        );
    }
}
