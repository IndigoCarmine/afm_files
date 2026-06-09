mod analysis;
mod app;
mod colormap;
mod parser;

fn setup_fonts(ctx: &egui::Context) {
    let font_path = r"C:\Windows\Fonts\NotoSansJP-VF.ttf";
    if let Ok(bytes) = std::fs::read(font_path) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "NotoSansJP".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(1, "NotoSansJP".to_owned());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .push("NotoSansJP".to_owned());
        ctx.set_fonts(fonts);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kintuba AFM Viewer")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Kintuba AFM Viewer",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::AfmViewerApp::default()))
        }),
    )
}
