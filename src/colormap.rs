use egui::ColorImage;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Colormap {
    AfmHot,
    Gray,
    Viridis,
}

impl Colormap {
    pub const ALL: &'static [Colormap] = &[Colormap::AfmHot, Colormap::Gray, Colormap::Viridis];

    pub fn name(self) -> &'static str {
        match self {
            Colormap::AfmHot => "AFM Hot",
            Colormap::Gray => "Gray",
            Colormap::Viridis => "Viridis",
        }
    }

    fn map_u8(self, t: f32) -> [u8; 3] {
        let t = t.clamp(0.0, 1.0);
        match self {
            Colormap::Gray => {
                let v = (t * 255.0) as u8;
                [v, v, v]
            }
            Colormap::AfmHot => afm_hot(t),
            Colormap::Viridis => viridis(t),
        }
    }
}

pub fn to_rgba_bytes(
    data: &[f32],
    rows: usize,
    cols: usize,
    cmap: Colormap,
    z_min: f32,
    z_max: f32,
) -> Vec<u8> {
    let range = (z_max - z_min).max(f32::EPSILON);
    let mut bytes = Vec::with_capacity(rows * cols * 4);
    for &v in data {
        let t = (v - z_min) / range;
        let [r, g, b] = cmap.map_u8(t);
        bytes.extend_from_slice(&[r, g, b, 255]);
    }
    bytes
}

pub fn to_color_image(
    data: &[f32],
    rows: usize,
    cols: usize,
    cmap: Colormap,
    z_min: f32,
    z_max: f32,
) -> ColorImage {
    let range = (z_max - z_min).max(f32::EPSILON);
    let pixels = data
        .iter()
        .map(|&v| {
            let t = (v - z_min) / range;
            let [r, g, b] = cmap.map_u8(t);
            egui::Color32::from_rgb(r, g, b)
        })
        .collect();
    ColorImage::new([cols, rows], pixels)
}

fn sigmoid(t: f32, sigma: f32, z: f32) -> f32 {
    1.0 / (1.0 + (-sigma * (t - z)).exp())
}

// afmhot: R rises first (0→1/3), then G (1/3→2/3), then B (2/3→1)
// Result: black → deep red → orange → yellow → white
fn afm_hot(t: f32) -> [u8; 3] {
    let t = t * 0.8 + 0.2; // shift and scale to make the colors more vibrant
    let r = (sigmoid(t, 7.0, 0.6) * 1.05).clamp(0.0, 1.0);
    let g = (sigmoid(t, 10.0, 0.7) * 1.1 - 0.05).clamp(0.0, 1.0);
    let b = (sigmoid(t, 11.0, 0.7) * 1.6 - 0.55).clamp(0.0, 1.0);
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

// Viridis colormap sampled at 8 control points
fn viridis(t: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 8] = [
        [0.267, 0.004, 0.329],
        [0.283, 0.141, 0.458],
        [0.254, 0.265, 0.530],
        [0.207, 0.372, 0.553],
        [0.164, 0.471, 0.558],
        [0.128, 0.566, 0.551],
        [0.285, 0.706, 0.429],
        [0.993, 0.906, 0.144],
    ];
    let scaled = t * (STOPS.len() - 1) as f32;
    let lo = (scaled.floor() as usize).min(STOPS.len() - 2);
    let hi = lo + 1;
    let frac = scaled - lo as f32;
    let r = STOPS[lo][0] + frac * (STOPS[hi][0] - STOPS[lo][0]);
    let g = STOPS[lo][1] + frac * (STOPS[hi][1] - STOPS[lo][1]);
    let b = STOPS[lo][2] + frac * (STOPS[hi][2] - STOPS[lo][2]);
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}
