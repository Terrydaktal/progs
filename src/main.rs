mod app;
mod dependency_graph;
mod models;
mod scanner;
mod search;

use app::ProgramManagerApp;
use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1420.0, 820.0])
            .with_title("progs - System Programs Manager"),
        ..Default::default()
    };
    eframe::run_native(
        "progs",
        options,
        Box::new(|cc| Ok(Box::new(ProgramManagerApp::new(cc)))),
    )
}
