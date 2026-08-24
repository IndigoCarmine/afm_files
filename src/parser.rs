use crate::calibration::Calibration;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Clone)]
struct ImageBlock {
    fields: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ChannelInfo {
    pub index: usize,
    pub name: String,
    // Kept for debugging/display purposes even though nothing reads it yet.
    #[allow(dead_code)]
    pub direction: String,
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_header_and_image_blocks(header_text: &str) -> (HashMap<String, String>, Vec<ImageBlock>) {
    let mut measurement_conditions = HashMap::new();
    let mut image_blocks: Vec<ImageBlock> = Vec::new();
    let mut current_section = String::from("root");
    let mut current_image_block: Option<ImageBlock> = None;

    for raw_line in header_text.lines() {
        let line = raw_line.trim_end_matches('\r').trim();
        if !line.starts_with('\\') {
            continue;
        }

        if line.starts_with("\\*") {
            let section = line.trim_start_matches("\\*").trim();
            current_section = section.to_string();
            if section == "Ciao image list" {
                if let Some(block) = current_image_block.take() {
                    image_blocks.push(block);
                }
                current_image_block = Some(ImageBlock::default());
            }
            continue;
        }

        let body = line.trim_start_matches('\\');
        let parsed = body.split_once(": ").or_else(|| body.split_once(':'));
        if let Some((key, value)) = parsed {
            let key = key.trim();
            let value = value.trim();

            let sectioned_key = format!("{current_section}::{key}");
            measurement_conditions.insert(sectioned_key, value.to_string());
            measurement_conditions
                .entry(key.to_string())
                .or_insert_with(|| value.to_string());

            if let Some(block) = current_image_block.as_mut() {
                block.fields.insert(key.to_string(), value.to_string());
            }
        }
    }

    if let Some(block) = current_image_block.take() {
        image_blocks.push(block);
    }

    (measurement_conditions, image_blocks)
}

fn parse_usize_field(value: Option<&String>, field_name: &str) -> Result<usize, String> {
    let raw = value.ok_or_else(|| format!("Missing field: {field_name}"))?;
    let num_text: String = raw
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if num_text.is_empty() {
        return Err(format!("Failed to parse numeric field {field_name}: {raw}"));
    }
    num_text
        .parse::<usize>()
        .map_err(|e| format!("Failed to parse field {field_name}: {e}"))
}

fn parse_float_before_unit(text: &str, unit: &str) -> Option<f32> {
    let idx = text.find(unit)?;
    let prefix = text[..idx].trim_end();
    let token = prefix
        .split_whitespace()
        .last()
        .map(|s| s.trim_matches(|c: char| c == '(' || c == ')' || c == ','))?;
    token.parse::<f32>().ok()
}

fn parse_sensitivity_name(scale_text: &str) -> Option<String> {
    let sens_pos = scale_text.find("[Sens.")?;
    let after = &scale_text[(sens_pos + "[Sens.".len())..];
    let end = after.find(']')?;
    Some(after[..end].trim().to_string())
}

// Returns the last parseable f32 token in text, used to extract the full-scale V value
// from a Z scale string like "V [Sens. Zsens] (0.006176541 V/LSB) 1.988846 V".
fn parse_last_float(text: &str) -> Option<f32> {
    text.split_whitespace()
        .filter_map(|t| {
            t.trim_matches(|c: char| c == '(' || c == ')' || c == ',')
                .parse::<f32>()
                .ok()
        })
        .next_back()
}

fn parse_nm_per_lsb(
    image_block: &ImageBlock,
    measurement_conditions: &HashMap<String, String>,
    bytes_per_pixel: usize,
) -> Result<f32, String> {
    let z_scale_text = image_block
        .fields
        .iter()
        .find_map(|(k, v)| (k.ends_with(":Z scale") || k == "Z scale").then_some(v))
        .ok_or_else(|| "Missing Z scale field in image block".to_string())?;

    if let Some(nm_per_lsb) = parse_float_before_unit(z_scale_text, "nm/LSB") {
        return Ok(nm_per_lsb);
    }

    let v_per_lsb = parse_float_before_unit(z_scale_text, "V/LSB")
        .ok_or_else(|| format!("Could not parse V/LSB from Z scale: {z_scale_text}"))?;

    let sens_name = parse_sensitivity_name(z_scale_text).ok_or_else(|| {
        format!("Could not parse sensitivity reference from Z scale: {z_scale_text}")
    })?;
    let sens_key = format!("@Sens. {sens_name}");
    let sens_value = measurement_conditions
        .get(&sens_key)
        .ok_or_else(|| format!("Missing sensitivity field: {sens_key}"))?;
    let nm_per_v = parse_float_before_unit(sens_value, "nm/V").ok_or_else(|| {
        format!("Could not parse nm/V from sensitivity field {sens_key}: {sens_value}")
    })?;

    // Bruker convention: Z scale voltage represents the full peak-to-peak range across all
    // 256^bpp steps (unsigned integer range). For 16-bit: 65536 steps, so 1 LSB = z_scale_v / 65536.
    // For trace scans z_scale_v < v_per_lsb * max_int, so min() selects z_scale_v / max_int.
    // For retrace scans z_scale_v = v_per_lsb * max_int (full hardware range), so both are equal.
    let z_scale_v = parse_last_float(z_scale_text)
        .ok_or_else(|| format!("Could not parse full-scale V from Z scale: {z_scale_text}"))?;
    let max_int = (1u64 << (bytes_per_pixel * 8)) as f32;
    let effective_v_per_lsb = v_per_lsb.min(z_scale_v / max_int);

    Ok(effective_v_per_lsb * nm_per_v)
}

fn apply_scale_in_place(image_2d: &mut [Vec<f32>], scale: f32) {
    for row in image_2d.iter_mut() {
        for v in row.iter_mut() {
            *v *= scale;
        }
    }
}

fn has_required_fields(b: &ImageBlock) -> bool {
    b.fields.contains_key("Data offset")
        && b.fields.contains_key("Data length")
        && b.fields.contains_key("Samps/line")
        && b.fields.contains_key("Number of lines")
        && b.fields.contains_key("Bytes/pixel")
}

fn extract_channel_name(image_data_value: &str) -> String {
    // Value format: "S [Height] \"Height\"" → extract text between first pair of quotes
    let parts: Vec<&str> = image_data_value.split('"').collect();
    if parts.len() >= 2 && !parts[1].is_empty() {
        parts[1].to_string()
    } else {
        image_data_value.to_string()
    }
}

fn list_channels_from_blocks(image_blocks: &[ImageBlock]) -> Vec<ChannelInfo> {
    image_blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| has_required_fields(b))
        .map(|(i, b)| {
            let name = b
                .fields
                .iter()
                .find_map(|(k, v)| {
                    (k.ends_with(":Image Data") || k == "Image Data")
                        .then(|| extract_channel_name(v))
                })
                .unwrap_or_else(|| format!("Channel {i}"));
            let direction = b.fields.get("Line Direction").cloned().unwrap_or_default();
            ChannelInfo {
                index: i,
                name,
                direction,
            }
        })
        .collect()
}

fn choose_image_block(
    image_blocks: &[ImageBlock],
    channel_idx: Option<usize>,
) -> Result<(usize, &ImageBlock), String> {
    // Explicit channel index: use that block directly
    if let Some(idx) = channel_idx {
        let block = image_blocks
            .get(idx)
            .ok_or_else(|| format!("Channel index {idx} out of range"))?;
        if !has_required_fields(block) {
            return Err(format!("Channel {idx} is missing required fields"));
        }
        return Ok((idx, block));
    }

    // Auto: prefer last Height block
    if let Some((idx, block)) = image_blocks.iter().enumerate().rfind(|(_, b)| {
        let has_height = b.fields.iter().any(|(k, v)| {
            (k.ends_with(":Image Data") || k == "Image Data") && v.contains("Height")
        });
        has_required_fields(b) && has_height
    }) {
        return Ok((idx, block));
    }

    // Fallback: first valid block
    image_blocks
        .iter()
        .enumerate()
        .find(|(_, b)| has_required_fields(b))
        .ok_or_else(|| "No image block with required fields found".to_string())
}

fn decode_image_to_2d(
    data_bytes: &[u8],
    samps_per_line: usize,
    number_of_lines: usize,
    bytes_per_pixel: usize,
    start_context: Option<&String>,
) -> Result<Vec<Vec<f32>>, String> {
    let n_pixels = samps_per_line
        .checked_mul(number_of_lines)
        .ok_or_else(|| "Image dimensions are too large".to_string())?;
    let expected_bytes = n_pixels
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "Image byte count overflow".to_string())?;
    if data_bytes.len() < expected_bytes {
        return Err(format!(
            "Not enough image bytes: expected {expected_bytes}, got {}",
            data_bytes.len()
        ));
    }

    let big_endian = start_context
        .map(|v| v.to_ascii_uppercase().contains("BIG"))
        .unwrap_or(false);

    let mut flat = Vec::with_capacity(n_pixels);
    match bytes_per_pixel {
        2 => {
            for chunk in data_bytes[..expected_bytes].chunks_exact(2) {
                let v = if big_endian {
                    i16::from_be_bytes([chunk[0], chunk[1]])
                } else {
                    i16::from_le_bytes([chunk[0], chunk[1]])
                };
                flat.push(v as f32);
            }
        }
        4 => {
            for chunk in data_bytes[..expected_bytes].chunks_exact(4) {
                let v = if big_endian {
                    i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                } else {
                    i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                };
                flat.push(v as f32);
            }
        }
        other => {
            return Err(format!("Unsupported Bytes/pixel value: {other}"));
        }
    }

    let mut image_2d = Vec::with_capacity(number_of_lines);
    for row in flat.chunks_exact(samps_per_line) {
        image_2d.push(row.to_vec());
    }
    image_2d.reverse();
    Ok(image_2d)
}

fn flatten_polynomial(image_2d: &[Vec<f32>], order: u32) -> Result<Vec<f32>, String> {
    if order > 3 {
        return Err("Polynomial order must be 0-3".to_string());
    }

    let n_lines = image_2d.len();
    if n_lines == 0 {
        return Err("Empty image".to_string());
    }

    let n_samples = image_2d[0].len();
    let mut flattened = vec![0.0_f32; n_lines * n_samples];

    for (line_idx, row) in image_2d.iter().enumerate() {
        if row.len() != n_samples {
            return Err("Inconsistent line length".to_string());
        }

        let x: Vec<f32> = (0..n_samples)
            .map(|i| i as f32 / (n_samples - 1).max(1) as f32)
            .collect();
        let y: Vec<f32> = row.to_vec();

        let coeffs = fit_polynomial(&x, &y, order as usize)
            .map_err(|e| format!("Polynomial fitting failed: {e}"))?;

        for (sample_idx, x_val) in x.iter().enumerate() {
            let fit_val = eval_polynomial(&coeffs, *x_val);
            flattened[line_idx * n_samples + sample_idx] = y[sample_idx] - fit_val;
        }
    }

    Ok(flattened)
}

fn fit_polynomial(x: &[f32], y: &[f32], order: usize) -> Result<Vec<f32>, String> {
    if x.len() != y.len() || x.is_empty() {
        return Err("Mismatched or empty x/y arrays".to_string());
    }
    if order > 3 {
        return Err("Order must be 0-3".to_string());
    }

    let n = x.len();
    let m = order + 1;

    let mut a = vec![vec![0.0_f64; m]; m];
    let mut b = vec![0.0_f64; m];

    for i in 0..n {
        let xi = x[i] as f64;
        let yi = y[i] as f64;

        let mut powers = vec![1.0_f64; 2 * m];
        for p in 1..(2 * m) {
            powers[p] = powers[p - 1] * xi;
        }

        for row in 0..m {
            b[row] += yi * powers[row];
            for col in 0..m {
                a[row][col] += powers[row + col];
            }
        }
    }

    solve_linear_system(&mut a, &mut b).map(|coeffs| coeffs.into_iter().map(|v| v as f32).collect())
}

// Classic Gaussian elimination with partial pivoting: rows/columns are
// cross-referenced by index (a[i][k], a[k][j], ...), so index-based loops
// read more clearly here than clippy's iterator-based suggestions.
#[allow(clippy::needless_range_loop)]
fn solve_linear_system(a: &mut [Vec<f64>], b: &mut [f64]) -> Result<Vec<f64>, String> {
    let n = b.len();
    if a.len() != n || a.iter().any(|row| row.len() != n) {
        return Err("Linear solver received invalid matrix dimensions".to_string());
    }

    let eps = 1e-12_f64;

    for k in 0..n {
        let mut pivot_row = k;
        let mut pivot_val = a[k][k].abs();
        for r in (k + 1)..n {
            let v = a[r][k].abs();
            if v > pivot_val {
                pivot_val = v;
                pivot_row = r;
            }
        }

        if pivot_val < eps {
            return Err("Singular matrix while fitting polynomial".to_string());
        }

        if pivot_row != k {
            a.swap(k, pivot_row);
            b.swap(k, pivot_row);
        }

        let pivot = a[k][k];
        for j in k..n {
            a[k][j] /= pivot;
        }
        b[k] /= pivot;

        for i in (k + 1)..n {
            let factor = a[i][k];
            if factor.abs() < eps {
                continue;
            }
            for j in k..n {
                a[i][j] -= factor * a[k][j];
            }
            b[i] -= factor * b[k];
        }
    }

    let mut x = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut sum = b[i];
        for j in (i + 1)..n {
            sum -= a[i][j] * x[j];
        }
        x[i] = sum;
    }

    Ok(x)
}

fn eval_polynomial(coeffs: &[f32], x: f32) -> f32 {
    let mut result = 0.0;
    let mut x_power = 1.0;
    for &coeff in coeffs {
        result += coeff * x_power;
        x_power *= x;
    }
    result
}

/// Look up a header field, ignoring case in both the section name and the key.
///
/// Nanoscope is not consistent about capitalisation: Gwyddion's reader keys on
/// `Scan size` and `Aspect ratio` where our sample files write `Scan Size` and
/// `Aspect Ratio`. An exact-case lookup therefore misses the field on some
/// files and silently falls through to a default, so every header read goes
/// through here. `section` of `None` means the unsectioned key, which holds the
/// first occurrence in file order (see the `or_insert_with` in
/// [`parse_header_and_image_blocks`]); the same call also works on an image
/// block's own field map, whose keys are all unsectioned.
///
/// More than one key can match once case is ignored, so the lexicographically
/// first one wins — that keeps the result independent of `HashMap` order.
fn header_get<'a>(
    metadata: &'a HashMap<String, String>,
    section: Option<&str>,
    key: &str,
) -> Option<&'a String> {
    metadata
        .iter()
        .filter(|(k, _)| {
            let (sec, bare) = match k.split_once("::") {
                Some((s, b)) => (Some(s), b),
                None => (None, k.as_str()),
            };
            bare.eq_ignore_ascii_case(key)
                && match (section, sec) {
                    (None, None) => true,
                    (Some(want), Some(have)) => have.eq_ignore_ascii_case(want),
                    _ => false,
                }
        })
        .min_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, v)| v)
}

/// Split a header length field into its numbers, converted to nm: "4000 nm",
/// "4000", "4 4 ~m" (microns), "5.0 2.5 ~m". The unit applies to every number.
fn parse_lengths_nm(raw: &str) -> Option<(f32, Option<f32>)> {
    // "~m" or "um" means microns, anything else (including a bare number) nm.
    let scale = if raw.contains("~m") || raw.contains("um") || raw.contains("µm") {
        1000.0
    } else {
        1.0
    };
    let mut nums = raw.split_whitespace().filter_map(|s| s.parse::<f32>().ok());
    let x = nums.next()?;
    Some((x * scale, nums.next().map(|y| y * scale)))
}

/// Whether an `Aspect Ratio` field ("1:1", "2:1", …) describes a rectangular
/// scan. Matches Gwyddion: the literal "1:1" is square, otherwise the leading
/// number decides. A missing field means square.
fn is_nonsquare_aspect(raw: Option<&String>) -> bool {
    let Some(raw) = raw.map(|v| v.trim()) else {
        return false;
    };
    if raw == "1:1" {
        return false;
    }
    match raw
        .split(':')
        .next()
        .and_then(|t| t.trim().parse::<f32>().ok())
    {
        Some(v) => v > 0.0 && v != 1.0,
        None => false,
    }
}

/// Lateral scan extents in nm as (x, y): x along the fast (line) axis, y along
/// the slow axis — the extent the image's rows span.
///
/// This mirrors Gwyddion's `nanoscope.c`, the closest thing to a reference
/// reader for the format, which resolves y in three layers:
///
/// 1. `Scan Size` gives x, and its second number — when the file spells one
///    out — gives y. Older files carry a single number and mean a square scan.
/// 2. `Slow Axis Size` in the scan list names the slow extent outright, so it
///    overrides the pair above when present.
/// 3. When `Aspect Ratio` is not 1:1, the image's own line count has to be
///    weighed against the scan list's `Lines`: a rectangular scan, or one
///    captured before the frame finished, covers only part of the slow travel.
///
/// `cols`/`rows` are the loaded image block's `Samps/line` and
/// `Number of lines`; `image_fields` is that block's own field map. Every part
/// of step 3 is a no-op for a 1:1 scan, which is the common case.
fn parse_scan_size_nm_xy(
    metadata: &HashMap<String, String>,
    image_fields: &HashMap<String, String>,
    cols: usize,
    rows: usize,
) -> (f32, f32) {
    const DEFAULT: f32 = 1000.0;
    // Nanoscope records `Scan Size` more than once, and the forms differ: the
    // scan list often carries just the fast axis ("1000 nm") while the image
    // blocks spell out both ("1 1 ~m"). Prefer whichever explicitly gives two
    // numbers, trying the sections in a fixed order so the result never depends
    // on `HashMap` iteration order; the bare key is the fallback.
    let candidates: Vec<&String> = ["Ciao scan list", "Ciao image list"]
        .iter()
        .filter_map(|s| header_get(metadata, Some(s), "Scan Size"))
        .chain(header_get(metadata, None, "Scan Size"))
        .collect();
    let raw = candidates
        .iter()
        .find(|v| parse_lengths_nm(v).is_some_and(|(_, y)| y.is_some()))
        .or_else(|| candidates.iter().find(|v| parse_lengths_nm(v).is_some()));

    let Some((x, y)) = raw.and_then(|v| parse_lengths_nm(v)) else {
        return (DEFAULT, DEFAULT);
    };
    let mut y = y.unwrap_or(x);

    let have_slow_axis_size = match header_get(metadata, Some("Ciao scan list"), "Slow Axis Size")
        .and_then(|v| parse_lengths_nm(v))
    {
        Some((slow, _)) if slow > 0.0 => {
            y = slow;
            true
        }
        _ => false,
    };

    // Aspect ratio is read from the loaded block and from the scan list
    // separately, because Gwyddion branches on the two independently.
    let nonsquare_image = is_nonsquare_aspect(header_get(image_fields, None, "Aspect Ratio"));
    let nonsquare_scan =
        is_nonsquare_aspect(header_get(metadata, Some("Ciao scan list"), "Aspect Ratio"));
    let scan_rows = header_get(metadata, Some("Ciao scan list"), "Lines")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|g| *g > 0);

    if nonsquare_image && rows > 0 {
        if !have_slow_axis_size {
            if nonsquare_scan {
                // Gwyddion carries this branch with "Reported by Peter Eaton.
                // Not sure if we detect it correctly." Kept as it stands there
                // rather than replaced by a guess of our own.
                if cols > 0 {
                    y = y * rows as f32 / cols as f32;
                }
            } else if let Some(g) = scan_rows {
                y = y * rows as f32 / g as f32;
            }
        } else if let Some(g) = scan_rows.filter(|g| rows < *g) {
            // "Capture Now": the frame was saved with fewer lines than the scan
            // was set up for, so it spans proportionally less of the slow axis.
            y = y * rows as f32 / g as f32;
        }
    }

    (x, y)
}

/// Instrument identifier (e.g. "10396jvlr"), read from the header's
/// `\Serial Number` line.
///
/// The calibration corrects the *scanner*, so `Scanner list` wins; the
/// unsectioned key — the first occurrence in file order — is the fallback for
/// headers that record the serial elsewhere. Without that preference a file
/// that also names a serial under `Equipment list` (which comes first) would
/// key the calibration on the microscope body, and two scanners used on one
/// body would share a single entry.
pub fn instrument_id(metadata: &HashMap<String, String>) -> Option<String> {
    fn normalize(raw: &str) -> Option<String> {
        let id = raw.trim().trim_matches('"').trim().to_ascii_lowercase();
        (!id.is_empty()).then_some(id)
    }
    header_get(metadata, Some("Scanner list"), "Serial Number")
        .and_then(|v| normalize(v))
        .or_else(|| header_get(metadata, None, "Serial Number").and_then(|v| normalize(v)))
}

pub struct SpmImage {
    pub data: Vec<f32>, // flat row-major: data[row * samps_per_line + col]
    // Kept for debugging/display purposes even though nothing reads it yet.
    #[allow(dead_code)]
    pub metadata: HashMap<String, String>,
    /// Extent the image's columns span, along the fast (line) axis.
    pub scan_size_x_nm: f32,
    /// Extent the image's rows span, along the slow axis. "y" is the image's
    /// vertical axis, not the scanner's: with a non-zero `\Rotate Ang.` the
    /// fast/slow pair is rotated away from the piezo's X/Y. Everything that
    /// reads this wants the image axis, so the rotation does not matter here.
    pub scan_size_y_nm: f32,
    pub samps_per_line: usize,
    pub number_of_lines: usize,
    #[allow(dead_code)]
    pub channel_name: String,
    pub channel_idx: usize,
    pub available_channels: Vec<ChannelInfo>,
    /// Instrument this image came from, from the header's `\Serial Number`.
    pub instrument_id: Option<String>,
    /// Calibration already folded into `scan_size_*_nm` and `data`.
    /// [`Calibration::UNITY`] means the header values are used as-is.
    pub calibration: Calibration,
}

impl SpmImage {
    /// Apply a per-instrument calibration: `x`/`y` scale the lateral extents,
    /// `z` scales the height samples. What was applied is recorded so the UI
    /// can warn while it is in effect.
    ///
    /// Scaling Z here — after flattening, smoothing and any rolling-ball
    /// subtraction — gives the same result as scaling the raw samples would,
    /// because every one of those steps is linear in the data.
    pub fn apply_calibration(&mut self, cal: Calibration) {
        self.scan_size_x_nm *= cal.x;
        self.scan_size_y_nm *= cal.y;
        if (cal.z - 1.0).abs() > f32::EPSILON {
            for v in self.data.iter_mut() {
                *v *= cal.z;
            }
        }
        self.calibration = cal;
    }
}

fn apply_gaussian_filter(data: &[f32], width: usize, height: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return data.to_vec();
    }
    let radius = (3.0 * sigma).ceil() as usize;
    let kernel: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let x = i as f32 - radius as f32;
            (-x * x / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let ksum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|&k| k / ksum).collect();

    // Horizontal pass
    let mut tmp = vec![0.0_f32; width * height];
    for r in 0..height {
        for c in 0..width {
            let mut acc = 0.0_f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let cc = (c + ki).saturating_sub(radius).min(width - 1);
                acc += data[r * width + cc] * kv;
            }
            tmp[r * width + c] = acc;
        }
    }

    // Vertical pass
    let mut out = vec![0.0_f32; width * height];
    for r in 0..height {
        for c in 0..width {
            let mut acc = 0.0_f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let rr = (r + ki).saturating_sub(radius).min(height - 1);
                acc += tmp[rr * width + c] * kv;
            }
            out[r * width + c] = acc;
        }
    }
    out
}

/// Read file bytes.
/// On macOS, uses NSFileCoordinator so that FileProvider-backed files
/// (iCloud Drive, Dropbox, Google Drive, etc.) are downloaded on demand
/// before the data is returned.
fn read_file_bytes(path: &Path) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    return coordinate_read(path, |p| {
        std::fs::read(p).map_err(|e| format!("Failed to read file: {e}"))
    })?;

    #[cfg(not(target_os = "macos"))]
    std::fs::read(path).map_err(|e| format!("Failed to read file: {e}"))
}

/// Run `accessor` inside an `NSFileCoordinator` coordinated read of `path`.
///
/// The coordinator transparently triggers a FileProvider download when the
/// file is a cloud placeholder, waiting until the data is available locally
/// before invoking `accessor` with the (possibly relocated) coordinated path.
/// This is the single FFI surface shared by on-demand reads and background
/// prefetching, replacing the previous approach of shelling out to
/// `fileproviderctl`/`brctl` and polling `stat::blocks`.
#[cfg(target_os = "macos")]
fn coordinate_read<R>(path: &Path, accessor: impl FnOnce(&Path) -> R) -> Result<R, String> {
    use core::ptr::NonNull;
    use objc2_foundation::{
        NSError, NSFileCoordinator, NSFileCoordinatorReadingOptions, NSString, NSURL,
    };
    use std::cell::Cell;

    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let coordinator = NSFileCoordinator::new();

    // The block is invoked synchronously on this thread before
    // `coordinateReading…` returns, so `Cell`s carry the accessor in and its
    // result out — no raw pointers, no cross-thread aliasing.
    let accessor = Cell::new(Some(accessor));
    let result: Cell<Option<R>> = Cell::new(None);

    let block = block2::StackBlock::new(|new_url: NonNull<NSURL>| {
        // SAFETY: NSFileCoordinator passes a valid, non-null coordinated URL.
        let coordinated = unsafe { new_url.as_ref() };
        let path_str = coordinated
            .path()
            .expect("coordinated URL has no path")
            .to_string();
        let f = accessor.take().expect("accessor invoked exactly once");
        result.set(Some(f(Path::new(&path_str))));
    });

    let mut out_error: Option<objc2::rc::Retained<NSError>> = None;
    coordinator.coordinateReadingItemAtURL_options_error_byAccessor(
        &url,
        NSFileCoordinatorReadingOptions(0),
        Some(&mut out_error),
        &block,
    );
    if let Some(err) = out_error {
        let desc = err.localizedDescription();
        return Err(format!("Failed to read from cloud storage: {desc}"));
    }

    result.take().ok_or_else(|| {
        format!(
            "Could not open the file. If it lives in cloud storage, download it \
             manually and try again.\n(path: {})",
            path.display()
        )
    })
}

/// Kick off background downloads for any cloud-placeholder files in `paths`.
///
/// - iCloud Drive: `NSFileManager.startDownloadingUbiquitousItemAtURL` — fires
///   asynchronously; the OS downloads in the background.
/// - Other FileProviders (Dropbox, Google Drive, …): a coordinated read with an
///   empty accessor ([`coordinate_read`]) — blocks the background thread until
///   each file is local.
///
/// Returns immediately; the work runs on a detached thread.
#[cfg(target_os = "macos")]
pub fn prefetch_cloud_files(paths: Vec<std::path::PathBuf>) {
    std::thread::spawn(move || {
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        use std::os::unix::fs::MetadataExt as _;

        let fm = NSFileManager::new();

        for path in &paths {
            let is_placeholder = std::fs::metadata(path)
                .map(|m| m.blocks() == 0 && m.len() > 0)
                .unwrap_or(false);
            if !is_placeholder {
                continue;
            }

            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));

            if fm.isUbiquitousItemAtURL(&url) {
                // iCloud: enqueue download and move on — OS handles the rest.
                let _ = fm.startDownloadingUbiquitousItemAtURL_error(&url);
            } else {
                // Other FileProviders: a coordinated read with an empty accessor
                // materialises the file; blocks this thread until it is local.
                let _ = coordinate_read(path, |_| ());
            }
        }
    });
}

/// No-op on non-macOS targets: cloud-placeholder prefetching is a macOS
/// FileProvider concept, so callers need no platform guard.
#[cfg(not(target_os = "macos"))]
pub fn prefetch_cloud_files(_paths: Vec<std::path::PathBuf>) {}

pub fn load_spm(
    path: &Path,
    flatten: Option<u32>,
    smooth_sigma: f32,
    channel_idx: Option<usize>,
) -> Result<SpmImage, String> {
    let bytes = read_file_bytes(path)?;

    let marker = b"\\*File list end";
    let marker_pos = find_bytes(&bytes, marker)
        .ok_or_else(|| "Could not find \\*File list end marker in file".to_string())?;
    let header_end = marker_pos + marker.len();
    let header_text = String::from_utf8_lossy(&bytes[..header_end]);

    let (measurement_conditions, image_blocks) = parse_header_and_image_blocks(&header_text);
    let available_channels = list_channels_from_blocks(&image_blocks);
    let (loaded_idx, image_block) = choose_image_block(&image_blocks, channel_idx)?;
    let channel_name = available_channels
        .iter()
        .find(|c| c.index == loaded_idx)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Channel {loaded_idx}"));

    let data_offset = parse_usize_field(image_block.fields.get("Data offset"), "Data offset")?;
    let data_length = parse_usize_field(image_block.fields.get("Data length"), "Data length")?;
    let samps_per_line = parse_usize_field(image_block.fields.get("Samps/line"), "Samps/line")?;
    let number_of_lines =
        parse_usize_field(image_block.fields.get("Number of lines"), "Number of lines")?;
    let bytes_per_pixel = parse_usize_field(image_block.fields.get("Bytes/pixel"), "Bytes/pixel")?;

    let end_offset = data_offset
        .checked_add(data_length)
        .ok_or_else(|| "Data offset overflow".to_string())?;
    if end_offset > bytes.len() {
        return Err(format!(
            "Data slice out of range: offset={data_offset}, length={data_length}, file_size={}",
            bytes.len()
        ));
    }

    let image_data = &bytes[data_offset..end_offset];
    let mut image_2d = decode_image_to_2d(
        image_data,
        samps_per_line,
        number_of_lines,
        bytes_per_pixel,
        image_block.fields.get("Start context"),
    )?;

    let nm_per_lsb = parse_nm_per_lsb(image_block, &measurement_conditions, bytes_per_pixel)?;
    apply_scale_in_place(&mut image_2d, nm_per_lsb);

    let data = if let Some(order) = flatten {
        flatten_polynomial(&image_2d, order)?
    } else {
        image_2d.iter().flatten().copied().collect()
    };
    let data = apply_gaussian_filter(&data, samps_per_line, number_of_lines, smooth_sigma);

    let (scan_size_x_nm, scan_size_y_nm) = parse_scan_size_nm_xy(
        &measurement_conditions,
        &image_block.fields,
        samps_per_line,
        number_of_lines,
    );
    let instrument_id = instrument_id(&measurement_conditions);

    Ok(SpmImage {
        data,
        metadata: measurement_conditions,
        scan_size_x_nm,
        scan_size_y_nm,
        samps_per_line,
        number_of_lines,
        channel_name,
        channel_idx: loaded_idx,
        available_channels,
        instrument_id,
        // Calibration is applied by the caller, which owns the lookup table.
        calibration: Calibration::UNITY,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Scan size for a square 512x512 image with no aspect-ratio quirks — the
    /// shape of every call that does not exercise the step-3 corrections.
    fn scan_size(pairs: &[(&str, &str)]) -> (f32, f32) {
        parse_scan_size_nm_xy(&meta(pairs), &HashMap::new(), 512, 512)
    }

    #[test]
    fn scan_size_reads_both_axes() {
        assert_eq!(
            scan_size(&[("Scan Size", "5.00000 5.00000 ~m")]),
            (5000.0, 5000.0)
        );
        assert_eq!(scan_size(&[("Scan Size", "5.0 2.5 ~m")]), (5000.0, 2500.0));
    }

    #[test]
    fn scan_size_prefers_the_form_that_spells_out_both_axes() {
        // Real Nanoscope files put a fast-axis-only value on the bare key and
        // the explicit pair in a section; the pair must win.
        let m = [
            ("Scan Size", "1000 nm"),
            ("Ciao scan list::Scan Size", "1000 nm"),
            ("Ciao image list::Scan Size", "1 0.5 ~m"),
        ];
        assert_eq!(scan_size(&m), (1000.0, 500.0));

        // The scan list wins over the image list when both spell out two axes.
        let m = [
            ("Scan Size", "1000 nm"),
            ("Ciao scan list::Scan Size", "1 1 ~m"),
            ("Ciao image list::Scan Size", "2 2 ~m"),
        ];
        assert_eq!(scan_size(&m), (1000.0, 1000.0));
    }

    #[test]
    fn scan_size_falls_back_to_a_single_number_for_both_axes() {
        assert_eq!(scan_size(&[("Scan Size", "4000 nm")]), (4000.0, 4000.0));
        assert_eq!(scan_size(&[("Scan Size", "4000")]), (4000.0, 4000.0));
        // Missing or unparseable: fall back to the 1 µm default.
        assert_eq!(scan_size(&[]), (1000.0, 1000.0));
        assert_eq!(scan_size(&[("Scan Size", "nonsense")]), (1000.0, 1000.0));
    }

    #[test]
    fn scan_size_keys_are_matched_ignoring_case() {
        // Gwyddion keys on `Scan size`; our sample files write `Scan Size`.
        // Either spelling has to resolve, or the reader silently returns the
        // 1 µm default and every distance in the app is wrong by that factor.
        assert_eq!(scan_size(&[("Scan size", "5 2.5 ~m")]), (5000.0, 2500.0));
        assert_eq!(
            scan_size(&[("ciao SCAN list::scan size", "5 2.5 ~m")]),
            (5000.0, 2500.0)
        );
        assert_eq!(
            scan_size(&[
                ("Scan size", "1000 nm"),
                ("Ciao scan list::Scan size", "1000 nm"),
                ("Ciao scan list::Slow axis size", "500 nm"),
            ]),
            (1000.0, 500.0)
        );
    }

    #[test]
    fn slow_axis_size_overrides_the_scan_size_pair() {
        // Gwyddion gives the scan list's `Slow Axis Size` precedence: it names
        // the slow extent outright, where the pair is only implied.
        let m = [
            ("Ciao image list::Scan Size", "1 1 ~m"),
            ("Ciao scan list::Slow Axis Size", "400 nm"),
        ];
        assert_eq!(scan_size(&m), (1000.0, 400.0));

        // A missing or degenerate value leaves the pair alone.
        let m = [
            ("Ciao image list::Scan Size", "1 0.5 ~m"),
            ("Ciao scan list::Slow Axis Size", "0 nm"),
        ];
        assert_eq!(scan_size(&m), (1000.0, 500.0));
    }

    #[test]
    fn a_square_aspect_ratio_never_rescales_the_slow_axis() {
        // The step-3 corrections all hang off a non-1:1 `Aspect Ratio`. Guard
        // the common case: even with a line count that disagrees with the scan
        // list, a 1:1 scan must come through untouched.
        let m = meta(&[
            ("Ciao image list::Scan Size", "1 1 ~m"),
            ("Ciao scan list::Slow Axis Size", "1000 nm"),
            ("Ciao scan list::Aspect Ratio", "1:1"),
            ("Ciao scan list::Lines", "512"),
        ]);
        let block = meta(&[("Aspect Ratio", "1:1")]);
        assert_eq!(
            parse_scan_size_nm_xy(&m, &block, 512, 128),
            (1000.0, 1000.0)
        );
    }

    #[test]
    fn a_partly_captured_rectangular_frame_spans_less_of_the_slow_axis() {
        // "Capture Now" on a 2:1 scan: the scan was set up for 256 lines but
        // the frame was saved after 128, so it covers half the slow travel.
        let m = meta(&[
            ("Ciao scan list::Scan Size", "1000 nm"),
            ("Ciao scan list::Slow Axis Size", "500 nm"),
            ("Ciao scan list::Aspect Ratio", "2:1"),
            ("Ciao scan list::Lines", "256"),
        ]);
        let block = meta(&[("Aspect Ratio", "2:1")]);
        assert_eq!(parse_scan_size_nm_xy(&m, &block, 512, 128), (1000.0, 250.0));
        // The complete frame is left alone.
        assert_eq!(parse_scan_size_nm_xy(&m, &block, 512, 256), (1000.0, 500.0));
    }

    #[test]
    fn a_rectangular_scan_without_slow_axis_size_is_scaled_by_the_line_counts() {
        // No `Slow Axis Size`, scan list still 1:1: y follows the image's line
        // count against the scan list's.
        let m = meta(&[
            ("Ciao image list::Scan Size", "1 1 ~m"),
            ("Ciao scan list::Aspect Ratio", "1:1"),
            ("Ciao scan list::Lines", "512"),
        ]);
        let block = meta(&[("Aspect Ratio", "2:1")]);
        assert_eq!(parse_scan_size_nm_xy(&m, &block, 512, 256), (1000.0, 500.0));

        // Both sections non-square: Gwyddion switches to the image's own grid.
        let m = meta(&[
            ("Ciao image list::Scan Size", "1 1 ~m"),
            ("Ciao scan list::Aspect Ratio", "2:1"),
            ("Ciao scan list::Lines", "512"),
        ]);
        assert_eq!(parse_scan_size_nm_xy(&m, &block, 512, 256), (1000.0, 500.0));
    }

    #[test]
    fn aspect_ratio_classification_matches_gwyddion() {
        assert!(!is_nonsquare_aspect(None));
        assert!(!is_nonsquare_aspect(Some(&"1:1".to_string())));
        assert!(!is_nonsquare_aspect(Some(&"junk".to_string())));
        assert!(is_nonsquare_aspect(Some(&"2:1".to_string())));
        assert!(is_nonsquare_aspect(Some(&" 0.5:1 ".to_string())));
    }

    #[test]
    fn instrument_id_is_case_insensitive_and_normalised() {
        assert_eq!(
            instrument_id(&meta(&[("Serial number", "10396JVLR")])),
            Some("10396jvlr".to_string())
        );
        assert_eq!(
            instrument_id(&meta(&[("Serial Number", "  \"10396jvlr\"  ")])),
            Some("10396jvlr".to_string())
        );
        assert_eq!(instrument_id(&meta(&[("Serial Number", "  ")])), None);
        assert_eq!(instrument_id(&meta(&[("Date", "2026-01-01")])), None);
    }

    #[test]
    fn instrument_id_ignores_unrelated_sections() {
        // The bare key is the fallback, so the result does not depend on
        // `HashMap` iteration order when both forms are present.
        let m = meta(&[
            ("Equipment list::Serial number", "sectioned"),
            ("Serial number", "10396jvlr"),
        ]);
        assert_eq!(instrument_id(&m), Some("10396jvlr".to_string()));

        let only_equipment = meta(&[("Equipment list::Serial number", "sectioned")]);
        assert_eq!(instrument_id(&only_equipment), None);
    }

    #[test]
    fn instrument_id_prefers_the_scanner_over_the_microscope_body() {
        // `\*Equipment list` comes before `\*Scanner list` in the file, so the
        // bare key holds the body's serial when both are recorded. The
        // calibration corrects the scanner, so the scanner's serial must win —
        // otherwise two scanners on one body would share a single entry.
        let m = meta(&[
            ("Equipment list::Serial Number", "BODY-1"),
            ("Serial Number", "BODY-1"),
            ("Scanner list::Serial Number", "11400EVLR"),
        ]);
        assert_eq!(instrument_id(&m), Some("11400evlr".to_string()));

        // An empty scanner entry still falls back rather than yielding nothing.
        let m = meta(&[
            ("Scanner list::Serial Number", "   "),
            ("Serial Number", "11400EVLR"),
        ]);
        assert_eq!(instrument_id(&m), Some("11400evlr".to_string()));
    }

    /// A minimal but structurally faithful Nanoscope file: a text header
    /// terminated by the `\*File list end` marker, padded out to `DATA_OFFSET`,
    /// followed by 4x2 little-endian i16 samples.
    fn write_synthetic_spm(path: &Path) {
        const DATA_OFFSET: usize = 1024;
        // Section order and key spellings follow a real MultiMode header: the
        // body's serial appears first, the scanner's second, and the scan list
        // carries the fast axis on `Scan Size` with the slow one beside it.
        let header = r#"\*File list
\Version: 0x09300000
\*Equipment list
\Description: MultiMode
\Serial number: BODY-0000
\*Scanner list
\Serial number: 10396JVLR
\*Ciao scan list
\Scan Size: 5.00000 2.50000 ~m
\Aspect Ratio: 1:1
\Lines: 2
\*Ciao image list
\Data offset: 1024
\Data length: 16
\Samps/line: 4
\Number of lines: 2
\Bytes/pixel: 2
\@2:Image Data: S [Height] "Height"
\@2:Z scale: V [Sens. Zsens] (0.5 nm/LSB) 100 nm
\*File list end
"#;
        let mut bytes = header.as_bytes().to_vec();
        assert!(bytes.len() < DATA_OFFSET);
        bytes.resize(DATA_OFFSET, b' ');
        for v in 0i16..8 {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, bytes).expect("write synthetic spm");
    }

    #[test]
    fn load_spm_reads_the_serial_number_and_both_scan_extents() {
        let path = std::env::temp_dir().join("kintuba_synthetic_test.spm");
        write_synthetic_spm(&path);

        let img = load_spm(&path, None, 0.0, None).expect("synthetic file should parse");
        let _ = std::fs::remove_file(&path);

        // The scanner's serial, not the body's, even though the body's comes
        // first in the file.
        assert_eq!(img.instrument_id.as_deref(), Some("10396jvlr"));
        assert_eq!(img.samps_per_line, 4);
        assert_eq!(img.number_of_lines, 2);
        assert_eq!(img.channel_name, "Height");
        // "5.00000 2.50000 ~m" -> both axes in nm, Y half of X.
        assert_eq!(img.scan_size_x_nm, 5000.0);
        assert_eq!(img.scan_size_y_nm, 2500.0);
        assert_eq!(img.calibration, Calibration::UNITY);
        // 0.5 nm/LSB applied to the raw counts, rows reversed.
        assert_eq!(img.data, vec![2.0, 2.5, 3.0, 3.5, 0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn apply_calibration_scales_extents_and_records_the_factors() {
        let mut img = SpmImage {
            data: vec![4.0, 8.0, 12.0, 16.0],
            metadata: HashMap::new(),
            scan_size_x_nm: 1000.0,
            scan_size_y_nm: 1000.0,
            samps_per_line: 2,
            number_of_lines: 2,
            channel_name: "Height".into(),
            channel_idx: 0,
            available_channels: Vec::new(),
            instrument_id: Some("10396jvlr".into()),
            calibration: Calibration::UNITY,
        };
        img.apply_calibration(Calibration {
            x: 1.0,
            y: 1.2,
            z: 0.5,
        });
        assert_eq!(img.scan_size_x_nm, 1000.0);
        assert!((img.scan_size_y_nm - 1200.0).abs() < 1e-3);
        // Z scales the height samples themselves.
        assert_eq!(img.data, vec![2.0, 4.0, 6.0, 8.0]);
        assert_eq!(img.calibration.z, 0.5);
    }
}
