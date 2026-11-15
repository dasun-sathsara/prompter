//! Colours and metrics. One dark palette, tuned for text over live video.

use gpui::{Hsla, hsla};

/// Corner radius of the panel, in points.
pub const RADIUS: f32 = 16.0;

/// Height of the control strip above the script.
pub const STRIP: f32 = 34.0;

/// The panel's fill, at the user's chosen opacity.
pub fn backdrop(alpha: f32) -> Hsla {
    hsla(220.0 / 360.0, 0.16, 0.055, alpha)
}

/// The script text. Always opaque: the panel fades, the words do not.
pub fn text() -> Hsla {
    hsla(210.0 / 360.0, 0.14, 0.975, 1.0)
}

/// Secondary marks (rules, paragraph numbers, hints).
pub fn faint(alpha: f32) -> Hsla {
    hsla(210.0 / 360.0, 0.12, 0.98, alpha)
}

/// A one pixel edge so the panel reads as a surface over light content,
/// whatever its own opacity.
pub fn border() -> Hsla {
    hsla(210.0 / 360.0, 0.18, 1.0, 0.12)
}

/// Interactive tint behind a hovered control.
pub fn control_hover(alpha: f32) -> Hsla {
    hsla(210.0 / 360.0, 0.10, 1.0, 0.10 * alpha)
}

/// The single accent, used for the reading line and the active control.
pub fn accent(alpha: f32) -> Hsla {
    hsla(26.0 / 360.0, 0.90, 0.66, alpha)
}
