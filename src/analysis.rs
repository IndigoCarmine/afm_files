use crate::parser::SpmImage;
use image::{ImageBuffer, Rgb};
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
