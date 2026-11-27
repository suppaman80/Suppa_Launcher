mod autostart;
mod catalog;

use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use catalog::{APPS, Available};
use eframe::egui::{self, Color32, RichText, Vec2};

const ACCENT: Color32 = Color32::from_rgb(199, 164, 255);
const PANEL: Color32 = Color32::from_rgb(29, 26, 39);
const MUTED: Color32 = Color32::from_rgb(171, 166, 185);
const INTERVAL: Duration = Duration::from_secs(3600);

#[derive(Default)]
struct Row {
    available: Option<Available>,
    checking: bool,
    progress: Option<f32>,
    message: String,
}

enum Event {
    Checked(usize, Result<Available, String>),
    Progress(usize, f32),
    Installed(usize, Result<(), String>),
    Autostart(Result<(), String>),
}

struct Launcher {
    root: PathBuf,
    rows: [Row; APPS.len()],
    icons: Vec<egui::TextureHandle>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    query: String,
    grid: bool,
    settings: bool,
    login_pending: bool,
    notice: String,
    last_check: Instant,
}

fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    if tx.send(event).is_ok() {
        ctx.request_repaint();
    }
}

impl Launcher {
    fn new(ctx: &egui::Context, root: PathBuf) -> Self {
        let mut theme = egui::Visuals::dark();
        theme.panel_fill = Color32::from_rgb(17, 16, 24);
        theme.selection.bg_fill = Color32::from_rgb(72, 51, 105);
        theme.hyperlink_color = ACCENT;
        ctx.set_visuals(theme);
        let icons = APPS
            .iter()
            .map(|app| {
                let png = image::load_from_memory(app.icon)
                    .expect("Icona inclusa")
                    .into_rgba8();
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [png.width() as usize, png.height() as usize],
                    png.as_raw(),
                );
                ctx.load_texture(app.repo, image, egui::TextureOptions::LINEAR)
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        let mut launcher = Self {
            root,
            rows: Default::default(),
            icons,
            tx,
            rx,
            query: String::new(),
            grid: false,
            settings: false,
            login_pending: false,
            notice: String::new(),
            last_check: Instant::now(),
        };
        launcher.check(ctx);
        launcher
    }

    fn check(&mut self, ctx: &egui::Context) {
        self.last_check = Instant::now();
        for (id, row) in self.rows.iter_mut().enumerate() {
            if row.checking || row.progress.is_some() {
                continue;
            }
            row.checking = true;
            row.message.clear();
            let (tx, ctx) = (self.tx.clone(), ctx.clone());
            thread::spawn(move || {
                let result = catalog::latest(&APPS[id]).map_err(|e| e.to_string());
                send(&tx, &ctx, Event::Checked(id, result));
            });
        }
    }

    fn install(&mut self, id: usize, ctx: &egui::Context) {
        let row = &mut self.rows[id];
        let Some(release) = row.available.clone() else {
            return;
        };
        row.progress = Some(0.0);
        row.message.clear();
        let (tx, ctx, root) = (self.tx.clone(), ctx.clone(), self.root.clone());
        thread::spawn(move || {
            let result = catalog::install(&APPS[id], &release, &root, |progress| {
                send(&tx, &ctx, Event::Progress(id, progress));
            })
            .map_err(|e| e.to_string());
            send(&tx, &ctx, Event::Installed(id, result));
        });
    }

    fn card(&mut self, id: usize, ui: &mut egui::Ui) {
        let app = &APPS[id];
        let installed = app.installed(&self.root);
        let row = &self.rows[id];
        
        // Extract values from row to avoid borrow issues in closure
        let available_version = row.available.as_ref().map(|r| r.version.clone());
        let checking = row.checking;
        let progress = row.progress;
        let message = row.message.clone();
        
        let update = available_version.as_ref().is_some_and(|release_version| {
            installed
                .as_ref()
                .is_some_and(|(version, _)| release_version > version)
        });
        let label = if update {
            "AGGIORNAMENTO"
        } else if installed.is_some() {
            "INSTALLATA"
        } else {
            "NON INSTALLATA"
        };
        egui::Frame::new()
            .fill(PANEL)
            .corner_radius(8)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.image((self.icons[id].id(), Vec2::splat(40.0)));
                    ui.vertical(|ui| {
                        ui.strong(app.title);
                        ui.label(RichText::new(app.description).color(MUTED));
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(label).monospace().small().color(ACCENT));
                    if let Some((version, _)) = &installed {
                        ui.label(format!("Installata: {version}"));
                    }
                    if let Some(release_version) = &available_version {
                        ui.label(format!("Su GitHub: {}", release_version));
                    }
                    if checking {
                        ui.spinner();
                    }
                });
                if let Some(progress) = progress {
                    ui.add(egui::ProgressBar::new(progress).show_percentage());
                }
                if !message.is_empty() {
                    ui.label(RichText::new(&message).color(MUTED));
                }
                let can_download = cfg!(target_arch = "x86_64")
                    && !checking
                    && progress.is_none()
                    && available_version.is_some()
                    && (installed.is_none() || update);
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            can_download,
                            egui::Button::new(if update { "Aggiorna" } else { "Scarica" }),
                        )
                        .clicked()
                    {
                        self.install(id, ui.ctx());
                    }
                    if let Some((_, path)) = installed {
                        if ui.button("Apri").clicked() {
                            match Command::new(path)
                                .env_remove("APPIMAGE")
                                .env_remove("APPDIR")
                                .env_remove("ARGV0")
                                .spawn()
                            {
                                Ok(mut child) => {
                                    thread::spawn(move || {
                                        let _ = child.wait();
                                    });
                                }
                                Err(error) => {
                                    self.rows[id].message = format!("Avvio non riuscito: {error}")
                                }
                            }
                        }
                    }
                    ui.hyperlink_to("Sito", app.site());
                });
            });
    }
}

impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Checked(id, result) => {
                    let row = &mut self.rows[id];
                    row.checking = false;
                    match result {
                        Ok(available) => row.available = Some(available),
                        Err(error) => row.message = format!("Verifica GitHub: {error}"),
                    }
                }
                Event::Progress(id, progress) => self.rows[id].progress = Some(progress),
                Event::Installed(id, result) => {
                    self.rows[id].progress = None;
                    self.rows[id].message = result
                        .err()
                        .unwrap_or_else(|| "AppImage verificata e pronta.".into());
                }
                Event::Autostart(result) => {
                    self.login_pending = false;
                    self.notice = result
                        .err()
                        .unwrap_or_else(|| "Avvio automatico aggiornato.".into());
                }
            }
        }
        if self.last_check.elapsed() >= INTERVAL {
            self.check(ctx);
        }
        ctx.request_repaint_after(INTERVAL.saturating_sub(self.last_check.elapsed()));

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(9.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("SUPPAWORLD").heading().strong().color(ACCENT));
                ui.label(RichText::new("LINUX APP SUITE").monospace().color(MUTED));
                if ui.button("Impostazioni").clicked() {
                    self.settings = true;
                }
            });
            ui.add_space(9.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "GITHUB  /  RELEASE STABILI  /  v{}",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .monospace()
                    .small()
                    .color(MUTED),
                );
                ui.hyperlink_to(
                    "Portale Suppaworld",
                    "https://suppaman80.github.io/Suppaworld/",
                );
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(7.0);
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Cerca app")
                        .desired_width(205.0),
                );
                if ui
                    .add_enabled(
                        !self.rows.iter().any(|row| row.checking),
                        egui::Button::new("Controlla aggiornamenti"),
                    )
                    .clicked()
                {
                    self.check(ctx);
                }
                ui.selectable_value(&mut self.grid, false, "Elenco");
                ui.selectable_value(&mut self.grid, true, "Griglia");
            });
            ui.add_space(10.0);
            let query = self.query.trim().to_lowercase();
            let visible = APPS
                .iter()
                .enumerate()
                .filter_map(|(id, app)| {
                    (app.title.to_lowercase().contains(&query)
                        || app.description.to_lowercase().contains(&query))
                    .then_some(id)
                })
                .collect::<Vec<_>>();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if visible.is_empty() {
                    ui.label("Nessuna app corrispondente.");
                }
                if self.grid && ui.available_width() >= 650.0 {
                    for ids in visible.chunks(2) {
                        ui.columns(2, |columns| {
                            for (&id, column) in ids.iter().zip(columns) {
                                self.card(id, column);
                            }
                        });
                        ui.add_space(8.0);
                    }
                } else {
                    for id in visible {
                        self.card(id, ui);
                        ui.add_space(8.0);
                    }
                }
            });
        });
        egui::Window::new("Impostazioni")
            .open(&mut self.settings)
            .show(ctx, |ui| {
                ui.label("Controllo GitHub all'apertura e ogni ora mentre il launcher e' aperto.");
                ui.label("Le AppImage vengono installate o aggiornate solo quando lo chiedi.");
                ui.label(format!("Cartella app: {}", self.root.display()));
                let mut enabled = autostart::entry().is_ok_and(|path| path.is_file());
                if ui
                    .add_enabled(
                        !self.login_pending,
                        egui::Checkbox::new(&mut enabled, "Avvia Suppaworld all'accesso"),
                    )
                    .changed()
                {
                    self.login_pending = true;
                    let (tx, ctx) = (self.tx.clone(), ctx.clone());
                    thread::spawn(move || {
                        let result = autostart::set(enabled).map_err(|e| e.to_string());
                        send(&tx, &ctx, Event::Autostart(result));
                    });
                }
                ui.label("L'attivazione conserva una copia del launcher nella cartella dati.");
                if !self.notice.is_empty() {
                    ui.label(&self.notice);
                }
            });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = catalog::xdg("XDG_DATA_HOME", ".local/share")?.join("suppaworld/apps");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 570.0])
            .with_min_inner_size([470.0, 390.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Suppaworld Launcher",
        options,
        Box::new(move |cc| Ok(Box::new(Launcher::new(&cc.egui_ctx, root)))),
    )?;
    Ok(())
}