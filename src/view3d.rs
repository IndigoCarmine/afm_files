//! 3D height-map surface view: an orbital camera plus a glow (OpenGL) renderer
//! that draws the AFM height field as a lit, colormap-shaded triangle mesh.
//!
//! The camera follows the design used in `moleucle_3dview_rs`: orientation is a
//! quaternion, the eye orbits a `center` at a fixed `radius`, the view matrix is
//! built directly from the inverse rotation and an eye translation, panning
//! moves the center along the camera-local axes scaled by `radius`, and dollying
//! adjusts `radius`. The projection is the OpenGL right-handed perspective
//! (clip-space z in [-1, 1]) to match the glow backend.

use crate::colormap::Colormap;
use crate::parser::SpmImage;
use egui::Vec2;
use glam::{Mat4, Quat, Vec3};
use glow::HasContext as _;

const ORBIT_SPEED: f32 = 0.01;
const PAN_SPEED: f32 = 0.0018;
const ZOOM_SPEED: f32 = 0.0015;
const MIN_RADIUS: f32 = 0.1;
const MAX_RADIUS: f32 = 50.0;

/// Orbital (turntable) camera. `rotation` is the single source of truth for
/// orientation; the eye position is derived from `center`, `rotation`, `radius`.
pub struct OrbitalCamera {
    pub center: Vec3,
    pub rotation: Quat,
    pub radius: f32,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for OrbitalCamera {
    fn default() -> Self {
        Self {
            center: Vec3::ZERO,
            // A pleasant 3/4 bird's-eye angle: yaw ~30°, pitch down ~30°.
            rotation: Quat::from_axis_angle(Vec3::X, -30f32.to_radians())
                * Quat::from_axis_angle(Vec3::Y, -30f32.to_radians()),
            radius: 3.5,
            fov_y: 45f32.to_radians(),
            near: 0.01,
            far: 100.0,
        }
    }
}

impl OrbitalCamera {
    /// World-space camera-local axes derived from the current rotation.
    fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }
    fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }
    fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    /// Eye position: `center - rotated_forward * radius`.
    pub fn eye(&self) -> Vec3 {
        self.center - self.forward() * self.radius
    }

    /// World→view matrix, built as `inverse(rotation) * translate(-eye)` (no
    /// `look_at`, so it stays well-defined even looking straight down).
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::from_quat(self.rotation.inverse()) * Mat4::from_translation(-self.eye())
    }

    pub fn proj_matrix(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh_gl(self.fov_y, aspect.max(0.01), self.near, self.far)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj_matrix(aspect) * self.view_matrix()
    }

    /// Orbit around `center`: yaw about world up, pitch about the camera's right
    /// axis, accumulated into the rotation quaternion.
    pub fn orbit(&mut self, delta: Vec2) {
        let yaw = Quat::from_axis_angle(Vec3::Y, -delta.x * ORBIT_SPEED);
        let pitch = Quat::from_axis_angle(self.right(), -delta.y * ORBIT_SPEED);
        self.rotation = (yaw * pitch * self.rotation).normalize();
    }

    /// Pan by sliding `center` along the camera-local right/up axes, scaled by
    /// `radius` so the drag feels consistent at any zoom level.
    pub fn pan(&mut self, delta: Vec2) {
        let scale = self.radius * PAN_SPEED;
        self.center += self.right() * (-delta.x * scale) + self.up() * (delta.y * scale);
    }

    /// Dolly in/out by scaling `radius` (positive `scroll` zooms in).
    pub fn dolly(&mut self, scroll: f32) {
        let factor = (1.0 - scroll * ZOOM_SPEED).clamp(0.2, 5.0);
        self.radius = (self.radius * factor).clamp(MIN_RADIUS, MAX_RADIUS);
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
}

const VERTEX_SHADER: &str = r#"
#version 330 core
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec3 a_color;
uniform mat4 u_mvp;
out vec3 v_normal;
out vec3 v_color;
void main() {
    gl_Position = u_mvp * vec4(a_pos, 1.0);
    v_normal = a_normal;
    v_color = a_color;
}
"#;

const FRAGMENT_SHADER: &str = r#"
#version 330 core
in vec3 v_normal;
in vec3 v_color;
out vec4 frag_color;
uniform vec3 u_light_dir;
void main() {
    vec3 n = normalize(v_normal);
    // Two-sided lighting so back faces of the surface are never black.
    float diff = abs(dot(n, normalize(u_light_dir)));
    float intensity = 0.35 + 0.65 * diff;
    frag_color = vec4(v_color * intensity, 1.0);
}
"#;

/// Holds the GL program and mesh buffers. Mesh data is staged in `pending` from
/// the UI thread and uploaded inside the paint callback, where the GL context is
/// guaranteed current.
pub struct SurfaceRenderer {
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    index_count: i32,
    pending: Option<(Vec<Vertex>, Vec<u32>)>,
    u_mvp: Option<glow::UniformLocation>,
    u_light_dir: Option<glow::UniformLocation>,
}

impl SurfaceRenderer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let program = link_program(gl, VERTEX_SHADER, FRAGMENT_SHADER)?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ebo = gl.create_buffer()?;

            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));

            let stride = std::mem::size_of::<Vertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 3, glow::FLOAT, false, stride, 24);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);

            let u_mvp = gl.get_uniform_location(program, "u_mvp");
            let u_light_dir = gl.get_uniform_location(program, "u_light_dir");

            Ok(Self {
                program,
                vao,
                vbo,
                ebo,
                index_count: 0,
                pending: None,
                u_mvp,
                u_light_dir,
            })
        }
    }

    /// Stage a new mesh built from the current image/colormap/exaggeration.
    /// The actual GL upload happens later in `paint`.
    pub fn set_mesh(
        &mut self,
        img: &SpmImage,
        cmap: Colormap,
        z_color_min: f32,
        z_color_max: f32,
        z_exaggeration: f32,
    ) {
        self.pending = Some(build_mesh(img, cmap, z_color_min, z_color_max, z_exaggeration));
    }

    pub fn paint(&mut self, gl: &glow::Context, mvp: &Mat4, light_dir: Vec3, viewport: [i32; 4]) {
        unsafe {
            if let Some((verts, indices)) = self.pending.take() {
                self.upload(gl, &verts, &indices);
            }
            if self.index_count == 0 {
                return;
            }

            gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            // Two-sided lighting + no culling: the whole surface renders
            // regardless of triangle winding.
            gl.disable(glow::CULL_FACE);
            gl.clear_depth_f32(1.0);
            // Scissor (set by egui to this widget's rect) confines the clear.
            gl.clear(glow::DEPTH_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(self.u_mvp.as_ref(), false, &mvp.to_cols_array());
            gl.uniform_3_f32(
                self.u_light_dir.as_ref(),
                light_dir.x,
                light_dir.y,
                light_dir.z,
            );
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements(glow::TRIANGLES, self.index_count, glow::UNSIGNED_INT, 0);

            // Restore state egui relies on.
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.disable(glow::DEPTH_TEST);
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context, verts: &[Vertex], indices: &[u32]) {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
        gl.buffer_data_u8_slice(
            glow::ARRAY_BUFFER,
            bytemuck::cast_slice(verts),
            glow::DYNAMIC_DRAW,
        );
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
        gl.buffer_data_u8_slice(
            glow::ELEMENT_ARRAY_BUFFER,
            bytemuck::cast_slice(indices),
            glow::DYNAMIC_DRAW,
        );
        self.index_count = indices.len() as i32;
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ebo);
        }
    }
}

unsafe fn link_program(
    gl: &glow::Context,
    vertex_src: &str,
    fragment_src: &str,
) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let shader_sources = [
        (glow::VERTEX_SHADER, vertex_src),
        (glow::FRAGMENT_SHADER, fragment_src),
    ];
    let mut shaders = Vec::with_capacity(shader_sources.len());
    for (kind, src) in shader_sources {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, src);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            gl.delete_program(program);
            return Err(format!("Shader compile error: {log}"));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        return Err(format!("Program link error: {log}"));
    }
    // Shaders can be detached/deleted once linked.
    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }
    Ok(program)
}

/// Build a triangle-mesh surface from the height field.
///
/// The scan plane is mapped to X/Z in [-1, 1]; height maps to Y. Geometry height
/// uses the full data range (scaled by `z_exaggeration`, since real AFM data is
/// near-flat relative to its lateral extent), while vertex colors use the same
/// `[z_color_min, z_color_max]` window as the 2D view.
fn build_mesh(
    img: &SpmImage,
    cmap: Colormap,
    z_color_min: f32,
    z_color_max: f32,
    z_exaggeration: f32,
) -> (Vec<Vertex>, Vec<u32>) {
    let w = img.samps_per_line;
    let h = img.number_of_lines;
    if w < 2 || h < 2 || img.data.len() < w * h {
        return (Vec::new(), Vec::new());
    }

    const SPAN: f32 = 2.0;
    let dx = SPAN / (w - 1) as f32;
    let dz = SPAN / (h - 1) as f32;

    let geo_min = img.data.iter().cloned().fold(f32::INFINITY, f32::min);
    let geo_max = img.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let geo_range = (geo_max - geo_min).max(f32::EPSILON);
    let color_range = (z_color_max - z_color_min).max(f32::EPSILON);

    // Precompute the world-space height (Y) of every grid point for normals.
    let height = |row: usize, col: usize| -> f32 {
        let t = (img.data[row * w + col] - geo_min) / geo_range;
        (t - 0.5) * z_exaggeration
    };

    let mut verts = Vec::with_capacity(w * h);
    for row in 0..h {
        for col in 0..w {
            let x = (col as f32 / (w - 1) as f32 - 0.5) * SPAN;
            let z = (row as f32 / (h - 1) as f32 - 0.5) * SPAN;
            let y = height(row, col);

            // Central differences (clamped at edges) → surface normal for y=f(x,z).
            let hl = height(row, col.saturating_sub(1));
            let hr = height(row, (col + 1).min(w - 1));
            let hd = height(row.saturating_sub(1), col);
            let hu = height((row + 1).min(h - 1), col);
            let dydx = (hr - hl) / (2.0 * dx);
            let dydz = (hu - hd) / (2.0 * dz);
            let normal = Vec3::new(-dydx, 1.0, -dydz).normalize_or_zero();

            let t_color = (img.data[row * w + col] - z_color_min) / color_range;
            let color = cmap.map_rgb_f32(t_color);

            verts.push(Vertex {
                pos: [x, y, z],
                normal: normal.into(),
                color,
            });
        }
    }

    let mut indices = Vec::with_capacity((w - 1) * (h - 1) * 6);
    for row in 0..h - 1 {
        for col in 0..w - 1 {
            let i00 = (row * w + col) as u32;
            let i10 = (row * w + col + 1) as u32;
            let i01 = ((row + 1) * w + col) as u32;
            let i11 = ((row + 1) * w + col + 1) as u32;
            indices.extend_from_slice(&[i00, i10, i11, i00, i11, i01]);
        }
    }

    (verts, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn dummy_image(w: usize, h: usize) -> SpmImage {
        SpmImage {
            data: (0..w * h).map(|i| i as f32).collect(),
            metadata: HashMap::new(),
            scan_size_nm: 1000.0,
            samps_per_line: w,
            number_of_lines: h,
            channel_name: "Height".into(),
            channel_idx: 0,
            available_channels: Vec::new(),
        }
    }

    #[test]
    fn mesh_has_expected_counts_and_finite_values() {
        let img = dummy_image(4, 3);
        let (verts, indices) = build_mesh(&img, Colormap::AfmHot, 0.0, 11.0, 1.0);
        assert_eq!(verts.len(), 4 * 3);
        assert_eq!(indices.len(), (4 - 1) * (3 - 1) * 6);
        assert!(indices.iter().all(|&i| (i as usize) < verts.len()));
        for v in &verts {
            assert!(v.pos.iter().all(|c| c.is_finite()));
            assert!(v.normal.iter().all(|c| c.is_finite()));
            assert!(v.color.iter().all(|&c| (0.0..=1.0).contains(&c)));
        }
    }

    #[test]
    fn mesh_is_empty_for_degenerate_size() {
        let img = dummy_image(1, 5);
        let (verts, indices) = build_mesh(&img, Colormap::Gray, 0.0, 1.0, 1.0);
        assert!(verts.is_empty());
        assert!(indices.is_empty());
    }

    #[test]
    fn camera_eye_sits_at_radius_and_dolly_zooms() {
        let mut cam = OrbitalCamera::default();
        let r = cam.radius;
        assert!(((cam.eye() - cam.center).length() - r).abs() < 1e-4);
        cam.dolly(100.0); // positive scroll → zoom in
        assert!(cam.radius < r);
        let m = cam.view_proj(1.5);
        assert!(m.to_cols_array().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn camera_orbit_changes_orientation() {
        let mut cam = OrbitalCamera::default();
        let before = cam.rotation;
        cam.orbit(egui::Vec2::new(50.0, 0.0));
        assert!(cam.rotation.angle_between(before) > 1e-3);
    }
}
