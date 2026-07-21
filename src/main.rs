mod app;
mod chrome_bridge;
mod config;
mod pty;

use app::LumiTermApp;
use config::AppConfig;
use eframe::egui;

fn main() -> eframe::Result {
    let config = match AppConfig::load_or_create() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("failed to load your Lumi-Term config: {error}");
            AppConfig::default()
        }
    };

    let app_title = config.window.title.clone();
    let app_title_for_error = app_title.clone();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.window.width, config.window.height])
            .with_title(app_title.clone())
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        &app_title,
        native_options,
        Box::new(move |_cc| match LumiTermApp::new(config.clone()) {
            Ok(app) => Ok(Box::new(app)),
            Err(error) => Ok(Box::new(LumiTermApp::error(
                app_title_for_error.clone(),
                error.to_string(),
            ))),
        }),
    )
}
