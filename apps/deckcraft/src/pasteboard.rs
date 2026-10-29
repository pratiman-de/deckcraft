//! System pasteboard: HTML rich text, a picture (TIFF on macOS, via arboard), and SVG markup from
//! vector apps rendered to a picture (via resvg).
//!
//! Pages keeps bold, italic and underline when the pasteboard carries HTML plus the same plain
//! words. A shape with no text is copied as an image by the UI; this module only reads a picture
//! back (Keynote's TIFF) as PNG bytes.

/// HTML currently on the pasteboard, when there is some.
pub fn html() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let html = cb.get().html().ok()?;
    let html = html.trim();
    if html.is_empty() { None } else { Some(html.to_string()) }
}

/// Write `html` and the plain alternative `text`. `false` when the pasteboard refuses it.
pub fn set_html(html: &str, text: &str) -> bool {
    let Ok(mut cb) = arboard::Clipboard::new() else { return false };
    cb.set_html(html, Some(text)).is_ok()
}

/// The pasteboard picture as PNG bytes (TIFF on macOS is decoded by arboard).
pub fn image_png() -> Option<Vec<u8>> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img = cb.get_image().ok()?;
    let width = u32::try_from(img.width).ok()?;
    let height = u32::try_from(img.height).ok()?;
    let rgba = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?;
    let mut out = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// Pixels per point for a pasted drawing: sharp on a 4K screen.
const SVG_PIXELS_PER_POINT: f64 = 4.0;
/// Longest side of a pasted drawing's picture, so a huge one stays a sane size.
const SVG_MAX_SIDE: f64 = 8192.0;
/// Most pixels a pasted drawing is rendered to: 16 megapixels, the same bound as a pasted
/// clipboard image, so a large drawing can't exhaust memory on its way to `insert.picture`.
const SVG_MAX_PIXELS: f64 = 16_000_000.0;

/// SVG markup rendered to PNG bytes at up to [`SVG_PIXELS_PER_POINT`], with its size in points.
///
/// Vector apps (VectorCraft, Illustrator, Inkscape, Figma…) put a copy's SVG markup on the
/// clipboard as its plain text, next to a small 1 px-per-point picture; rendering the markup keeps
/// a pasted drawing sharp. `None` when the markup doesn't parse or has no size.
pub fn svg_png(svg: &str) -> Option<(Vec<u8>, f64, f64)> {
    let mut options = resvg::usvg::Options {
        // Sizes come back in points: `width="1in"` is 72, not usvg's 96-dpi default.
        dpi: 72.0,
        // Pasted markup is untrusted: an `<image href="…">` must never read a local file, which
        // usvg's default resolver does. Data URLs still work; any other reference resolves to nothing.
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: resvg::usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(svg, &options).ok()?;
    let (w, h) = (f64::from(tree.size().width()), f64::from(tree.size().height()));
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let scale = SVG_PIXELS_PER_POINT.min(SVG_MAX_SIDE / w.max(h)).min((SVG_MAX_PIXELS / (w * h)).sqrt());
    let mut pixmap = resvg::tiny_skia::Pixmap::new((w * scale).ceil() as u32, (h * scale).ceil() as u32)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32), &mut pixmap.as_mut());
    Some((pixmap.encode_png().ok()?, w, h))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    /// A pasted drawing renders at 4 px per point and keeps its size in points; text that isn't
    /// SVG gives nothing.
    #[test]
    fn svg_markup_renders_sharp_with_its_size() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#ff0000"/></svg>"##;
        let (png, w, h) = super::svg_png(svg).expect("render");
        assert_eq!((w, h), (20.0, 10.0));
        let img = image::load_from_memory(&png).expect("decode").to_rgba8();
        assert_eq!(img.dimensions(), (80, 40));
        let p = img.get_pixel(40, 20);
        assert!(p[0] > 200 && p[1] < 40 && p[3] > 200, "{p:?}");
        assert!(super::svg_png("not svg").is_none());
    }

    /// A drawing past the pixel budget renders below 4 px per point, sizes in inches come back
    /// in points, and an `<image>` pointing at a local file draws nothing.
    #[test]
    fn svg_render_is_bounded_and_in_points() {
        let big = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4000" height="4000"><rect width="4000" height="4000" fill="#00ff00"/></svg>"##;
        let (png, w, h) = super::svg_png(big).expect("render");
        assert_eq!((w, h), (4000.0, 4000.0));
        let (pw, ph) = image::load_from_memory(&png).expect("decode").to_rgba8().dimensions();
        assert!(u64::from(pw) * u64::from(ph) <= 16_000_000, "{pw}x{ph}");
        let inch = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="0.5in"><rect width="100%" height="100%" fill="#0000ff"/></svg>"##;
        let (_, w, h) = super::svg_png(inch).expect("render");
        assert_eq!((w, h), (72.0, 36.0));
        let file = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image xlink:href="C:/Windows/notepad.exe" width="4" height="4"/></svg>"##;
        let (png, _, _) = super::svg_png(file).expect("render");
        let img = image::load_from_memory(&png).expect("decode").to_rgba8();
        assert_eq!(img.get_pixel(8, 8)[3], 0, "nothing drawn from a local path");
    }

    /// Puts HTML and a 1×1 picture on the macOS pasteboard and reads them back.
    /// Restores whatever plain text was there. Other platforms return immediately.
    #[test]
    fn html_and_a_picture_round_trip_on_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let previous = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok());
        let _restore = Restore(previous);
        assert!(super::set_html("<b>DeckCraft</b>", "DeckCraft"), "set html");
        let html = super::html().unwrap_or_default();
        assert!(html.contains("<b>") || html.contains("<B>"), "{html}");
        let text = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok()).unwrap_or_default();
        assert_eq!(text, "DeckCraft");
        let mut cb = arboard::Clipboard::new().expect("pasteboard");
        let img = arboard::ImageData { width: 1, height: 1, bytes: Cow::Owned(vec![255, 0, 0, 255]) };
        cb.set_image(img).expect("set image");
        drop(cb);
        let png = super::image_png().expect("png");
        let decoded = image::load_from_memory(&png).expect("decode").to_rgba8();
        let p = decoded.get_pixel(0, 0);
        assert!(p[0] > 200 && p[1] < 40 && p[3] > 200, "{p:?}");
    }

    /// Puts the previous plain text back, including when the test fails halfway.
    struct Restore(Option<String>);

    impl Drop for Restore {
        fn drop(&mut self) {
            let Ok(mut cb) = arboard::Clipboard::new() else { return };
            match self.0.take() {
                Some(t) => {
                    let _ = cb.set_text(t);
                }
                None => {
                    let _ = cb.clear();
                }
            }
        }
    }
}
