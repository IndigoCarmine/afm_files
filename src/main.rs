#![windows_subsystem = "windows"]
mod analysis;
mod app;
mod colormap;
mod name_filter;
mod parser;
mod view3d;

fn setup_fonts(ctx: &egui::Context) {
    #[cfg(target_os = "windows")]
    let font_path = r"C:\Windows\Fonts\NotoSansJP-VF.ttf";
    #[cfg(target_os = "macos")]
    let font_path = "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let font_path = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc";

    if let Ok(bytes) = std::fs::read(font_path) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "JapaneseFont".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(1, "JapaneseFont".to_owned());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .push("JapaneseFont".to_owned());
        ctx.set_fonts(fonts);
    }
}

fn app_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/AppIcon.iconset/icon_256x256.png");
    let image = image::load_from_memory(bytes).expect("valid icon PNG");
    let image = image.into_rgba8();
    let (w, h) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width: w,
        height: h,
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        // The 3D surface tab needs a depth buffer for the glow renderer.
        depth_buffer: 24,
        viewport: egui::ViewportBuilder::default()
            .with_title("Kintuba AFM Viewer")
            .with_inner_size([1200.0, 800.0])
            .with_icon(std::sync::Arc::new(app_icon())),
        ..Default::default()
    };
    eframe::run_native(
        "Kintuba AFM Viewer",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::AfmViewerApp::new(cc)))
        }),
    )
}
