//! Renders terminal screen state to a PNG image using `fontdue` for glyph
//! rasterization and `tiny-skia` for compositing.

use anyhow::{Context, Result};
use fontdue::{Font, FontSettings};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

// ── Embedded fonts (Cousine – Apache 2.0) ──────────────────────────────────

const FONT_REGULAR: &[u8] = include_bytes!("../assets/Cousine-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/Cousine-Bold.ttf");

// ── Theme ──────────────────────────────────────────────────────────────────

/// RGBA color theme for terminal rendering.
pub struct Theme {
    pub name: &'static str,
    pub background: [u8; 4],
    pub foreground: [u8; 4],
    /// Standard ANSI 16-color palette (indices 0–15).
    pub colors: [[u8; 4]; 16],
}

/// Returns a built-in theme by name. Unknown names fall back to `"dark"`.
pub fn get_theme(name: &str) -> Theme {
    match name {
        "light" => Theme {
            name: "light",
            background: [255, 255, 255, 255],
            foreground: [30, 30, 30, 255],
            colors: LIGHT_PALETTE,
        },
        _ => Theme {
            name: "dark",
            background: [30, 30, 30, 255],
            foreground: [204, 204, 204, 255],
            colors: DARK_PALETTE,
        },
    }
}

// Dark theme – One Dark–inspired palette
const DARK_PALETTE: [[u8; 4]; 16] = [
    [40, 44, 52, 255],     // 0  black
    [224, 108, 117, 255],  // 1  red
    [152, 195, 121, 255],  // 2  green
    [229, 192, 123, 255],  // 3  yellow
    [97, 175, 239, 255],   // 4  blue
    [198, 120, 221, 255],  // 5  magenta
    [86, 182, 194, 255],   // 6  cyan
    [171, 178, 191, 255],  // 7  white
    [92, 99, 112, 255],    // 8  bright black
    [224, 108, 117, 255],  // 9  bright red
    [152, 195, 121, 255],  // 10 bright green
    [229, 192, 123, 255],  // 11 bright yellow
    [97, 175, 239, 255],   // 12 bright blue
    [198, 120, 221, 255],  // 13 bright magenta
    [86, 182, 194, 255],   // 14 bright cyan
    [220, 223, 228, 255],  // 15 bright white
];

// Light theme – VS Code light–inspired palette
const LIGHT_PALETTE: [[u8; 4]; 16] = [
    [0, 0, 0, 255],        // 0  black
    [205, 49, 49, 255],    // 1  red
    [0, 135, 0, 255],      // 2  green
    [128, 128, 0, 255],    // 3  yellow
    [0, 0, 200, 255],      // 4  blue
    [188, 63, 188, 255],   // 5  magenta
    [17, 168, 205, 255],   // 6  cyan
    [229, 229, 229, 255],  // 7  white
    [102, 102, 102, 255],  // 8  bright black
    [241, 76, 76, 255],    // 9  bright red
    [35, 209, 35, 255],    // 10 bright green
    [245, 245, 67, 255],   // 11 bright yellow
    [59, 142, 234, 255],   // 12 bright blue
    [214, 112, 214, 255],  // 13 bright magenta
    [41, 184, 219, 255],   // 14 bright cyan
    [255, 255, 255, 255],  // 15 bright white
];

// ── Helpers ────────────────────────────────────────────────────────────────

/// Convert an ANSI 256-color index to RGBA using the theme's 16-color palette
/// and the standard 6×6×6 color cube + greyscale ramp for indices 16–255.
fn ansi256_to_rgba(idx: u8, theme: &Theme) -> [u8; 4] {
    if idx < 16 {
        return theme.colors[idx as usize];
    }
    if idx < 232 {
        // 6×6×6 color cube (indices 16-231)
        let i = idx - 16;
        let r = (i / 36) % 6;
        let g = (i / 6) % 6;
        let b = i % 6;
        let to_byte = |v: u8| if v == 0 { 0u8 } else { 55 + 40 * v };
        return [to_byte(r), to_byte(g), to_byte(b), 255];
    }
    // Greyscale ramp (indices 232-255)
    let v = 8 + 10 * (idx - 232);
    [v, v, v, 255]
}

/// Resolve a `vt100::Color` to RGBA bytes, considering the theme and bold
/// brightening (for foreground colors with indices 0–7).
fn resolve_color(
    color: vt100::Color,
    is_fg: bool,
    bold: bool,
    theme: &Theme,
) -> [u8; 4] {
    match color {
        vt100::Color::Default => {
            if is_fg {
                theme.foreground
            } else {
                theme.background
            }
        }
        vt100::Color::Idx(i) => {
            // Bold foreground colors 0-7 get brightened to 8-15
            let idx = if is_fg && bold && i < 8 { i + 8 } else { i };
            ansi256_to_rgba(idx, theme)
        }
        vt100::Color::Rgb(r, g, b) => [r, g, b, 255],
    }
}

/// Fill a rectangle on the pixmap with a solid RGBA color.
fn fill_rect(pixmap: &mut Pixmap, x: u32, y: u32, w: u32, h: u32, rgba: [u8; 4]) {
    let rect = match Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        Some(r) => r,
        None => return,
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    let path = match pb.finish() {
        Some(p) => p,
        None => return,
    };
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
    paint.anti_alias = false;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Render the current terminal screen to a PNG image.
///
/// Returns the raw PNG bytes on success.
pub fn render_screenshot(
    screen: &vt100::Screen,
    theme_name: &str,
    font_size: u32,
    scale: f32,
) -> Result<Vec<u8>> {
    let theme = get_theme(theme_name);
    let (rows, cols) = screen.size();

    // -- Load fonts ----------------------------------------------------------

    let font_regular = Font::from_bytes(
        FONT_REGULAR,
        FontSettings {
            scale: font_size as f32 * scale,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow::anyhow!("failed to load regular font: {e}"))?;

    let font_bold = Font::from_bytes(
        FONT_BOLD,
        FontSettings {
            scale: font_size as f32 * scale,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow::anyhow!("failed to load bold font: {e}"))?;

    // -- Cell geometry -------------------------------------------------------
    // Use metrics from the font to derive cell sizes.

    let px_size = font_size as f32 * scale;
    let metrics = font_regular.horizontal_line_metrics(px_size).unwrap_or(
        fontdue::LineMetrics {
            ascent: px_size * 0.8,
            descent: px_size * -0.2,
            line_gap: 0.0,
            new_line_size: px_size * 1.2,
        },
    );

    // Measure a reference character to get the advance width.
    let (m_metrics, _) = font_regular.rasterize('M', px_size);
    let cell_width = m_metrics.advance_width.ceil().max(1.0) as u32;
    let cell_height = metrics.new_line_size.ceil().max(1.0) as u32;
    let baseline = metrics.ascent.ceil() as i32;

    let padding: u32 = (8.0 * scale).ceil() as u32;
    let img_width = cell_width * cols as u32 + padding * 2;
    let img_height = cell_height * rows as u32 + padding * 2;

    // -- Create pixmap -------------------------------------------------------

    let mut pixmap = Pixmap::new(img_width, img_height)
        .context("failed to create pixmap (dimensions too large?)")?;

    // Fill background
    pixmap.fill(Color::from_rgba8(
        theme.background[0],
        theme.background[1],
        theme.background[2],
        theme.background[3],
    ));

    // -- Render cells --------------------------------------------------------

    for row in 0..rows {
        for col in 0..cols {
            let cell = match screen.cell(row, col) {
                Some(c) => c,
                None => continue,
            };

            if cell.is_wide_continuation() {
                continue;
            }

            let bold = cell.bold();
            let underline = cell.underline();
            let inverse = cell.inverse();

            // Resolve colors
            let (mut fg_rgba, mut bg_rgba) = (
                resolve_color(cell.fgcolor(), true, bold, &theme),
                resolve_color(cell.bgcolor(), false, false, &theme),
            );

            if inverse {
                std::mem::swap(&mut fg_rgba, &mut bg_rgba);
            }

            let cell_x = padding + col as u32 * cell_width;
            let cell_y = padding + row as u32 * cell_height;

            // Draw cell background if it differs from the theme background
            if bg_rgba != theme.background {
                let w = if cell.is_wide() {
                    cell_width * 2
                } else {
                    cell_width
                };
                fill_rect(&mut pixmap, cell_x, cell_y, w, cell_height, bg_rgba);
            }

            // Rasterize the character glyph
            let contents = cell.contents();
            let ch = match contents.chars().next() {
                Some(c) if !c.is_control() && c != ' ' => c,
                _ => continue,
            };

            let font = if bold { &font_bold } else { &font_regular };
            let (glyph_metrics, bitmap) = font.rasterize(ch, px_size);

            if bitmap.is_empty() || glyph_metrics.width == 0 || glyph_metrics.height == 0 {
                continue;
            }

            // Position the glyph within the cell
            let glyph_x = cell_x as i32 + glyph_metrics.xmin;
            let glyph_y = cell_y as i32 + baseline - glyph_metrics.height as i32 - glyph_metrics.ymin;

            // Paint glyph pixels onto pixmap
            paint_glyph(
                &mut pixmap,
                &bitmap,
                glyph_metrics.width,
                glyph_metrics.height,
                glyph_x,
                glyph_y,
                fg_rgba,
            );

            // Draw underline
            if underline {
                let underline_y = cell_y + cell_height - (2.0 * scale).ceil() as u32;
                let underline_h = (1.0 * scale).ceil().max(1.0) as u32;
                let w = if cell.is_wide() {
                    cell_width * 2
                } else {
                    cell_width
                };
                fill_rect(&mut pixmap, cell_x, underline_y, w, underline_h, fg_rgba);
            }
        }
    }

    // -- Encode PNG ----------------------------------------------------------

    let png_bytes = pixmap.encode_png().context("failed to encode PNG")?;
    Ok(png_bytes)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_dark_default() {
        let theme = get_theme("dark");
        assert_eq!(theme.name, "dark");
        assert_eq!(theme.background, [30, 30, 30, 255]);
    }

    #[test]
    fn theme_light() {
        let theme = get_theme("light");
        assert_eq!(theme.name, "light");
        assert_eq!(theme.background, [255, 255, 255, 255]);
        assert_eq!(theme.foreground, [30, 30, 30, 255]);
    }

    #[test]
    fn theme_unknown_defaults_to_dark() {
        let theme = get_theme("nonexistent");
        assert_eq!(theme.name, "dark");
    }

    #[test]
    fn ansi256_standard_colors() {
        let theme = get_theme("dark");
        for i in 0u8..16 {
            let rgba = ansi256_to_rgba(i, &theme);
            assert_eq!(rgba, theme.colors[i as usize]);
        }
    }

    #[test]
    fn ansi256_color_cube() {
        let theme = get_theme("dark");
        // Index 16 = (0,0,0) in the 6x6x6 cube
        let rgba = ansi256_to_rgba(16, &theme);
        assert_eq!(rgba, [0, 0, 0, 255]);

        // Index 196: i=180, r=180/36=5, g=0, b=0 → r=255, g=0, b=0
        let rgba = ansi256_to_rgba(196, &theme);
        assert_eq!(rgba[0], 55 + 40 * 5); // 255
        assert_eq!(rgba[1], 0);
        assert_eq!(rgba[2], 0);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn ansi256_greyscale_ramp() {
        let theme = get_theme("dark");
        // Index 232 = grey level 0 → value = 8
        let rgba = ansi256_to_rgba(232, &theme);
        assert_eq!(rgba, [8, 8, 8, 255]);

        // Index 255 = grey level 23 → value = 8 + 10*23 = 238
        let rgba = ansi256_to_rgba(255, &theme);
        assert_eq!(rgba, [238, 238, 238, 255]);
    }

    #[test]
    fn resolve_color_default_fg() {
        let theme = get_theme("dark");
        let rgba = resolve_color(vt100::Color::Default, true, false, &theme);
        assert_eq!(rgba, theme.foreground);
    }

    #[test]
    fn resolve_color_default_bg() {
        let theme = get_theme("dark");
        let rgba = resolve_color(vt100::Color::Default, false, false, &theme);
        assert_eq!(rgba, theme.background);
    }

    #[test]
    fn resolve_color_indexed() {
        let theme = get_theme("dark");
        let rgba = resolve_color(vt100::Color::Idx(1), true, false, &theme);
        assert_eq!(rgba, theme.colors[1]); // red
    }

    #[test]
    fn resolve_color_bold_brightens_fg() {
        let theme = get_theme("dark");
        // Bold + fg color 1 → should map to color 9 (bright red)
        let rgba = resolve_color(vt100::Color::Idx(1), true, true, &theme);
        assert_eq!(rgba, theme.colors[9]);
    }

    #[test]
    fn resolve_color_bold_does_not_brighten_bg() {
        let theme = get_theme("dark");
        // Bold should NOT brighten background colors
        let rgba = resolve_color(vt100::Color::Idx(1), false, true, &theme);
        assert_eq!(rgba, theme.colors[1]); // stays as index 1
    }

    #[test]
    fn resolve_color_bold_no_brighten_above_7() {
        let theme = get_theme("dark");
        // Bold + color 10 → no change (already bright range)
        let rgba = resolve_color(vt100::Color::Idx(10), true, true, &theme);
        assert_eq!(rgba, theme.colors[10]);
    }

    #[test]
    fn resolve_color_rgb() {
        let theme = get_theme("dark");
        let rgba = resolve_color(vt100::Color::Rgb(100, 150, 200), true, false, &theme);
        assert_eq!(rgba, [100, 150, 200, 255]);
    }

    #[test]
    fn render_screenshot_basic_does_not_panic() {
        let parser = vt100::Parser::new(5, 10, 0);
        let screen = parser.screen();
        let result = render_screenshot(screen, "dark", 14, 1.0);
        assert!(result.is_ok());
        let png = result.unwrap();
        // PNG files start with the magic bytes
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn render_screenshot_with_text() {
        let mut parser = vt100::Parser::new(3, 10, 0);
        parser.process(b"Hello!");
        let screen = parser.screen();
        let result = render_screenshot(screen, "dark", 14, 1.0);
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn render_screenshot_light_theme() {
        let parser = vt100::Parser::new(3, 10, 0);
        let screen = parser.screen();
        let result = render_screenshot(screen, "light", 14, 1.0);
        assert!(result.is_ok());
    }

    #[test]
    fn render_screenshot_with_colors() {
        let mut parser = vt100::Parser::new(3, 20, 0);
        parser.process(b"\x1b[31mred\x1b[0m \x1b[1;32mbold green\x1b[0m");
        let screen = parser.screen();
        let result = render_screenshot(screen, "dark", 14, 1.0);
        assert!(result.is_ok());
    }

    #[test]
    fn render_screenshot_scaled() {
        let parser = vt100::Parser::new(3, 10, 0);
        let screen = parser.screen();
        let result = render_screenshot(screen, "dark", 14, 2.0);
        assert!(result.is_ok());
    }
}

/// Paint a fontdue glyph bitmap onto the pixmap with the given foreground color.
///
/// Each byte in `bitmap` is a coverage value (0–255). We alpha-blend over
/// the existing pixel.
fn paint_glyph(
    pixmap: &mut Pixmap,
    bitmap: &[u8],
    glyph_w: usize,
    glyph_h: usize,
    x0: i32,
    y0: i32,
    fg: [u8; 4],
) {
    let img_w = pixmap.width() as i32;
    let img_h = pixmap.height() as i32;
    let pixels = pixmap.data_mut();

    for gy in 0..glyph_h {
        let py = y0 + gy as i32;
        if py < 0 || py >= img_h {
            continue;
        }
        for gx in 0..glyph_w {
            let px = x0 + gx as i32;
            if px < 0 || px >= img_w {
                continue;
            }

            let coverage = bitmap[gy * glyph_w + gx];
            if coverage == 0 {
                continue;
            }

            let offset = (py as usize * img_w as usize + px as usize) * 4;

            if coverage == 255 {
                // Fully opaque — direct write
                pixels[offset] = fg[0];
                pixels[offset + 1] = fg[1];
                pixels[offset + 2] = fg[2];
                pixels[offset + 3] = 255;
            } else {
                // Alpha-blend
                let alpha = coverage as u16;
                let inv = 255 - alpha;
                pixels[offset] = ((fg[0] as u16 * alpha + pixels[offset] as u16 * inv) / 255) as u8;
                pixels[offset + 1] =
                    ((fg[1] as u16 * alpha + pixels[offset + 1] as u16 * inv) / 255) as u8;
                pixels[offset + 2] =
                    ((fg[2] as u16 * alpha + pixels[offset + 2] as u16 * inv) / 255) as u8;
                pixels[offset + 3] = 255;
            }
        }
    }
}
