use crate::analysis::{export_afm_image, export_csv, export_profile_png, line_profile, nice_scale};
use crate::colormap::{to_color_image, to_rgba_bytes, Colormap};
use crate::parser::{load_spm, ChannelInfo, SpmImage};
use crate::view3d::{OrbitalCamera, SurfaceRenderer};
use egui::{TextureHandle, TextureOptions, Vec2};
use egui_plot::{Line, Plot, PlotPoints, Points, VLine};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(PartialEq)]
enum Tab {
    View,
    HeightAnalysis,
    Analysis,
    View3D,
}

#[derive(PartialEq, Clone)]
enum ProfileFilter {
    None,
    Gaussian,
    Custom,
}

pub struct AfmViewerApp {
    // File browser
    folder: Option<PathBuf>,
    files: Vec<PathBuf>,
    selected: Option<usize>,

    // Loaded image state
    image: Option<SpmImage>,
    texture: Option<TextureHandle>,
    load_error: Option<String>,

    // Channel selection
    channel_idx: usize,
    available_channels: Vec<ChannelInfo>,

    // Display controls
    colormap: Colormap,
    flatten_order: Option<u32>,
    smooth_sigma: f32,
    rolling_ball: bool,
    rolling_ball_radius: u32,
    z_min: f32,
    z_max: f32,
    z_data_min: f32,
    z_data_max: f32,

    // Export options
    show_scale_bar: bool,

    // Tab
    tab: Tab,

    // Pan / zoom (shared between View and Analysis)
    zoom: f32,
    pan: Vec2,

    // Analysis state – fractional image coords [0,1]x[0,1]
    line_p0: Option<egui::Pos2>,
    line_p1: Option<egui::Pos2>,
    drag_target: Option<u8>, // 0=p0, 1=p1
    profile: Vec<(f32, f32)>,
    status_msg: String,

    // Profile plot markers (distance in nm)
    plot_marker_a: Option<f64>,
    plot_marker_b: Option<f64>,

    // Image filter (Analysis tab)
    profile_filter: ProfileFilter,
    filter_sigma: f32,
    custom_kernel_str: String,
    custom_kernel: Vec<f32>,
    analysis_texture: Option<TextureHandle>,
    analysis_texture_dirty: bool,

    // 3D view
    gl: Option<Arc<glow::Context>>,
    camera: OrbitalCamera,
    renderer: Option<Arc<Mutex<SurfaceRenderer>>>,
    z_exaggeration: f32,
    image_gen: u64,
    // Cheap signature of the inputs the mesh depends on; when it changes the
    // mesh is rebuilt. (z values stored as bits to keep the key Copy + Eq.)
    mesh_key: Option<(u64, Colormap, u32, u32, u32)>,
    // 3D image export: a requested save path is consumed by the paint callback
    // (where the GL context is current); the outcome is reported back here.
    export_request: Option<PathBuf>,
    export_status: Arc<Mutex<Option<String>>>,
}

impl Default for AfmViewerApp {
    fn default() -> Self {
        Self {
            folder: None,
            files: vec![],
            selected: None,
            image: None,
            texture: None,
            load_error: None,
            channel_idx: 0,
            available_channels: vec![],
            colormap: Colormap::AfmHot,
            flatten_order: Some(2),
            smooth_sigma: 0.0,
            rolling_ball: false,
            rolling_ball_radius: 30,
            z_min: 0.0,
            z_max: 1.0,
            z_data_min: 0.0,
            z_data_max: 1.0,
            show_scale_bar: true,
            tab: Tab::View,
            zoom: 1.0,
            pan: Vec2::ZERO,
            line_p0: None,
            line_p1: None,
            drag_target: None,
            profile: vec![],
            status_msg: String::new(),
            plot_marker_a: None,
            plot_marker_b: None,
            profile_filter: ProfileFilter::None,
            filter_sigma: 1.0,
            custom_kernel_str: "1,2,1".to_string(),
            custom_kernel: vec![1.0, 2.0, 1.0],
            analysis_texture: None,
            analysis_texture_dirty: false,
            gl: None,
            camera: OrbitalCamera::default(),
            renderer: None,
            z_exaggeration: 0.6,
            image_gen: 0,
            mesh_key: None,
            export_request: None,
            export_status: Arc::new(Mutex::new(None)),
        }
    }
}

impl AfmViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            // Available when eframe runs on the glow backend (the default).
            gl: cc.gl.clone(),
            ..Self::default()
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Map a fractional image coordinate to screen space given the image rect and zoom/pan.
fn frac_to_screen(frac: egui::Pos2, rect: egui::Rect, zoom: f32, pan: Vec2) -> egui::Pos2 {
    let origin = image_origin(rect, zoom, pan);
    let size = rect.size() * zoom;
    egui::pos2(origin.x + frac.x * size.x, origin.y + frac.y * size.y)
}

/// Map a screen position back to fractional image coordinates.
fn screen_to_frac(pos: egui::Pos2, rect: egui::Rect, zoom: f32, pan: Vec2) -> egui::Pos2 {
    let origin = image_origin(rect, zoom, pan);
    let size = rect.size() * zoom;
    egui::pos2((pos.x - origin.x) / size.x, (pos.y - origin.y) / size.y)
}

/// Top-left corner of the (possibly zoomed + panned) image inside `rect`.
fn image_origin(rect: egui::Rect, zoom: f32, pan: Vec2) -> egui::Pos2 {
    let cx = rect.center().x + pan.x - rect.width() * zoom * 0.5;
    let cy = rect.center().y + pan.y - rect.height() * zoom * 0.5;
    egui::pos2(cx, cy)
}

fn is_spm_file(path: &std::path::Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    match ext.as_deref() {
        Some("spm") => true,
        Some(e) => e.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

fn gaussian_filter_2d(data: &[f32], width: usize, height: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 || data.is_empty() {
        return data.to_vec();
    }
    let radius = (3.0 * sigma).ceil() as usize;

    // horizontal pass
    let mut h = vec![0.0f32; data.len()];
    for row in 0..height {
        for col in 0..width {
            let mut sum = 0.0f32;
            let mut ws = 0.0f32;
            for c in col.saturating_sub(radius)..(col + radius + 1).min(width) {
                let d = (col as f32 - c as f32) / sigma;
                let w = (-0.5 * d * d).exp();
                sum += data[row * width + c] * w;
                ws += w;
            }
            h[row * width + col] = sum / ws;
        }
    }
    // vertical pass
    let mut out = vec![0.0f32; data.len()];
    for col in 0..width {
        for row in 0..height {
            let mut sum = 0.0f32;
            let mut ws = 0.0f32;
            for r in row.saturating_sub(radius)..(row + radius + 1).min(height) {
                let d = (row as f32 - r as f32) / sigma;
                let w = (-0.5 * d * d).exp();
                sum += h[r * width + col] * w;
                ws += w;
            }
            out[row * width + col] = sum / ws;
        }
    }
    out
}

fn custom_kernel_filter_2d(data: &[f32], width: usize, height: usize, kernel: &[f32]) -> Vec<f32> {
    if kernel.is_empty() || data.is_empty() {
        return data.to_vec();
    }
    let half = kernel.len() / 2;

    let conv1d = |src: &[f32], n: usize, stride: usize, out: &mut [f32]| {
        for i in 0..n {
            let mut sum = 0.0f32;
            let mut ws = 0.0f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let s = i as isize + ki as isize - half as isize;
                if s >= 0 && s < n as isize {
                    sum += src[s as usize * stride] * kv;
                    ws += kv;
                }
            }
            out[i * stride] = if ws.abs() > f32::EPSILON { sum / ws } else { src[i * stride] };
        }
    };

    // horizontal pass (stride = 1, n = width, repeat for each row)
    let mut h = data.to_vec();
    for row in 0..height {
        let base = row * width;
        let src: Vec<f32> = (0..width).map(|c| data[base + c]).collect();
        conv1d(&src, width, 1, &mut h[base..]);
    }
    // vertical pass (stride = width, n = height, repeat for each col)
    let mut out = h.clone();
    for col in 0..width {
        let src: Vec<f32> = (0..height).map(|r| h[r * width + col]).collect();
        let mut tmp = vec![0.0f32; height];
        conv1d(&src, height, 1, &mut tmp);
        for r in 0..height {
            out[r * width + col] = tmp[r];
        }
    }
    out
}

// ── impl ──────────────────────────────────────────────────────────────────────

impl AfmViewerApp {
    fn rebuild_analysis_texture(&mut self, ctx: &egui::Context) {
        if let Some(ref img) = self.image {
            let filtered = match self.profile_filter {
                ProfileFilter::None => img.data.clone(),
                ProfileFilter::Gaussian => {
                    gaussian_filter_2d(&img.data, img.samps_per_line, img.number_of_lines, self.filter_sigma)
                }
                ProfileFilter::Custom => {
                    custom_kernel_filter_2d(&img.data, img.samps_per_line, img.number_of_lines, &self.custom_kernel)
                }
            };
            let ci = to_color_image(
                &filtered,
                img.number_of_lines,
                img.samps_per_line,
                self.colormap,
                self.z_min,
                self.z_max,
            );
            self.analysis_texture = Some(ctx.load_texture("analysis_image", ci, TextureOptions::NEAREST));
        }
        self.analysis_texture_dirty = false;
    }

    fn load_file(&mut self, ctx: &egui::Context, idx: usize) {
        self.load_file_channel(ctx, idx, None);
    }

    fn load_file_channel(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        channel_override: Option<Option<usize>>,
    ) {
        let path = self.files[idx].clone();
        let is_new_file = self.selected != Some(idx);
        self.selected = Some(idx);
        self.load_error = None;
        self.line_p0 = None;
        self.line_p1 = None;
        self.drag_target = None;
        self.profile = vec![];
        self.plot_marker_a = None;
        self.plot_marker_b = None;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.analysis_texture = None;
        self.analysis_texture_dirty = true;

        // For new files, auto-detect channel; for reloads use current channel
        let channel = if let Some(ch) = channel_override {
            ch
        } else if is_new_file {
            None // auto-detect
        } else {
            Some(self.channel_idx)
        };

        match load_spm(&path, self.flatten_order, self.smooth_sigma, channel) {
            Ok(mut img) => {
                if self.rolling_ball {
                    img.data = crate::analysis::rolling_ball_subtract(
                        &img.data,
                        img.samps_per_line,
                        img.number_of_lines,
                        self.rolling_ball_radius as usize,
                    );
                }
                let z_min = img.data.iter().cloned().fold(f32::INFINITY, f32::min);
                let z_max = img.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                self.z_data_min = z_min;
                self.z_data_max = z_max;
                self.z_min = z_min;
                self.z_max = z_max;
                if is_new_file {
                    self.channel_idx = img.channel_idx;
                }
                self.available_channels = img.available_channels.clone();
                self.rebuild_texture(ctx, &img);
                self.image = Some(img);
                self.image_gen += 1;
            }
            Err(e) => {
                self.load_error = Some(e);
                self.image = None;
                self.texture = None;
            }
        }
    }

    fn rebuild_texture(&mut self, ctx: &egui::Context, img: &SpmImage) {
        let ci = to_color_image(
            &img.data,
            img.number_of_lines,
            img.samps_per_line,
            self.colormap,
            self.z_min,
            self.z_max,
        );
        self.texture = Some(ctx.load_texture("afm_image", ci, TextureOptions::NEAREST));
    }

    fn scan_folder(&mut self, folder: PathBuf) {
        self.files = std::fs::read_dir(&folder)
            .map(|rd| {
                let mut paths: Vec<PathBuf> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && is_spm_file(p))
                    .collect();
                paths.sort();
                paths
            })
            .unwrap_or_default();
        self.folder = Some(folder);
        self.selected = None;
        self.image = None;
        self.texture = None;
        self.load_error = None;

        crate::parser::prefetch_cloud_files(self.files.clone());
    }

    fn show_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if ui.button("📂 Open Folder").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.scan_folder(folder);
                }
            }

            ui.separator();

            ui.label("Colormap:");
            let prev_cmap = self.colormap;
            egui::ComboBox::from_id_salt("cmap")
                .selected_text(self.colormap.name())
                .show_ui(ui, |ui| {
                    for &cmap in Colormap::ALL {
                        ui.selectable_value(&mut self.colormap, cmap, cmap.name());
                    }
                });
            if self.colormap != prev_cmap {
                if let Some(img) = &self.image {
                    let ci = to_color_image(
                        &img.data,
                        img.number_of_lines,
                        img.samps_per_line,
                        self.colormap,
                        self.z_min,
                        self.z_max,
                    );
                    self.texture = Some(ctx.load_texture("afm_image", ci, TextureOptions::NEAREST));
                }
            }

            ui.separator();

            ui.label("Flatten:");
            let orders: &[Option<u32>] = &[None, Some(0), Some(1), Some(2), Some(3)];
            let order_label = |o: Option<u32>| match o {
                None => "None",
                Some(0) => "0",
                Some(1) => "1",
                Some(2) => "2",
                Some(3) => "3",
                _ => "?",
            };
            let prev_order = self.flatten_order;
            egui::ComboBox::from_id_salt("flatten")
                .selected_text(order_label(self.flatten_order))
                .show_ui(ui, |ui| {
                    for &o in orders {
                        ui.selectable_value(&mut self.flatten_order, o, order_label(o));
                    }
                });
            if self.flatten_order != prev_order {
                if let Some(idx) = self.selected {
                    self.load_file(ctx, idx);
                }
            }

            ui.separator();

            ui.label("Smooth σ:");
            let prev_sigma = self.smooth_sigma;
            ui.add(
                egui::DragValue::new(&mut self.smooth_sigma)
                    .speed(0.1)
                    .range(0.0..=10.0)
                    .suffix(" px"),
            );
            if (self.smooth_sigma - prev_sigma).abs() > f32::EPSILON {
                if let Some(idx) = self.selected {
                    self.load_file(ctx, idx);
                }
            }

            ui.separator();

            let prev_rb = self.rolling_ball;
            let prev_radius = self.rolling_ball_radius;
            ui.checkbox(&mut self.rolling_ball, "Rolling ball");
            ui.add_enabled(
                self.rolling_ball,
                egui::DragValue::new(&mut self.rolling_ball_radius)
                    .speed(1.0)
                    .range(1..=300)
                    .suffix(" px"),
            );
            if self.rolling_ball != prev_rb || self.rolling_ball_radius != prev_radius {
                if let Some(idx) = self.selected {
                    self.load_file(ctx, idx);
                }
            }

            ui.separator();

            if ui.button("⟳ Reset View").clicked() {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
        });
    }

    fn show_file_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.files.is_empty() {
                ui.label("No SPM files found.");
                return;
            }
            let mut to_load: Option<usize> = None;
            for (i, path) in self.files.iter().enumerate() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                let selected = self.selected == Some(i);
                if ui.selectable_label(selected, name.as_ref()).clicked() && !selected {
                    to_load = Some(i);
                }
            }
            if let Some(idx) = to_load {
                self.load_file(ctx, idx);
            }
        });
    }

    // ── View tab ──────────────────────────────────────────────────────────────

    fn show_view_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(ref err) = self.load_error.clone() {
            ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
            return;
        }
        if self.texture.is_none() {
            ui.label("Select a file to view.");
            return;
        }

        // Z range — DragValue allows values outside the data min/max
        let mut z_changed = false;
        ui.horizontal(|ui| {
            ui.label("Z min:");
            let prev = self.z_min;
            ui.add(
                egui::DragValue::new(&mut self.z_min)
                    .speed(0.1)
                    .suffix(" nm"),
            );
            if (self.z_min - prev).abs() > f32::EPSILON {
                z_changed = true;
            }
            ui.label("Z max:");
            let prev = self.z_max;
            ui.add(
                egui::DragValue::new(&mut self.z_max)
                    .speed(0.1)
                    .suffix(" nm"),
            );
            if (self.z_max - prev).abs() > f32::EPSILON {
                z_changed = true;
            }
            if ui.button("データ範囲").clicked() {
                self.z_min = self.z_data_min;
                self.z_max = self.z_data_max;
                z_changed = true;
            }
        });
        if z_changed {
            if let Some(img) = &self.image {
                let ci = to_color_image(
                    &img.data,
                    img.number_of_lines,
                    img.samps_per_line,
                    self.colormap,
                    self.z_min,
                    self.z_max,
                );
                self.texture = Some(ctx.load_texture("afm_image", ci, TextureOptions::NEAREST));
            }
        }

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_scale_bar, "Scale bar");
            if ui.button("💾 Save Image").clicked() {
                if let Some(ref img) = self.image {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("PNG", &["png"])
                        .save_file()
                    {
                        match export_afm_image(
                            &img.data,
                            img.number_of_lines,
                            img.samps_per_line,
                            self.colormap,
                            self.z_min,
                            self.z_max,
                            img.scan_size_nm,
                            self.show_scale_bar,
                            &path,
                        ) {
                            Ok(_) => self.status_msg = "Image saved.".to_string(),
                            Err(e) => self.status_msg = format!("Save error: {e}"),
                        }
                    }
                }
            }
            if ui.button("📋 Copy to Clipboard").clicked() {
                if let Some(ref img) = self.image {
                    let rgba = to_rgba_bytes(
                        &img.data,
                        img.number_of_lines,
                        img.samps_per_line,
                        self.colormap,
                        self.z_min,
                        self.z_max,
                    );
                    match arboard::Clipboard::new() {
                        Ok(mut cb) => {
                            let img_data = arboard::ImageData {
                                width: img.samps_per_line,
                                height: img.number_of_lines,
                                bytes: std::borrow::Cow::Owned(rgba),
                            };
                            match cb.set_image(img_data) {
                                Ok(_) => self.status_msg = "Copied to clipboard.".to_string(),
                                Err(e) => self.status_msg = format!("Clipboard error: {e}"),
                            }
                        }
                        Err(e) => self.status_msg = format!("Clipboard error: {e}"),
                    }
                }
            }
            if !self.status_msg.is_empty() {
                ui.label(&self.status_msg.clone());
            }
        });

        let avail = ui.available_size();
        let base_side = avail.x.min(avail.y) - 20.0;

        let (response, painter) = ui.allocate_painter(
            Vec2::new(base_side, base_side),
            egui::Sense::click_and_drag(),
        );
        let rect = response.rect;

        // Pan via drag
        if response.dragged() {
            self.pan += response.drag_delta();
        }

        // Zoom via scroll
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            let factor = (1.0 + scroll * 0.001).clamp(0.8, 1.25);
            if let Some(cursor) = ctx.input(|i| i.pointer.hover_pos()) {
                // Zoom toward cursor
                let before = cursor - rect.center().to_vec2() - self.pan;
                self.zoom = (self.zoom * factor).clamp(0.1, 50.0);
                self.pan = cursor - rect.center().to_vec2() - before * factor;
            } else {
                self.zoom = (self.zoom * factor).clamp(0.1, 50.0);
            }
        }

        // Draw image
        if let Some(ref tex) = self.texture {
            let origin = image_origin(rect, self.zoom, self.pan);
            let size = rect.size() * self.zoom;
            let img_rect = egui::Rect::from_min_size(origin, size);
            painter.image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // Border around image area
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, egui::Color32::BLACK), egui::StrokeKind::Inside);

        // Scale bar
        if let Some(ref img) = self.image {
            let bar_nm = nice_scale(img.scan_size_nm / 5.0);
            let bar_frac = bar_nm / img.scan_size_nm;
            let bar_px = bar_frac * rect.width() * self.zoom;
            let bar_y = rect.max.y - 12.0;
            let bar_x0 = rect.min.x + 10.0;
            painter.line_segment(
                [
                    egui::pos2(bar_x0, bar_y),
                    egui::pos2(bar_x0 + bar_px, bar_y),
                ],
                egui::Stroke::new(3.0, egui::Color32::WHITE),
            );
            painter.text(
                egui::pos2(bar_x0 + bar_px + 4.0, bar_y),
                egui::Align2::LEFT_CENTER,
                format!("{bar_nm:.0} nm"),
                egui::FontId::proportional(12.0),
                egui::Color32::WHITE,
            );
        }
    }

    // ── Height Analysis tab ───────────────────────────────────────────────────

    fn show_analysis_tab(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        if self.texture.is_none() || self.image.is_none() {
            ui.label("Select a file first.");
            return;
        }

        ui.horizontal(|ui| {
            ui.label("Click: set start/end. Drag handles to adjust.");
            if ui.button("Clear Line").clicked() {
                self.line_p0 = None;
                self.line_p1 = None;
                self.drag_target = None;
                self.profile = vec![];
                self.status_msg.clear();
            }
            if !self.status_msg.is_empty() {
                ui.label(&self.status_msg.clone());
            }
        });

        let avail = ui.available_size();
        let image_side = (avail.x * 0.5).min(avail.y - 60.0);
        let ctx = ui.ctx().clone();

        ui.horizontal(|ui| {
            // ── image panel ──────────────────────────────────────────────────
            let (response, painter) =
                ui.allocate_painter(Vec2::splat(image_side), egui::Sense::click_and_drag());
            let rect = response.rect;

            // Pan
            if response.dragged() && self.drag_target.is_none() {
                self.pan += response.drag_delta();
            }

            // Zoom
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 && response.hovered() {
                let factor = (1.0 + scroll * 0.001).clamp(0.8, 1.25);
                if let Some(cursor) = ctx.input(|i| i.pointer.hover_pos()) {
                    let before = cursor - rect.center().to_vec2() - self.pan;
                    self.zoom = (self.zoom * factor).clamp(0.1, 50.0);
                    self.pan = cursor - rect.center().to_vec2() - before * factor;
                } else {
                    self.zoom = (self.zoom * factor).clamp(0.1, 50.0);
                }
            }

            // Draw image
            if let Some(ref tex) = self.texture {
                let origin = image_origin(rect, self.zoom, self.pan);
                let size = rect.size() * self.zoom;
                let img_rect = egui::Rect::from_min_size(origin, size);
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            // Border around image area
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, egui::Color32::from_gray(100)), egui::StrokeKind::Outside);

            // ── handle drag start ────────────────────────────────────────────
            const HANDLE_R: f32 = 7.0;
            const GRAB_R: f32 = 14.0;

            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let mut target: Option<u8> = None;
                    if let Some(p0) = self.line_p0 {
                        let sp0 = frac_to_screen(p0, rect, self.zoom, self.pan);
                        if (pos - sp0).length() < GRAB_R {
                            target = Some(0);
                        }
                    }
                    if target.is_none() {
                        if let Some(p1) = self.line_p1 {
                            let sp1 = frac_to_screen(p1, rect, self.zoom, self.pan);
                            if (pos - sp1).length() < GRAB_R {
                                target = Some(1);
                            }
                        }
                    }
                    self.drag_target = target;
                }
            }

            // ── handle drag ──────────────────────────────────────────────────
            if response.dragged() {
                if let Some(target) = self.drag_target {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let frac = screen_to_frac(pos, rect, self.zoom, self.pan);
                        let frac = egui::pos2(frac.x.clamp(0.0, 1.0), frac.y.clamp(0.0, 1.0));
                        match target {
                            0 => self.line_p0 = Some(frac),
                            _ => self.line_p1 = Some(frac),
                        }
                        // Recompute profile on every drag step
                        if let (Some(p0), Some(p1), Some(ref img)) =
                            (self.line_p0, self.line_p1, &self.image)
                        {
                            self.profile = line_profile(img, (p0.x, p0.y), (p1.x, p1.y));
                        }
                    }
                }
            }

            if response.drag_stopped() {
                self.drag_target = None;
            }

            // ── click to set points (only when not dragging a handle) ────────
            if response.clicked() && self.drag_target.is_none() {
                if let Some(pos) = response.interact_pointer_pos() {
                    // Ignore click if it landed on an existing handle
                    let on_p0 = self
                        .line_p0
                        .map(|p| {
                            (pos - frac_to_screen(p, rect, self.zoom, self.pan)).length() < GRAB_R
                        })
                        .unwrap_or(false);
                    let on_p1 = self
                        .line_p1
                        .map(|p| {
                            (pos - frac_to_screen(p, rect, self.zoom, self.pan)).length() < GRAB_R
                        })
                        .unwrap_or(false);

                    if !on_p0 && !on_p1 {
                        let frac = screen_to_frac(pos, rect, self.zoom, self.pan);
                        let frac = egui::pos2(frac.x.clamp(0.0, 1.0), frac.y.clamp(0.0, 1.0));
                        if self.line_p0.is_none() || self.line_p1.is_some() {
                            self.line_p0 = Some(frac);
                            self.line_p1 = None;
                            self.profile = vec![];
                            self.status_msg = "Start set. Click for end.".to_string();
                        } else {
                            self.line_p1 = Some(frac);
                            if let (Some(p0), Some(ref img)) = (self.line_p0, &self.image) {
                                self.profile = line_profile(img, (p0.x, p0.y), (frac.x, frac.y));
                            }
                            self.status_msg = format!("{} profile points", self.profile.len());
                        }
                    }
                }
            }

            // ── cursor hint near handles ─────────────────────────────────────
            if let Some(cursor) = ctx.input(|i| i.pointer.hover_pos()) {
                let near_handle = [self.line_p0, self.line_p1].iter().any(|opt| {
                    opt.map(|p| {
                        (cursor - frac_to_screen(p, rect, self.zoom, self.pan)).length() < GRAB_R
                    })
                    .unwrap_or(false)
                });
                if near_handle && rect.contains(cursor) {
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                }
            }

            // ── draw overlay ─────────────────────────────────────────────────
            if let Some(p0) = self.line_p0 {
                let sp0 = frac_to_screen(p0, rect, self.zoom, self.pan);
                if let Some(p1) = self.line_p1 {
                    let sp1 = frac_to_screen(p1, rect, self.zoom, self.pan);
                    painter.line_segment([sp0, sp1], egui::Stroke::new(2.0, egui::Color32::WHITE));
                    // p1 = red
                    painter.circle_filled(sp1, HANDLE_R, egui::Color32::RED);
                    painter.circle_stroke(
                        sp1,
                        HANDLE_R,
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                    );
                }
                // p0 = green
                painter.circle_filled(sp0, HANDLE_R, egui::Color32::GREEN);
                painter.circle_stroke(sp0, HANDLE_R, egui::Stroke::new(1.5, egui::Color32::WHITE));
            }

            // ── cross-section plot ───────────────────────────────────────────
            ui.vertical(|ui| {
                if !self.profile.is_empty() {
                    let profile = self.profile.clone();

                    // Helper: interpolate height at a given x (nm) from profile
                    let interp_height = |x_nm: f64| -> f64 {
                        let x = x_nm as f32;
                        if let Some(i) = profile.iter().position(|&(d, _)| d >= x) {
                            if i == 0 {
                                return profile[0].1 as f64;
                            }
                            let (d0, h0) = profile[i - 1];
                            let (d1, h1) = profile[i];
                            let t = if (d1 - d0).abs() < f32::EPSILON {
                                0.0_f32
                            } else {
                                (x - d0) / (d1 - d0)
                            };
                            (h0 + t * (h1 - h0)) as f64
                        } else {
                            profile.last().map(|&(_, h)| h as f64).unwrap_or(0.0)
                        }
                    };

                    let ma = self.plot_marker_a;
                    let mb = self.plot_marker_b;
                    let ma_h = ma.map(|x| interp_height(x));
                    let mb_h = mb.map(|x| interp_height(x));

                    let curve: PlotPoints =
                        profile.iter().map(|&(d, h)| [d as f64, h as f64]).collect();

                    let plot_resp = Plot::new("profile_plot")
                        .width(avail.x * 0.45)
                        .height(image_side - 80.0)
                        .x_axis_label("Distance (nm)")
                        .y_axis_label("Height (nm)")
                        .allow_zoom(false)
                        .allow_scroll(false)
                        .allow_drag(false)
                        .show(ui, |plot_ui| {
                            plot_ui.line(Line::new("高さ", curve));

                            if let Some(xa) = ma {
                                plot_ui.vline(
                                    VLine::new("A", xa)
                                        .color(egui::Color32::from_rgb(50, 130, 255))
                                        .width(1.5),
                                );
                            }
                            if let Some(xb) = mb {
                                plot_ui.vline(
                                    VLine::new("B", xb)
                                        .color(egui::Color32::from_rgb(220, 60, 60))
                                        .width(1.5),
                                );
                            }

                            if let (Some(xa), Some(ha)) = (ma, ma_h) {
                                plot_ui.points(
                                    Points::new("A", vec![[xa, ha]])
                                        .radius(5.0)
                                        .color(egui::Color32::from_rgb(50, 130, 255)),
                                );
                            }
                            if let (Some(xb), Some(hb)) = (mb, mb_h) {
                                plot_ui.points(
                                    Points::new("B", vec![[xb, hb]])
                                        .radius(5.0)
                                        .color(egui::Color32::from_rgb(220, 60, 60)),
                                );
                            }

                            if let (Some(xa), Some(ha), Some(xb), Some(hb)) = (ma, ma_h, mb, mb_h) {
                                plot_ui.line(
                                    Line::new("A-B", vec![[xa, ha], [xb, hb]])
                                        .color(egui::Color32::from_rgba_premultiplied(
                                            200, 200, 200, 160,
                                        ))
                                        .width(1.0),
                                );
                            }

                            plot_ui.pointer_coordinate()
                        });

                    // Left-click → set A then B alternately
                    if plot_resp.response.clicked() {
                        if let Some(coord) = plot_resp.inner {
                            let x = coord.x;
                            if self.plot_marker_a.is_none() || self.plot_marker_b.is_some() {
                                self.plot_marker_a = Some(x);
                                self.plot_marker_b = None;
                            } else {
                                self.plot_marker_b = Some(x);
                            }
                        }
                    }
                    // Right-click → clear
                    if plot_resp.response.secondary_clicked() {
                        self.plot_marker_a = None;
                        self.plot_marker_b = None;
                    }

                    // ── Difference readout ────────────────────────────────────
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| match (ma, ma_h, mb, mb_h) {
                            (Some(xa), Some(ha), Some(xb), Some(hb)) => {
                                let dx = xb - xa;
                                let dh = hb - ha;
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(80, 160, 255),
                                        format!("A  {xa:.2} nm,  {ha:.3} nm"),
                                    );
                                    ui.label("→");
                                    ui.colored_label(
                                        egui::Color32::from_rgb(240, 80, 80),
                                        format!("B  {xb:.2} nm,  {hb:.3} nm"),
                                    );
                                });
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("ΔD = {dx:+.2} nm"))
                                            .size(14.0)
                                            .strong(),
                                    );
                                    ui.add_space(16.0);
                                    ui.label(
                                        egui::RichText::new(format!("ΔH = {dh:+.3} nm"))
                                            .size(14.0)
                                            .strong(),
                                    );
                                });
                            }
                            (Some(xa), Some(ha), None, _) => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(80, 160, 255),
                                    format!("A  {xa:.2} nm,  {ha:.3} nm"),
                                );
                                ui.label("← クリックで B を設定");
                            }
                            _ => {
                                ui.label("プロット上をクリック → A、次のクリック → B");
                                ui.label("右クリックでクリア");
                            }
                        });

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save CSV").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("CSV", &["csv"])
                                .save_file()
                            {
                                match export_csv(&self.profile, &path) {
                                    Ok(_) => self.status_msg = "CSV saved.".to_string(),
                                    Err(e) => self.status_msg = format!("CSV error: {e}"),
                                }
                            }
                        }
                        if ui.button("Save PNG").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("PNG", &["png"])
                                .save_file()
                            {
                                match export_profile_png(&self.profile, &path) {
                                    Ok(_) => self.status_msg = "PNG saved.".to_string(),
                                    Err(e) => self.status_msg = format!("PNG error: {e}"),
                                }
                            }
                        }
                        if ui.button("マーカークリア").clicked() {
                            self.plot_marker_a = None;
                            self.plot_marker_b = None;
                        }
                    });
                } else {
                    ui.label("(プロフィールなし)");
                }
            });
        });
    }

    // ── Analysis (Image Filter) tab ───────────────────────────────────────────

    fn show_filter_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.image.is_none() {
            ui.label("Select a file first.");
            return;
        }

        // ── filter controls ──────────────────────────────────────────────────
        let mut dirty = self.analysis_texture_dirty;

        ui.horizontal(|ui| {
            ui.label("Filter:");
            let prev = self.profile_filter.clone();
            egui::ComboBox::from_id_salt("img_filter")
                .selected_text(match self.profile_filter {
                    ProfileFilter::None => "None",
                    ProfileFilter::Gaussian => "Gaussian",
                    ProfileFilter::Custom => "Custom",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.profile_filter, ProfileFilter::None, "None");
                    ui.selectable_value(&mut self.profile_filter, ProfileFilter::Gaussian, "Gaussian");
                    ui.selectable_value(&mut self.profile_filter, ProfileFilter::Custom, "Custom");
                });
            if self.profile_filter != prev {
                dirty = true;
            }

            match self.profile_filter {
                ProfileFilter::Gaussian => {
                    ui.label("σ:");
                    let prev_sigma = self.filter_sigma;
                    ui.add(
                        egui::DragValue::new(&mut self.filter_sigma)
                            .speed(0.1)
                            .range(0.1..=50.0)
                            .suffix(" px"),
                    );
                    if (self.filter_sigma - prev_sigma).abs() > f32::EPSILON {
                        dirty = true;
                    }
                }
                ProfileFilter::Custom => {
                    ui.label("Kernel (separable 1D):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.custom_kernel_str).desired_width(160.0),
                    );
                    if ui.button("Apply").clicked() {
                        self.custom_kernel = self.custom_kernel_str
                            .split(',')
                            .filter_map(|s| s.trim().parse::<f32>().ok())
                            .collect();
                        dirty = true;
                    }
                }
                ProfileFilter::None => {}
            }

            ui.checkbox(&mut self.show_scale_bar, "Scale bar");
            if ui.button("💾 Save Image").clicked() {
                if let (Some(ref img), Some(ref tex)) = (&self.image, &self.analysis_texture) {
                    let _ = tex; // texture already built; re-derive filtered data for export
                    let filtered = match self.profile_filter {
                        ProfileFilter::None => img.data.clone(),
                        ProfileFilter::Gaussian => gaussian_filter_2d(
                            &img.data, img.samps_per_line, img.number_of_lines, self.filter_sigma,
                        ),
                        ProfileFilter::Custom => custom_kernel_filter_2d(
                            &img.data, img.samps_per_line, img.number_of_lines, &self.custom_kernel,
                        ),
                    };
                    if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).save_file() {
                        match export_afm_image(
                            &filtered,
                            img.number_of_lines,
                            img.samps_per_line,
                            self.colormap,
                            self.z_min,
                            self.z_max,
                            img.scan_size_nm,
                            self.show_scale_bar,
                            &path,
                        ) {
                            Ok(_) => self.status_msg = "Image saved.".to_string(),
                            Err(e) => self.status_msg = format!("Save error: {e}"),
                        }
                    }
                }
            }

            if !self.status_msg.is_empty() {
                ui.label(&self.status_msg.clone());
            }
        });

        // rebuild texture when dirty
        if dirty {
            self.rebuild_analysis_texture(ctx);
        }

        ui.separator();

        // ── filtered image display ────────────────────────────────────────────
        let avail = ui.available_size();
        let side = avail.x.min(avail.y) - 20.0;

        let (response, painter) =
            ui.allocate_painter(Vec2::new(side, side), egui::Sense::click_and_drag());
        let rect = response.rect;

        if response.dragged() {
            self.pan += response.drag_delta();
        }
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            let factor = (1.0 + scroll * 0.001).clamp(0.8, 1.25);
            if let Some(cursor) = ctx.input(|i| i.pointer.hover_pos()) {
                let before = cursor - rect.center().to_vec2() - self.pan;
                self.zoom = (self.zoom * factor).clamp(0.1, 50.0);
                self.pan = cursor - rect.center().to_vec2() - before * factor;
            } else {
                self.zoom = (self.zoom * factor).clamp(0.1, 50.0);
            }
        }

        if let Some(ref tex) = self.analysis_texture {
            let origin = image_origin(rect, self.zoom, self.pan);
            let size = rect.size() * self.zoom;
            painter.image(
                tex.id(),
                egui::Rect::from_min_size(origin, size),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        painter.rect_stroke(rect, 0.0, egui::Stroke::new(5.0, egui::Color32::BLACK), egui::StrokeKind::Inside);

        if let Some(ref img) = self.image {
            let bar_nm = nice_scale(img.scan_size_nm / 5.0);
            let bar_px = (bar_nm / img.scan_size_nm) * rect.width() * self.zoom;
            let bar_y = rect.max.y - 12.0;
            let bar_x0 = rect.min.x + 10.0;
            painter.line_segment(
                [egui::pos2(bar_x0, bar_y), egui::pos2(bar_x0 + bar_px, bar_y)],
                egui::Stroke::new(3.0, egui::Color32::WHITE),
            );
            painter.text(
                egui::pos2(bar_x0 + bar_px + 4.0, bar_y),
                egui::Align2::LEFT_CENTER,
                format!("{bar_nm:.0} nm"),
                egui::FontId::proportional(12.0),
                egui::Color32::WHITE,
            );
        }
    }

    fn show_view3d_tab(&mut self, ui: &mut egui::Ui) {
        // Surface the result of a previous frame's export: the save runs in the
        // paint callback, so its outcome is reported back a frame later.
        if let Some(msg) = self.export_status.lock().unwrap().take() {
            self.status_msg = msg;
        }

        ui.horizontal(|ui| {
            ui.label("Z exaggeration:");
            ui.add(egui::Slider::new(&mut self.z_exaggeration, 0.05..=3.0).logarithmic(true));
            if ui.button("Reset view").clicked() {
                self.camera = OrbitalCamera::default();
            }
            if ui.button("💾 Save Image").clicked() && self.image.is_some() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PNG", &["png"])
                    .save_file()
                {
                    self.export_request = Some(path);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Drag: orbit · Right-drag / Shift-drag: pan · Scroll: zoom");
            if !self.status_msg.is_empty() {
                ui.separator();
                ui.label(&self.status_msg);
            }
        });
        ui.separator();

        let Some(gl) = self.gl.clone() else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "3D view requires the glow rendering backend.",
            );
            return;
        };
        if self.image.is_none() {
            ui.label("Select a file to view its surface in 3D.");
            return;
        }

        // Lazily create the GL renderer the first time the tab is shown.
        if self.renderer.is_none() {
            match SurfaceRenderer::new(&gl) {
                Ok(r) => self.renderer = Some(Arc::new(Mutex::new(r))),
                Err(e) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Renderer init failed: {e}"));
                    return;
                }
            }
        }
        let renderer = self.renderer.clone().unwrap();

        // Rebuild the mesh only when an input it depends on has changed.
        let key = (
            self.image_gen,
            self.colormap,
            self.z_min.to_bits(),
            self.z_max.to_bits(),
            self.z_exaggeration.to_bits(),
        );
        if self.mesh_key != Some(key) {
            if let Some(ref img) = self.image {
                renderer.lock().unwrap().set_mesh(
                    img,
                    self.colormap,
                    self.z_min,
                    self.z_max,
                    self.z_exaggeration,
                );
                self.mesh_key = Some(key);
            }
        }

        // Camera interaction over the drawing area.
        let size = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::drag());
        if response.dragged() {
            let delta = response.drag_delta();
            let pan = response.dragged_by(egui::PointerButton::Secondary)
                || ui.input(|i| i.modifiers.shift);
            if pan {
                self.camera.pan(delta);
            } else {
                self.camera.orbit(delta);
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.dolly(scroll);
            }
        }

        // Resolve the camera to an MVP matrix and hand rendering to glow.
        let aspect = rect.width() / rect.height().max(1.0);
        let mvp = self.camera.view_proj(aspect);
        let light_dir = glam::Vec3::new(0.4, 1.0, 0.6);

        // A pending save is rendered off-screen inside the callback (where the
        // GL context is current). Export at ~1200 px tall, preserving the
        // on-screen aspect so the saved framing matches what's displayed.
        let export = self.export_request.take();
        let export_status = self.export_status.clone();
        let export_ctx = ui.ctx().clone();
        let export_h = 1200i32;
        let export_w = ((export_h as f32 * aspect).round() as i32).max(16);

        let cb = egui_glow::CallbackFn::new(move |info, painter| {
            let gl = painter.gl();
            let mut r = renderer.lock().unwrap();
            if let Some(path) = export.as_ref() {
                let msg = match r.render_to_image(gl, &mvp, light_dir, export_w, export_h) {
                    Ok(img) => match img.save(path) {
                        Ok(()) => format!("3D image saved: {}", path.display()),
                        Err(e) => format!("Save error: {e}"),
                    },
                    Err(e) => format!("Export error: {e}"),
                };
                *export_status.lock().unwrap() = Some(msg);
                export_ctx.request_repaint();
            }
            let vp = info.viewport_in_pixels();
            let viewport = [
                vp.left_px as i32,
                vp.from_bottom_px as i32,
                vp.width_px as i32,
                vp.height_px as i32,
            ];
            r.paint(gl, &mvp, light_dir, viewport);
        });
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(cb),
        });
    }
}

impl eframe::App for AfmViewerApp {
    // Required by eframe 0.34 — our actual logic lives in `update`.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            self.show_toolbar(ui, ctx);
        });

        egui::SidePanel::left("file_panel")
            .min_width(200.0)
            .show(ctx, |ui| {
                ui.heading("Files");
                if let Some(ref folder) = self.folder.clone() {
                    ui.small(folder.to_string_lossy().as_ref());
                }
                ui.separator();
                self.show_file_list(ui, ctx);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(self.tab == Tab::View, "View").clicked() {
                    self.tab = Tab::View;
                }
                if ui
                    .selectable_label(self.tab == Tab::HeightAnalysis, "Height Analysis")
                    .clicked()
                {
                    self.tab = Tab::HeightAnalysis;
                }
                if ui
                    .selectable_label(self.tab == Tab::Analysis, "Analysis")
                    .clicked()
                {
                    self.tab = Tab::Analysis;
                }
                if ui.selectable_label(self.tab == Tab::View3D, "3D").clicked() {
                    self.tab = Tab::View3D;
                }
            });
            ui.separator();

            match self.tab {
                Tab::View => self.show_view_tab(ui, ctx),
                Tab::HeightAnalysis => self.show_analysis_tab(ui, ctx),
                Tab::Analysis => self.show_filter_tab(ui, ctx),
                Tab::View3D => self.show_view3d_tab(ui),
            }
        });
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        if let (Some(gl), Some(renderer)) = (gl, &self.renderer) {
            renderer.lock().unwrap().destroy(gl);
        }
    }
}
