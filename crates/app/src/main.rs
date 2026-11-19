//! Motion Choreographer: the desktop app (`ptz-app`).

mod app;
mod brand;
mod camera_ui;
mod frames;
#[cfg(all(test, target_os = "linux"))]
mod gpu_tests;
mod graph;
mod link;
mod live_edit;
mod lock_ui;
mod media_ui;
mod monitor;
mod multiview;
mod panels;
mod placement_ui;
mod rig_ui;
mod session;
mod setlist_ui;
mod settings_ui;
mod stage;
mod stage_gpu;
mod stage_ui;
mod theme;
mod timeline;
mod timeline_clipboard;
mod timeline_ui;
#[cfg(test)]
mod ui_tests;
mod units;
mod update;

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(brand::NAME)
        .with_inner_size([1500.0, 920.0])
        .with_min_inner_size([1100.0, 700.0]);
    if let Some(icon) = brand::window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(brand::NAME, options, Box::new(|cc| Ok(Box::new(app::PtzApp::new(cc)))))
}
