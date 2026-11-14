//! PTZ Programmer desktop app.

// Link commands for media, jog and thumbnails are wired up by the tabs that use them.
#![allow(dead_code)]

mod app;
mod frames;
mod graph;
mod link;
mod lock_ui;
mod media_ui;
mod monitor;
mod panels;
mod rig_ui;
mod session;
mod settings_ui;
mod theme;
mod timeline;
mod timeline_ui;
#[cfg(test)]
mod ui_tests;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PTZ Programmer")
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native("PTZ Programmer", options, Box::new(|cc| Ok(Box::new(app::PtzApp::new(cc)))))
}
