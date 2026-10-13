#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod compressor;
mod importer;
mod integration;
mod metadata;
mod model;
mod settings;
mod theme;
mod ui;

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1
        && (args[0] == "--install-context-menu" || args[0] == "--remove-context-menu")
    {
        let result = if args[0] == "--install-context-menu" {
            integration::install()
        } else {
            integration::uninstall()
        };
        match result {
            Ok(message) => println!("{message}"),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let paths = args
        .into_iter()
        .filter(|arg| arg != "--")
        .map(std::path::PathBuf::from)
        .collect();
    let icon = image::load_from_memory(include_bytes!("../packaging/assets/marzianetto.png"))
        .expect("embedded marzianetto icon must be a valid PNG")
        .to_rgba8();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("marzianetto")
            .with_icon(eframe::egui::IconData {
                rgba: icon.as_raw().clone(),
                width: icon.width(),
                height: icon.height(),
            })
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([1040.0, 680.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "marzianetto",
        options,
        Box::new(move |cc| Ok(Box::new(ui::CompressorApp::new(cc, paths)))),
    )
}
