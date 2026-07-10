//! Renderer-specific color helpers. Palette resolution lives in `core_vt`.

pub use core_vt::{DEFAULT_BG, DEFAULT_FG, resolve_color as resolve};

/// Converts an sRGB byte to a linear float for writing into an sRGB target.
pub fn srgb8_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}
