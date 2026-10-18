#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod app;
mod engine;
mod fmt;
mod listing;
mod selection;
mod text;

use eframe::egui;

fn main() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([760.0, 460.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("Copiar de OneDrive", options, Box::new(|cc| Ok(Box::new(app::App::new(cc.egui_ctx.clone())))))
}
