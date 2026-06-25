use crate::colormap::{to_rgba_bytes, Colormap};
use crate::parser::SpmImage;
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use image::{ImageBuffer, Rgb, Rgba, RgbaImage};
use std::path::Path;

/// Compute a line profile between two pixel-space points (fractional, 0..1).
/// Returns Vec of (distance_nm, height_nm).
pub fn line_profile(image: &SpmImage, p0: (f32, f32), p1: (f32, f32)) -> Vec<(f32, f32)> {
    let rows = image.number_of_lines;
    let cols = image.samps_per_line;
    let nm_per_px_x = image.scan_size_nm / cols as f32;
    let nm_per_px_y = image.scan_size_nm / rows as f32;

    let x0 = p0.0 * cols as f32;
    let y0 = p0.1 * rows as f32;
    let x1 = p1.0 * cols as f32;
    let y1 = p1.1 * rows as f32;

    let dx_px = x1 - x0;
    let dy_px = y1 - y0;
    let len_px = (dx_px * dx_px + dy_px * dy_px).sqrt();
    let n_samples = (len_px.ceil() as usize).max(2);

    let total_nm = ((dx_px * nm_per_px_x).powi(2) + (dy_px * nm_per_px_y).powi(2)).sqrt();

    let mut profile = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let frac = i as f32 / (n_samples - 1) as f32;
        let px = x0 + frac * dx_px;
        let py = y0 + frac * dy_px;

        let col = px.round() as isize;
        let row = py.round() as isize;
        if col < 0 || row < 0 || col as usize >= cols || row as usize >= rows {
            continue;
        }
        let height = image.data[row as usize * cols + col as usize];
        profile.push((frac * total_nm, height));
    }

    profile
}

pub fn export_afm_image(
    data: &[f32],
    rows: usize,
    cols: usize,
    cmap: Colormap,
    z_min: f32,
    z_max: f32,
    scan_size_nm: f32,
    scale_bar: bool,
    path: &Path,
) -> Result<(), String> {
    let rgba = to_rgba_bytes(data, rows, cols, cmap, z_min, z_max);
    let mut img: RgbaImage = ImageBuffer::from_raw(cols as u32, rows as u32, rgba)
        .ok_or_else(|| "Failed to create image buffer".to_string())?;
    if scale_bar {
        draw_scale_bar(&mut img, scan_size_nm);
    }
    img.save(path)
        .map_err(|e| format!("Failed to save image: {e}"))
}

/// Snap a length hint (nm) to a "nice" 1-2-5 value, covering 1 nm … 100 µm.
pub fn nice_scale(hint: f32) -> f32 {
    let nice = [
        1.0_f32, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
        20000.0, 50000.0, 100000.0,
    ];
    nice.iter()
        .copied()
        .min_by_key(|&v| ((v - hint).abs() * 1000.0) as i64)
        .unwrap_or(hint)
}

/// Burn a scale bar (~1/5 of the image width, snapped to a nice value) plus a
/// "<n> nm" label into the bottom-left of the image. Everything is drawn at the
/// image's native resolution, so the export is independent of any on-screen zoom.
fn draw_scale_bar(img: &mut RgbaImage, scan_size_nm: f32) {
    let (w, h) = (img.width(), img.height());
    if w < 16 || h < 16 || scan_size_nm <= 0.0 {
        return;
    }

    let bar_nm = nice_scale(scan_size_nm / 5.0);
    let bar_px = (((bar_nm / scan_size_nm) * w as f32).round() as u32).clamp(1, w - 1);

    // Sizes scale with the image so the bar and label stay legible at any res.
    let s = (h / 100).max(2);
    let bar_h = (h / 120).max(3);
    let pad = s;
    let gap = s;
    let margin = 4 * s;
    let label = format!("{} nm", bar_nm as i32);

    // Prefer Arial for the label; fall back to the built-in bitmap font.
    let font = load_label_font();
    let px = (h as f32 * 0.06).max(11.0);
    let (text_w, text_h) = match &font {
        Some(f) => (measure_text_ttf(&label, px, f).ceil() as u32, px.ceil() as u32),
        None => (text_width(&label, s), 7 * s),
    };

    let bar_x = margin;
    let bar_y = h.saturating_sub(margin + bar_h);
    let text_x = bar_x;
    let text_y = bar_y.saturating_sub(text_h + gap);

    // Darkened panel behind the group → readable on any colormap.
    let panel_x = bar_x.saturating_sub(pad);
    let panel_y = text_y.saturating_sub(pad);
    let panel_w = bar_px.max(text_w) + 2 * pad;
    let panel_h = (bar_y + bar_h).saturating_sub(text_y) + 2 * pad;
    darken_rect(img, panel_x, panel_y, panel_w, panel_h, 0.45);

    let white = Rgba([255u8, 255, 255, 255]);
    fill_rect(img, bar_x, bar_y, bar_px, bar_h, white);
    match &font {
        Some(f) => draw_text_ttf(img, text_x, text_y, &label, px, f),
        None => draw_text(img, text_x, text_y, &label, s, white),
    }
}

/// Load Arial for the exported label, trying the usual Windows/macOS locations.
/// Returns `None` if unavailable (callers then fall back to the bitmap font).
fn load_label_font() -> Option<FontVec> {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\arial.ttf",
        "/Library/Fonts/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(font) = FontVec::try_from_vec(bytes) {
                return Some(font);
            }
        }
    }
    None
}

fn measure_text_ttf(text: &str, px: f32, font: &FontVec) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    text.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum()
}

/// Draw anti-aliased white text at pixel height `px`, blending each glyph's
/// coverage over the (already darkened) background.
fn draw_text_ttf(img: &mut RgbaImage, x: u32, y: u32, text: &str, px: f32, font: &FontVec) {
    let sf = font.as_scaled(PxScale::from(px));
    let ascent = sf.ascent();
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let mut caret = x as f32;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        let glyph = id.with_scale_and_position(px, point(caret, y as f32 + ascent));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let tx = bounds.min.x as i32 + gx as i32;
                let ty = bounds.min.y as i32 + gy as i32;
                if tx >= 0 && ty >= 0 && tx < iw && ty < ih {
                    let c = cov.clamp(0.0, 1.0);
                    let p = img.get_pixel_mut(tx as u32, ty as u32);
                    p[0] = (p[0] as f32 * (1.0 - c) + 255.0 * c) as u8;
                    p[1] = (p[1] as f32 * (1.0 - c) + 255.0 * c) as u8;
                    p[2] = (p[2] as f32 * (1.0 - c) + 255.0 * c) as u8;
                }
            });
        }
        caret += sf.h_advance(id);
    }
}

fn fill_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    let (iw, ih) = (img.width(), img.height());
    for yy in y..(y + h).min(ih) {
        for xx in x..(x + w).min(iw) {
            img.put_pixel(xx, yy, color);
        }
    }
}

fn darken_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, amount: f32) {
    let (iw, ih) = (img.width(), img.height());
    let keep = (1.0 - amount).clamp(0.0, 1.0);
    for yy in y..(y + h).min(ih) {
        for xx in x..(x + w).min(iw) {
            let p = img.get_pixel_mut(xx, yy);
            p[0] = (p[0] as f32 * keep) as u8;
            p[1] = (p[1] as f32 * keep) as u8;
            p[2] = (p[2] as f32 * keep) as u8;
        }
    }
}

/// Advance per glyph is 5 px wide + 1 px spacing, scaled by `s`.
fn text_width(text: &str, s: u32) -> u32 {
    text.chars().count() as u32 * 6 * s
}

fn draw_text(img: &mut RgbaImage, x: u32, y: u32, text: &str, s: u32, color: Rgba<u8>) {
    let mut cx = x;
    for c in text.chars() {
        draw_glyph(img, cx, y, c, s, color);
        cx += 6 * s;
    }
}

fn draw_glyph(img: &mut RgbaImage, x: u32, y: u32, c: char, s: u32, color: Rgba<u8>) {
    for (row, bits) in glyph(c).iter().enumerate() {
        for col in 0..5u32 {
            if bits & (0x10 >> col) != 0 {
                fill_rect(img, x + col * s, y + row as u32 * s, s, s, color);
            }
        }
    }
}

/// 5×7 bitmap font for the glyphs the scale-bar label needs (digits, space,
/// 'n', 'm'). Each row's low 5 bits are columns, MSB (0x10) = leftmost.
fn glyph(c: char) -> [u8; 7] {
    const SPACE: [u8; 7] = [0, 0, 0, 0, 0, 0, 0];
    const DIGITS: [[u8; 7]; 10] = [
        [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
    ];
    const N: [u8; 7] = [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11];
    const M: [u8; 7] = [0x00, 0x00, 0x1A, 0x15, 0x15, 0x15, 0x15];
    match c {
        '0'..='9' => DIGITS[c as usize - '0' as usize],
        'n' => N,
        'm' => M,
        _ => SPACE,
    }
}

pub fn export_csv(profile: &[(f32, f32)], path: &Path) -> Result<(), String> {
    use std::fmt::Write as FmtWrite;
    let mut out = String::from("distance_nm,height_nm\n");
    for (d, h) in profile {
        writeln!(out, "{d:.4},{h:.6}").map_err(|e| e.to_string())?;
    }
    std::fs::write(path, out).map_err(|e| format!("Failed to write CSV: {e}"))
}

/// Export the profile as a simple PNG plot (800×400 px).
pub fn export_profile_png(profile: &[(f32, f32)], path: &Path) -> Result<(), String> {
    if profile.is_empty() {
        return Err("Profile is empty".to_string());
    }

    const W: u32 = 800;
    const H: u32 = 400;
    const MARGIN: u32 = 50;

    let x_min = profile.first().map(|p| p.0).unwrap_or(0.0);
    let x_max = profile.last().map(|p| p.0).unwrap_or(1.0);
    let y_min = profile.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let y_max = profile.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    let x_range = (x_max - x_min).max(f32::EPSILON);
    let y_range = (y_max - y_min).max(f32::EPSILON);

    let mut img: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_pixel(W, H, Rgb([255u8, 255, 255]));

    let plot_w = W - 2 * MARGIN;
    let plot_h = H - 2 * MARGIN;

    // Draw axes
    for x in MARGIN..(W - MARGIN) {
        img.put_pixel(x, H - MARGIN, Rgb([0u8, 0, 0]));
    }
    for y in MARGIN..(H - MARGIN) {
        img.put_pixel(MARGIN, y, Rgb([0u8, 0, 0]));
    }

    // Draw profile line
    let to_pixel = |(d, h): (f32, f32)| -> (u32, u32) {
        let px = MARGIN + ((d - x_min) / x_range * plot_w as f32) as u32;
        let py = H - MARGIN - ((h - y_min) / y_range * plot_h as f32) as u32;
        (px.min(W - 1), py.min(H - 1))
    };

    let points: Vec<(u32, u32)> = profile.iter().map(|&p| to_pixel(p)).collect();
    for window in points.windows(2) {
        let (x0, y0) = (window[0].0 as i32, window[0].1 as i32);
        let (x1, y1) = (window[1].0 as i32, window[1].1 as i32);
        draw_line(&mut img, x0, y0, x1, y1, Rgb([0u8, 100, 200]));
    }

    img.save(path)
        .map_err(|e| format!("Failed to save PNG: {e}"))
}

/// Rolling-ball background subtraction (Sternberg). Returns `data` with the
/// estimated background removed; `radius` is the ball radius in pixels.
///
/// The height field is normalized to a fixed 0..100 working range so the ball
/// (a sphere of radius `radius` in that calibrated space) behaves consistently
/// regardless of the data's physical units. For speed the image is shrunk for
/// large radii (à la ImageJ), the ball rolled on the small image, and the
/// background bilinearly enlarged before subtraction. The result is always
/// ≥ 0 (morphological opening is anti-extensive), i.e. background ≈ 0.
pub fn rolling_ball_subtract(data: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    if radius == 0 || w == 0 || h == 0 || data.len() < w * h {
        return data.to_vec();
    }
    let min = data.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(f32::EPSILON);
    const WORK: f32 = 100.0;

    let norm: Vec<f32> = data.iter().map(|&v| (v - min) / range * WORK).collect();
    let background = rolling_ball_background(&norm, w, h, radius);

    data.iter()
        .zip(background.iter())
        .map(|(&v, &bg)| v - (bg / WORK * range + min))
        .collect()
}

/// Estimate the background of a 0..100 normalized field by rolling a ball of
/// `radius` px underneath it (grayscale opening), shrinking first for speed.
fn rolling_ball_background(norm: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let shrink = if radius <= 10 {
        1
    } else if radius <= 30 {
        2
    } else if radius <= 100 {
        4
    } else {
        8
    };
    let small_r = (radius / shrink).max(1);

    let (sw, sh, small) = shrink_min(norm, w, h, shrink);
    let small_bg = ball_opening(&small, sw, sh, small_r);
    enlarge(&small_bg, sw, sh, w, h)
}

/// Downsample by taking the minimum of each `k`×`k` block (the ball rolls under
/// the surface, so the background follows the local minima).
fn shrink_min(data: &[f32], w: usize, h: usize, k: usize) -> (usize, usize, Vec<f32>) {
    if k <= 1 {
        return (w, h, data.to_vec());
    }
    let sw = w.div_ceil(k);
    let sh = h.div_ceil(k);
    let mut out = vec![f32::INFINITY; sw * sh];
    for y in 0..h {
        for x in 0..w {
            let o = (y / k) * sw + (x / k);
            out[o] = out[o].min(data[y * w + x]);
        }
    }
    (sw, sh, out)
}

/// Grayscale opening (erode then dilate) with a spherical structuring element.
fn ball_opening(g: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let ball = build_ball(r);
    let eroded = ball_morph(g, w, h, &ball, false);
    ball_morph(&eroded, w, h, &ball, true)
}

/// Offsets (di, dj) within radius `r` and the ball's height there (√(r²−d²)).
fn build_ball(r: usize) -> Vec<(i32, i32, f32)> {
    let ri = r as i32;
    let r2 = (r * r) as f32;
    let mut ball = Vec::new();
    for dj in -ri..=ri {
        for di in -ri..=ri {
            let d2 = (di * di + dj * dj) as f32;
            if d2 <= r2 {
                ball.push((di, dj, (r2 - d2).sqrt()));
            }
        }
    }
    ball
}

/// One morphological pass with the (symmetric) ball SE: erosion (`dilate=false`)
/// computes min(g − ball); dilation (`dilate=true`) computes max(g + ball).
/// Out-of-bounds samples use edge replication.
fn ball_morph(g: &[f32], w: usize, h: usize, ball: &[(i32, i32, f32)], dilate: bool) -> Vec<f32> {
    let (wi, hi) = (w as i32, h as i32);
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = if dilate { f32::NEG_INFINITY } else { f32::INFINITY };
            for &(di, dj, bh) in ball {
                let xx = (x as i32 + di).clamp(0, wi - 1) as usize;
                let yy = (y as i32 + dj).clamp(0, hi - 1) as usize;
                let s = g[yy * w + xx];
                if dilate {
                    acc = acc.max(s + bh);
                } else {
                    acc = acc.min(s - bh);
                }
            }
            out[y * w + x] = acc;
        }
    }
    out
}

/// Bilinearly enlarge an `sw`×`sh` field to `w`×`h` (corners aligned).
fn enlarge(small: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    if sw == w && sh == h {
        return small.to_vec();
    }
    let mut out = vec![0.0f32; w * h];
    let sx = if w > 1 { (sw - 1) as f32 / (w - 1) as f32 } else { 0.0 };
    let sy = if h > 1 { (sh - 1) as f32 / (h - 1) as f32 } else { 0.0 };
    for y in 0..h {
        let gy = y as f32 * sy;
        let y0 = gy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = gy - y0 as f32;
        for x in 0..w {
            let gx = x as f32 * sx;
            let x0 = gx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = gx - x0 as f32;
            let v00 = small[y0 * sw + x0];
            let v01 = small[y0 * sw + x1];
            let v10 = small[y1 * sw + x0];
            let v11 = small[y1 * sw + x1];
            let top = v00 + (v01 - v00) * tx;
            let bot = v10 + (v11 - v10) * tx;
            out[y * w + x] = top + (bot - top) * ty;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_ball_noop_for_zero_radius() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(rolling_ball_subtract(&data, 2, 2, 0), data);
    }

    #[test]
    fn rolling_ball_flattens_constant() {
        let (w, h) = (32, 32);
        let data = vec![5.0f32; w * h];
        let out = rolling_ball_subtract(&data, w, h, 8);
        assert!(out.iter().all(|&v| v.abs() < 1e-3));
    }

    #[test]
    fn rolling_ball_keeps_narrow_feature_removes_background() {
        let (w, h) = (40, 40);
        let mut data = vec![0.0f32; w * h];
        for y in 19..21 {
            for x in 19..21 {
                data[y * w + x] = 10.0; // tiny bump, much smaller than the ball
            }
        }
        let out = rolling_ball_subtract(&data, w, h, 10);
        assert!(out[20 * w + 20] > 5.0, "feature preserved: {}", out[20 * w + 20]);
        assert!(out[0].abs() < 1.0, "background removed: {}", out[0]);
        assert!(out.iter().all(|&v| v.is_finite() && v >= -1e-3));
    }

    #[test]
    fn nice_scale_snaps_to_1_2_5() {
        assert_eq!(nice_scale(190.0), 200.0);
        assert_eq!(nice_scale(0.9), 1.0);
        assert_eq!(nice_scale(3.0), 2.0);
        assert_eq!(nice_scale(800.0), 1000.0);
        assert_eq!(nice_scale(3000.0), 2000.0);
    }

    #[test]
    fn scale_bar_has_expected_width() {
        let (w, h) = (200u32, 200u32);
        let mut img = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]));
        let scan = 1000.0_f32; // bar_nm = nice_scale(200) = 200 → 40 px
        draw_scale_bar(&mut img, scan);

        let expected = ((200.0 / scan) * w as f32).round() as u32; // 40
        // Longest solid-white horizontal run = the bar (wider than any glyph).
        let mut best = 0u32;
        for y in 0..h {
            let mut run = 0u32;
            for x in 0..w {
                if img.get_pixel(x, y).0 == [255, 255, 255, 255] {
                    run += 1;
                    best = best.max(run);
                } else {
                    run = 0;
                }
            }
        }
        assert!(
            (best as i32 - expected as i32).abs() <= 2,
            "bar width {best}, expected {expected}"
        );
    }

    #[test]
    fn scale_bar_is_noop_on_tiny_image() {
        let mut img = RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 255]));
        draw_scale_bar(&mut img, 1000.0);
        assert!(img.pixels().all(|p| p.0 == [10, 20, 30, 255]));
    }
}

fn draw_line(img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb<u8>) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let steps = dx.max(dy).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = (x0 as f32 + t * (x1 - x0) as f32).round() as i32;
        let y = (y0 as f32 + t * (y1 - y0) as f32).round() as i32;
        if x >= 0 && y >= 0 && x < w && y < h {
            img.put_pixel(x as u32, y as u32, color);
        }
    }
}
