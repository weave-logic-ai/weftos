//! `Primitive::Text` → `ctx.fillText` with a CSS font string.
//!
//! v1 honours [`FontFace::Builtin`] only; the two built-in mono fonts
//! map to portable CSS families that ship with every browser:
//!
//! | Builtin       | CSS family            | Notes                       |
//! |---------------|-----------------------|-----------------------------|
//! | `Mono6x10`    | `monospace`           | The browser picks a system mono |
//! | `Mono10x20`   | `monospace`           | Same family, size carries shape |
//!
//! We deliberately don't try to mirror the embedded mono atlases pixel-
//! for-pixel — the browser's native mono font renders cleaner than the
//! 6×10 / 10×20 atlases, and the canvas declares `ANTIALIASED` /
//! `SUBPIXEL` so the renderer doesn't expect a bitmap-fontish output.
//!
//! `FontFace::Vector` and `FontFace::Inline` are deferred to v1.1 — the
//! canvas declares `!VECTOR_FONTS` so the renderer will skip those
//! nodes before they reach us. We return `UnsupportedFont` if one
//! arrives anyway (e.g., a future test that runs the renderer with
//! `VECTOR_FONTS` synthetically enabled).

use web_sys::CanvasRenderingContext2d;

use weftos_leaf_scene::{FontFace, Rgba, Style};

use crate::canvas_surface::{css_color, CanvasError};

/// Draw a `Primitive::Text` at the current node origin.
///
/// The caller has already applied the node transform; we draw at
/// `(0, 0)`. Canvas2D's default `textBaseline` is `alphabetic` — but
/// the embedded sim uses `top` (so `transform.y` matches the visual
/// top of the glyph row). We set `top` here to keep the two backends
/// pixel-comparable for the same scene.
pub fn draw_text(
    ctx: &CanvasRenderingContext2d,
    content: &str,
    face: &FontFace,
    size_q8: u16,
    weight: u16,
    style: &Style,
) -> Result<(), CanvasError> {
    let css_font = css_font_string(face, size_q8, weight)?;
    ctx.set_font(&css_font);
    // `top` baseline matches `embedded-graphics` Baseline::Top so the
    // sim and canvas frames overlay cleanly in side-by-side previews.
    ctx.set_text_baseline("top");

    let color = style.fill.unwrap_or(Rgba::WHITE);
    ctx.set_fill_style_str(&css_color(color));
    // `fill_text_with_max_width` is the long-form name; the simple
    // signature suffices here.
    ctx.fill_text(content, 0.0, 0.0).map_err(|_| {
        // Canvas2D's `fillText` only fails if the context is in a
        // pathological state (e.g., a synchronous task killed mid-
        // frame). Treat as "rendering glitch" and return Unsupported
        // so the renderer logs + continues with the next primitive.
        CanvasError::UnsupportedFont("fillText failed")
    })?;
    Ok(())
}

/// Build a CSS `font` shorthand string from the wire's `FontFace`,
/// `size_q8`, and `weight`.
///
/// CSS font shorthand is `[style] [variant] [weight] size family`.
/// We emit `<weight> <size>px <family>`.
fn css_font_string(
    face: &FontFace,
    size_q8: u16,
    weight: u16,
) -> Result<String, CanvasError> {
    // Q8.8 → f64 px. Canvas2D's `font` parser tolerates fractional
    // pixel sizes; we preserve the sub-pixel value so the `SUBPIXEL`
    // capability carries through to glyph metrics.
    let size_px = f64::from(size_q8) / 256.0;
    // Clamp weight to the CSS-legal 100..=900 range. The wire allows
    // any u16, but Canvas2D parses out-of-range weights as `normal`.
    let weight = weight.clamp(100, 900);

    let family = match face {
        // The two built-ins both map to `monospace` (the size carries
        // the visual shape difference). v1.1 swaps these for the
        // bundled atlas-derived families once we wire `@font-face`.
        FontFace::Builtin(_) => "monospace",
        FontFace::Vector { family, .. } => {
            // The renderer's capability check should skip vector
            // fonts before we get here (canvas declares
            // `!VECTOR_FONTS`); if one does sneak through, refuse
            // cleanly with the family name in the error.
            let _ = family;
            return Err(CanvasError::UnsupportedFont("FontFace::Vector"));
        }
        FontFace::Inline { .. } => {
            return Err(CanvasError::UnsupportedFont("FontFace::Inline"));
        }
    };

    // `format!` allocates a `String` per text draw. For v1 this is
    // fine — the surface caches nothing and the renderer issues at
    // most one `draw_primitive` per visible text node per frame. v1.1
    // can hoist the format into a small `HashMap` keyed on
    // `(face_id, size_q8, weight)` if profiling shows it.
    Ok(format!("{weight} {size_px}px {family}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftos_leaf_scene::{BuiltinFont, FontStyle};

    #[test]
    fn builtin_mono_maps_to_monospace() {
        let s = css_font_string(
            &FontFace::Builtin(BuiltinFont::Mono10x20),
            20 << 8,
            400,
        )
        .unwrap();
        // "400 20px monospace"
        assert!(s.contains("400"));
        assert!(s.contains("20"));
        assert!(s.contains("monospace"));
    }

    #[test]
    fn weight_is_clamped_to_css_range() {
        let s = css_font_string(
            &FontFace::Builtin(BuiltinFont::Mono6x10),
            10 << 8,
            9999,
        )
        .unwrap();
        // 9999 clamps to 900.
        assert!(s.contains("900"));
    }

    #[test]
    fn vector_font_returns_unsupported() {
        let r = css_font_string(
            &FontFace::Vector {
                family: "Inter".to_string(),
                style: FontStyle::Normal,
            },
            10 << 8,
            400,
        );
        assert!(matches!(r, Err(CanvasError::UnsupportedFont(_))));
    }

    #[test]
    fn inline_font_returns_unsupported() {
        let r = css_font_string(&FontFace::Inline { id: 7 }, 10 << 8, 400);
        assert!(matches!(r, Err(CanvasError::UnsupportedFont(_))));
    }
}
