// ABOUTME: Pure-Rust SVG → PNG rasterization helper used to feed multimodal
// ABOUTME: summarizers a rendered visual alongside the original SVG markup.

use anyhow::{Context, Result};

/// Hard cap on rasterization dimensions. A malicious SVG can declare an
/// arbitrarily large `viewBox` (e.g. 1_000_000 × 1_000_000), and `Pixmap::new`
/// would dutifully try to allocate ~4 bytes per pixel — multi-GB allocations
/// from a single upload. We refuse anything past this cap before allocating;
/// callers degrade to markup-only summarization.
const MAX_RASTER_DIM: u32 = 4096;

/// Rasterize an SVG document to PNG bytes.
///
/// Returns the encoded PNG on success. Returns an error on malformed SVG,
/// dimensions that exceed `MAX_RASTER_DIM`, allocation failure, or PNG
/// encoding failure — callers degrade to markup-only summarization in that
/// case.
pub fn rasterize_svg(markup: &str) -> Result<Vec<u8>> {
    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_str(markup, &opts).context("failed to parse SVG markup")?;

    let size = tree.size().to_int_size();
    if size.width() > MAX_RASTER_DIM || size.height() > MAX_RASTER_DIM {
        anyhow::bail!(
            "SVG dimensions {}x{} exceed raster cap ({MAX_RASTER_DIM}x{MAX_RASTER_DIM})",
            size.width(),
            size.height()
        );
    }
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .context("failed to allocate pixmap for SVG rasterization")?;

    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );

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

    #[test]
    fn rasterize_rejects_oversized_dimensions() {
        // A hostile SVG declaring a 100k × 100k viewBox would otherwise trigger
        // a multi-GB Pixmap allocation. The dimension cap must reject it before
        // any allocation occurs.
        let oversize = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100000 100000"
            width="100000" height="100000"><rect width="100" height="100" fill="red"/></svg>"#;
        let err = rasterize_svg(oversize).unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("exceed") || msg.contains("dimensions") || msg.contains("cap"),
            "expected dimension-cap error, got: {msg}"
        );
    }
}
