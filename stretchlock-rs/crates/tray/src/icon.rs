//! Procedurally drawn 32x32 tray icon: a blue rounded square with an
//! "S" glyph, generated as RGBA pixels without any image decoding crate.

const SIZE: usize = 32;

const BG: [u8; 4] = [0x25, 0x63, 0xEB, 0xFF];
const GLYPH: [u8; 4] = [0xF5, 0xF7, 0xFA, 0xFF];

/// Background pixel inside the rounded square (corners clipped).
fn background(x: usize, y: usize) -> bool {
    let edge = |v: usize| v < 4 || v >= SIZE - 4;
    !(edge(x) && edge(y))
}

/// "S" glyph built from axis-aligned strokes.
fn glyph(x: usize, y: usize) -> bool {
    let top_bar = (7..10).contains(&y) && (9..24).contains(&x);
    let upper_left = (10..14).contains(&y) && (7..10).contains(&x);
    let middle_bar = (14..17).contains(&y) && (7..25).contains(&x);
    let lower_right = (17..21).contains(&y) && (22..25).contains(&x);
    let bottom_bar = (21..24).contains(&y) && (8..23).contains(&x);
    top_bar || upper_left || middle_bar || lower_right || bottom_bar
}

pub(crate) fn rgba() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let color = if glyph(x, y) {
                GLYPH
            } else if background(x, y) {
                BG
            } else {
                [0, 0, 0, 0]
            };
            pixels.extend_from_slice(&color);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_32x32_rgba() {
        let data = rgba();
        assert_eq!(data.len(), 32 * 32 * 4);
    }

    #[test]
    fn icon_has_glyph_and_background() {
        let data = rgba();
        assert!(data.iter().any(|&b| b == 0x25));
        assert!(data.iter().any(|&b| b == 0xF5));
        // corners stay transparent
        assert_eq!(&data[0..4], &[0, 0, 0, 0]);
    }
}
