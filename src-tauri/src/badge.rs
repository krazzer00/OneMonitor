//! Tray icon with a number on it (balance or remaining limit), drawn with a
//! tiny built-in 5x7 pixel font. Rendered at 64x64 and box-downsampled to
//! 32x32 so Windows can scale it down to 16/20/24 px and keep it legible.

use crate::model::Health;

const S: usize = 64;
const OUT: usize = 32;

/// 5x7 glyphs, one byte per row (low 5 bits, MSB = leftmost column), plus width.
fn glyph(c: char) -> Option<([u8; 7], usize)> {
    Some(match c {
        '0' => ([0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], 5),
        '1' => ([0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], 5),
        '2' => ([0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], 5),
        '3' => ([0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], 5),
        '4' => ([0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], 5),
        '5' => ([0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], 5),
        '6' => ([0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], 5),
        '7' => ([0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], 5),
        '8' => ([0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], 5),
        '9' => ([0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], 5),
        // narrow glyphs are stored left-aligned in the 5-bit row
        '.' => ([0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18], 2),
        'k' => ([0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12], 4),
        'M' => ([0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11], 5),
        '-' => ([0x00, 0x00, 0x00, 0x1E, 0x00, 0x00, 0x00], 4),
        '!' => ([0x10, 0x10, 0x10, 0x10, 0x10, 0x00, 0x10], 1),
        _ => return None,
    })
}

/// Compact text for a dollar amount: 9.5, 317, 1.3k, 13k, 1.2M.
pub fn money_text(v: f64) -> String {
    let v = v.max(0.0);
    if v < 10.0 {
        format!("{v:.1}")
    } else if v < 1000.0 {
        format!("{v:.0}")
    } else if v < 10_000.0 {
        format!("{:.1}k", v / 1000.0)
    } else if v < 1_000_000.0 {
        format!("{:.0}k", v / 1000.0)
    } else {
        format!("{:.1}M", v / 1_000_000.0)
    }
}

fn color(h: Option<Health>) -> [u8; 3] {
    match h {
        Some(Health::Ok) => [22, 163, 112],
        Some(Health::Warn) => [217, 150, 18],
        Some(Health::Error) => [220, 68, 68],
        _ => [100, 110, 128],
    }
}

/// Renders a 32x32 RGBA badge with `text` on a state-coloured rounded square.
pub fn render(text: &str, health: Option<Health>) -> Vec<u8> {
    let mut px = vec![0u8; S * S * 4];
    let [r, g, b] = color(health);

    // rounded square background
    let rad = 14.0f32;
    for y in 0..S {
        for x in 0..S {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = fx.clamp(rad, S as f32 - rad);
            let cy = fy.clamp(rad, S as f32 - rad);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            if d <= rad {
                let i = (y * S + x) * 4;
                px[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }

    // pick the largest integer scale that fits
    let glyphs: Vec<([u8; 7], usize)> = text.chars().filter_map(glyph).collect();
    if glyphs.is_empty() {
        return downsample(&px);
    }
    let width_at = |s: usize| -> usize {
        glyphs.iter().map(|(_, w)| w * s).sum::<usize>() + (glyphs.len() - 1) * s
    };
    let scale = (2..=6)
        .rev()
        .find(|&s| width_at(s) <= S - 6 && 7 * s <= S - 14)
        .unwrap_or(2);
    let total_w = width_at(scale);
    let mut x0 = (S.saturating_sub(total_w)) / 2;
    let y0 = (S - 7 * scale) / 2;
    for (rows, w) in &glyphs {
        for (ry, row) in rows.iter().enumerate() {
            for cx in 0..*w {
                if row & (0x10 >> cx) == 0 {
                    continue;
                }
                // bold: each dot is half a cell wider and taller, so strokes stay
                // solid when Windows scales the icon down to 16 px
                let bold = scale + scale / 2;
                for dy in 0..bold {
                    for dx in 0..bold {
                        let x = x0 + cx * scale + dx;
                        let y = y0 + ry * scale + dy;
                        if x < S && y < S {
                            let i = (y * S + x) * 4;
                            px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                        }
                    }
                }
            }
        }
        x0 += (w + 1) * scale;
    }
    downsample(&px)
}

/// 2x2 box filter with premultiplied alpha.
fn downsample(px: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; OUT * OUT * 4];
    for y in 0..OUT {
        for x in 0..OUT {
            let mut acc = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y * 2 + dy) * S + (x * 2 + dx)) * 4;
                let a = px[i + 3] as u32;
                acc[0] += px[i] as u32 * a;
                acc[1] += px[i + 1] as u32 * a;
                acc[2] += px[i + 2] as u32 * a;
                acc[3] += a;
            }
            let o = (y * OUT + x) * 4;
            if acc[3] > 0 {
                out[o] = (acc[0] / acc[3]) as u8;
                out[o + 1] = (acc[1] / acc[3]) as u8;
                out[o + 2] = (acc[2] / acc[3]) as u8;
                out[o + 3] = (acc[3] / 4) as u8;
            }
        }
    }
    out
}

pub const SIZE: u32 = OUT as u32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_formats() {
        assert_eq!(money_text(0.85), "0.8");
        assert_eq!(money_text(54.77), "55");
        assert_eq!(money_text(1317.05), "1.3k");
        assert_eq!(money_text(25_400.0), "25k");
    }

    #[test]
    fn renders_text_inside_the_badge() {
        let px = render("1.3k", Some(Health::Ok));
        assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);
        // some pure-ish white text pixels in the middle row band
        let white = px
            .chunks(4)
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200 && p[3] > 200)
            .count();
        assert!(white > 20, "{white}");
        // corners stay transparent (rounded)
        assert_eq!(px[3], 0);
    }

    #[test]
    fn unknown_chars_are_ignored() {
        render("??", None);
        render("", Some(Health::Error));
    }
}
