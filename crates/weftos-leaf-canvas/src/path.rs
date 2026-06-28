//! `Primitive::Path` → `Path2d` translation.
//!
//! v1 of the leaf wire format ships `Primitive::Path` + `PathCmd`
//! variants but the renderer defers rasterization to v1.1 (design doc
//! §11). Canvas2D, however, has `Path2d` natively — the moment the
//! capability flag flips on, this backend can land path rendering with
//! zero wire changes.
//!
//! For Phase D we prep the dispatch shape (this module exists, takes
//! the same arguments the v1.1 translator will take) and return
//! [`CanvasError::UnsupportedPrimitive`] so the renderer skips the
//! node with a logged warning. v1.1 swaps the body of [`draw_path`]
//! for a `PathCmd` → `Path2d` walk + `ctx.fill(path)` / `ctx.stroke(path)`.

use web_sys::CanvasRenderingContext2d;

use weftos_leaf_scene::PathCmd;

use crate::canvas_surface::CanvasError;

/// Draw a `Primitive::Path` (v1.1 — currently returns Unsupported).
///
/// `commands` is the wire-format path command sequence. The caller has
/// already applied the node transform; coordinates are local to the
/// node origin.
///
/// v1.1 implementation sketch:
///
/// ```ignore
/// let p = web_sys::Path2d::new()?;
/// for cmd in commands {
///     match cmd {
///         PathCmd::MoveTo { x, y } => p.move_to(from_px_q8(*x) as f64,
///                                               from_px_q8(*y) as f64),
///         PathCmd::LineTo { x, y } => p.line_to(...),
///         PathCmd::QuadTo { cx, cy, x, y } => p.quadratic_curve_to(...),
///         PathCmd::CubicTo { c1x, c1y, c2x, c2y, x, y } => p.bezier_curve_to(...),
///         PathCmd::Close => p.close_path(),
///     }
/// }
/// if let Some(fill) = style.fill   { ctx.set_fill_style_str(...); ctx.fill_with_path_2d(&p); }
/// if let Some(stroke)= style.stroke{ ctx.set_stroke_style_str(...); ctx.stroke_with_path(&p); }
/// ```
pub fn draw_path(
    _ctx: &CanvasRenderingContext2d,
    _commands: &[PathCmd],
    _style: &weftos_leaf_scene::Style,
) -> Result<(), CanvasError> {
    Err(CanvasError::UnsupportedPrimitive("Primitive::Path"))
}
