//! Motion Choreographer desktop app.

mod app;
mod brand;
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
mod setlist_ui;
mod settings_ui;
mod stage;
mod stage_ui;
mod theme;
mod timeline;
mod timeline_ui;
#[cfg(test)]
mod ui_tests;
mod units;
mod update;

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title(brand::NAME)
        .with_inner_size([1500.0, 920.0])
        .with_min_inner_size([1100.0, 700.0]);
    // On a Mac the icon is the app bundle's, on the rounded tile that macOS expects; a window icon would replace it.
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_icon(brand::window_icon());
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(brand::NAME, options, Box::new(|cc| Ok(Box::new(app::PtzApp::new(cc)))))
}
