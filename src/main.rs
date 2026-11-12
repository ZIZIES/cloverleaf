// cloverleaf: a native gui for opencode.
//
//   main.rs    window setup + the eframe glue
//   server.rs  spawns `opencode serve` and kills it on exit
//   api.rs     http client + the server-sent-events reader thread
//   model.rs   structs parsed out of the api's json
//   app.rs     state, event handling, actions
//   ui.rs      drawing
//   settings.rs  the settings window (keys, mcp, plugins, ...)
//   custom.rs  custom openai-compatible providers, kept in a config file of our own
//   ollama.rs  its "local models" tab: download + manage ollama models, what fits in vram
//   theme.rs   palette + fonts,  logo.rs  the clover

mod api;
mod app;
mod custom;
mod logo;
mod model;
mod ollama;
mod server;
mod settings;
mod theme;
mod ui;

struct Shell(app::App);

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.draw(ui);
        screenshot(&mut self.0, ui.ctx());
    }

    // the window itself is cleared to nothing; the panels paint the (maybe translucent) background.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }
}

// debug hook: CLOVERLEAF_SHOT=/path/out.ppm dumps one frame after CLOVERLEAF_SHOT_AFTER seconds, then quits.
fn screenshot(app: &mut app::App, ctx: &egui::Context) {
    let Some((path, after)) = app.shot.clone() else { return };
    ctx.request_repaint_after_secs(0.1);
    if !app.shot_sent && app.now() > after {
        app.shot_sent = true;
        let ctx2 = ctx.clone();
        let cb = egui::ScreenshotCallback::new(move |img: std::sync::Arc<egui::ColorImage>| {
            let mut out = format!("P6\n{} {}\n255\n", img.width(), img.height()).into_bytes();
            for px in &img.pixels {
                out.extend_from_slice(&[px.r(), px.g(), px.b()]);
            }
            let _ = std::fs::write(&path, out);
            let px = img.pixels[img.width() * (img.height() - 40) + img.width() / 2];
            eprintln!("shot: background pixel alpha = {}", px.a());
            ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(cb));
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("cloverleaf")
            .with_app_id("cloverleaf")
            .with_transparent(true)
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([420.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native("cloverleaf", options, Box::new(|cc| Ok(Box::new(Shell(app::App::new(cc))))))
}
