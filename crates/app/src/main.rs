//! PTZ Programmer desktop app.

mod app;
mod camera_ui;
mod frames;
mod graph;
mod link;
mod lock_ui;
mod media_ui;
mod monitor;
mod multiview;
mod panels;
mod rig_ui;
mod session;
mod settings_ui;
mod stage;
mod stage_ui;
mod theme;
mod timeline;
mod timeline_ui;
#[cfg(test)]
mod ui_tests;
mod units;

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
