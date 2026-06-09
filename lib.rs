use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::fs;
use ndarray::Array2;
use numpy::ToPyArray;

#[derive(Debug, Default, Clone)]
struct ImageBlock {
    fields: HashMap<String, String>,
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

fn parse_nm_per_lsb(
    image_block: &ImageBlock,
    measurement_conditions: &HashMap<String, String>,
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

    let sens_name = parse_sensitivity_name(z_scale_text)
        .ok_or_else(|| format!("Could not parse sensitivity reference from Z scale: {z_scale_text}"))?;
    let sens_key = format!("@Sens. {sens_name}");
    let sens_value = measurement_conditions
        .get(&sens_key)
        .ok_or_else(|| format!("Missing sensitivity field: {sens_key}"))?;
    let nm_per_v = parse_float_before_unit(sens_value, "nm/V")
        .ok_or_else(|| format!("Could not parse nm/V from sensitivity field {sens_key}: {sens_value}"))?;

    Ok(v_per_lsb * nm_per_v)
}

fn apply_scale_in_place(image_2d: &mut [Vec<f32>], scale: f32) {
    for row in image_2d.iter_mut() {
        for v in row.iter_mut() {
            *v *= scale;
        }
    }
}

fn choose_image_block(image_blocks: &[ImageBlock]) -> Result<&ImageBlock, String> {
    let has_required_fields = |b: &ImageBlock| {
        b.fields.contains_key("Data offset")
            && b.fields.contains_key("Data length")
            && b.fields.contains_key("Samps/line")
            && b.fields.contains_key("Number of lines")
            && b.fields.contains_key("Bytes/pixel")
    };

    // Match spmpy behavior: duplicate image names are overwritten, so the last
    // valid Height block wins (typically Retrace when both Trace/Retrace exist).
    if let Some(block) = image_blocks.iter().rfind(|b| {
        let has_height = b.fields.iter().any(|(k, v)| {
            (k.ends_with(":Image Data") || k == "Image Data") && v.contains("Height")
        });
        has_required_fields(b)
            && has_height
    }) {
        return Ok(block);
    }

    image_blocks
        .iter()
        .find(|b| has_required_fields(b))
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

/// Apply polynomial line-by-line flattening.
///
/// For each line, fit and subtract:
/// - order 0: z = a
/// - order 1: z = a + b x
/// - order 2: z = a + b x + c x^2
/// - order 3: z = a + b x + c x^2 + d x^3
fn flatten_polynomial(
    image_2d: &[Vec<f32>],
    order: u32,
) -> Result<Array2<f32>, String> {
    if order > 3 {
        return Err("Polynomial order must be 0-3".to_string());
    }

    let n_lines = image_2d.len();
    if n_lines == 0 {
        return Err("Empty image".to_string());
    }

    let n_samples = image_2d[0].len();
    let mut flattened = Array2::zeros((n_lines, n_samples));

    for (line_idx, row) in image_2d.iter().enumerate() {
        if row.len() != n_samples {
            return Err("Inconsistent line length".to_string());
        }

        // Build x coordinates (0.0 to 1.0 normalized)
        let x: Vec<f32> = (0..n_samples)
            .map(|i| i as f32 / (n_samples - 1).max(1) as f32)
            .collect();

        // Build y values
        let y: Vec<f32> = row.to_vec();

        // Fit polynomial using least squares and subtract fitted baseline per line.
        let coeffs = fit_polynomial(&x, &y, order as usize)
            .map_err(|e| format!("Polynomial fitting failed: {e}"))?;

        // Subtract fitted polynomial from original data
        for (sample_idx, x_val) in x.iter().enumerate() {
            let fit_val = eval_polynomial(&coeffs, *x_val);
            flattened[[line_idx, sample_idx]] = y[sample_idx] - fit_val;
        }
    }

    Ok(flattened)
}

/// Fit polynomial of given order using least squares method
/// Returns coefficients [a0, a1, a2, ...] where y = a0 + a1*x + a2*x^2 + ...
fn fit_polynomial(x: &[f32], y: &[f32], order: usize) -> Result<Vec<f32>, String> {
    if x.len() != y.len() || x.is_empty() {
        return Err("Mismatched or empty x/y arrays".to_string());
    }
    if order > 3 {
        return Err("Order must be 0-3".to_string());
    }

    let n = x.len();
    let m = order + 1;

    // Normal equations: (X^T X) c = X^T y
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

    solve_linear_system(&mut a, &mut b)
        .map(|coeffs| coeffs.into_iter().map(|v| v as f32).collect())
}

/// Solve A x = b with Gaussian elimination and partial pivoting.
fn solve_linear_system(a: &mut [Vec<f64>], b: &mut [f64]) -> Result<Vec<f64>, String> {
    let n = b.len();
    if a.len() != n || a.iter().any(|row| row.len() != n) {
        return Err("Linear solver received invalid matrix dimensions".to_string());
    }

    let eps = 1e-12_f64;

    for k in 0..n {
        let mut pivot_row = k;
        let mut pivot_val = a[k][k].abs();
        for (r, row) in a.iter().enumerate().skip(k + 1).take(n.saturating_sub(k + 1)) {
            let v = row[k].abs();
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
        for (j, xj) in x.iter().enumerate().skip(i + 1).take(n.saturating_sub(i + 1)) {
            sum -= a[i][j] * *xj;
        }
        x[i] = sum;
    }

    Ok(x)
}

/// Evaluate polynomial at given x
fn eval_polynomial(coeffs: &[f32], x: f32) -> f32 {
    let mut result = 0.0;
    let mut x_power = 1.0;
    for &coeff in coeffs {
        result += coeff * x_power;
        x_power *= x;
    }
    result
}

#[pyfunction]
fn hello_from_bin() -> String {
    "Hello from afm-image-loader!".to_string()
}

#[pyfunction]
#[pyo3(signature = (path, flatten=None))]
fn _load_spm_raw(
    py: Python<'_>,
    path: &str,
    flatten: Option<u32>,
) -> PyResult<(HashMap<String, String>, PyObject)> {
    let bytes = fs::read(path)
        .map_err(|e| PyValueError::new_err(format!("Failed to read file: {e}")))?;

    let marker = b"\\*File list end";
    let marker_pos = find_bytes(&bytes, marker)
        .ok_or_else(|| PyValueError::new_err("Could not find \\*File list end marker in file"))?;
    let header_end = marker_pos + marker.len();
    let header_text = String::from_utf8_lossy(&bytes[..header_end]);

    let (measurement_conditions, image_blocks) = parse_header_and_image_blocks(&header_text);
    let image_block = choose_image_block(&image_blocks).map_err(PyValueError::new_err)?;

    let data_offset = parse_usize_field(image_block.fields.get("Data offset"), "Data offset")
        .map_err(PyValueError::new_err)?;
    let data_length = parse_usize_field(image_block.fields.get("Data length"), "Data length")
        .map_err(PyValueError::new_err)?;
    let samps_per_line =
        parse_usize_field(image_block.fields.get("Samps/line"), "Samps/line")
            .map_err(PyValueError::new_err)?;
    let number_of_lines =
        parse_usize_field(image_block.fields.get("Number of lines"), "Number of lines")
            .map_err(PyValueError::new_err)?;
    let bytes_per_pixel = parse_usize_field(image_block.fields.get("Bytes/pixel"), "Bytes/pixel")
        .map_err(PyValueError::new_err)?;

    let end_offset = data_offset
        .checked_add(data_length)
        .ok_or_else(|| PyValueError::new_err("Data offset overflow"))?;
    if end_offset > bytes.len() {
        return Err(PyValueError::new_err(format!(
            "Data slice out of range: offset={data_offset}, length={data_length}, file_size={}",
            bytes.len()
        )));
    }

    let image_data = &bytes[data_offset..end_offset];
    let image_2d = decode_image_to_2d(
        image_data,
        samps_per_line,
        number_of_lines,
        bytes_per_pixel,
        image_block.fields.get("Start context"),
    )
    .map_err(PyValueError::new_err)?;

    // Convert raw LSB values to nanometers using image block scale metadata.
    let nm_per_lsb = parse_nm_per_lsb(image_block, &measurement_conditions)
        .map_err(PyValueError::new_err)?;
    let mut image_2d_nm = image_2d;
    apply_scale_in_place(&mut image_2d_nm, nm_per_lsb);

    // Apply flattening if requested
    let result_array = if let Some(order) = flatten {
        let array = flatten_polynomial(&image_2d_nm, order)
            .map_err(PyValueError::new_err)?;
        array.to_pyarray_bound(py).to_object(py)
    } else {
        // Convert Vec<Vec<f32>> to ndarray Array2, then to PyArray2
        let mut flat_vec = Vec::new();
        for row in &image_2d_nm {
            flat_vec.extend_from_slice(row);
        }
        let array = Array2::from_shape_vec(
            (image_2d_nm.len(), samps_per_line),
            flat_vec,
        )
        .map_err(|e| PyValueError::new_err(format!("Failed to create array: {e}")))?;
        array.to_pyarray_bound(py).to_object(py)
    };

    Ok((measurement_conditions, result_array))
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(hello_from_bin, m)?)?;
    m.add_function(wrap_pyfunction!(_load_spm_raw, m)?)?;
    Ok(())
}
