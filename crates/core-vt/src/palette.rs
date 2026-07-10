//! Terminal color resolution: ANSI colors -> sRGB, honoring runtime palette
//! overrides (OSC 4/10/11). Pure terminal semantics, no GPU dependency.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// Default foreground, sRGB.
pub const DEFAULT_FG: [u8; 3] = [0xd8, 0xd8, 0xd8];
/// Default background, sRGB.
pub const DEFAULT_BG: [u8; 3] = [0x12, 0x12, 0x16];

/// The 16 base ANSI colors (a restrained dark palette).
const BASE16: [[u8; 3]; 16] = [
    [0x1c, 0x1c, 0x1c], // black
    [0xcc, 0x66, 0x66], // red
    [0xb5, 0xbd, 0x68], // green
    [0xf0, 0xc6, 0x74], // yellow
    [0x81, 0xa2, 0xbe], // blue
    [0xb2, 0x94, 0xbb], // magenta
    [0x8a, 0xbe, 0xb7], // cyan
    [0xc5, 0xc8, 0xc6], // white
    [0x66, 0x66, 0x66], // bright black
    [0xd5, 0x4e, 0x53], // bright red
    [0xb9, 0xca, 0x4a], // bright green
    [0xe7, 0xc5, 0x47], // bright yellow
    [0x7a, 0xa6, 0xda], // bright blue
    [0xc3, 0x97, 0xd8], // bright magenta
    [0x70, 0xc0, 0xb1], // bright cyan
    [0xea, 0xea, 0xea], // bright white
];

/// Resolves an indexed color (0-255) to sRGB using the xterm-256 layout.
fn indexed(idx: u8) -> [u8; 3] {
    match idx {
        0..=15 => BASE16[idx as usize],
        16..=231 => {
            let idx = idx as usize - 16;
            let steps = [0u8, 95, 135, 175, 215, 255];
            [steps[idx / 36], steps[(idx % 36) / 6], steps[idx % 6]]
        }
        232..=255 => {
            let v = 8 + 10 * (idx - 232);
            [v, v, v]
        }
    }
}

fn named(name: NamedColor) -> [u8; 3] {
    match name {
        NamedColor::Foreground
        | NamedColor::BrightForeground
        | NamedColor::DimForeground
        | NamedColor::Cursor => DEFAULT_FG,
        NamedColor::Background => DEFAULT_BG,
        // Dim variants fall back to their normal counterparts; a proper dim
        // pass is applied by the renderer via the DIM cell flag.
        other => indexed(other as u8),
    }
}

/// Resolves a cell color to sRGB, preferring runtime palette overrides.
pub fn resolve_color(color: Color, overrides: &Colors) -> [u8; 3] {
    match color {
        Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
        Color::Named(name) => overrides[name]
            .map(|rgb| [rgb.r, rgb.g, rgb.b])
            .unwrap_or_else(|| named(name)),
        Color::Indexed(idx) => overrides[idx as usize]
            .map(|rgb| [rgb.r, rgb.g, rgb.b])
            .unwrap_or_else(|| indexed(idx)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::color::Colors;
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

    #[test]
    fn named_red_resolves() {
        let overrides = Colors::default();
        assert_eq!(
            resolve_color(Color::Named(NamedColor::Red), &overrides),
            [0xcc, 0x66, 0x66]
        );
    }

    #[test]
    fn truecolor_passes_through() {
        let overrides = Colors::default();
        let rgb = Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(resolve_color(Color::Spec(rgb), &overrides), [1, 2, 3]);
    }

    #[test]
    fn indexed_grayscale_ramp() {
        let overrides = Colors::default();
        // index 232 is the darkest gray step (8,8,8).
        assert_eq!(resolve_color(Color::Indexed(232), &overrides), [8, 8, 8]);
    }

    #[test]
    fn override_wins_over_default() {
        let mut overrides = Colors::default();
        overrides[NamedColor::Red as usize] = Some(Rgb {
            r: 10,
            g: 20,
            b: 30,
        });
        assert_eq!(
            resolve_color(Color::Named(NamedColor::Red), &overrides),
            [10, 20, 30]
        );
    }
}
