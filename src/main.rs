mod autostart;
mod catalog;
mod settings;

use std::cmp::Ordering;
use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use catalog::{APPS, Available};
use eframe::egui::{
    self, Align, Color32, Layout, RichText, Stroke, Vec2,
};
use settings::{Settings, Theme};

const ACCENT: Color32 = Color32::from_rgb(199, 164, 255);
const PINK: Color32 = Color32::from_rgb(220, 78, 137);
const SUCCESS: Color32 = Color32::from_rgb(104, 214, 167);
const BLUE: Color32 = Color32::from_rgb(127, 181, 239);
const PANEL: Color32 = Color32::from_rgb(29, 26, 39);
const PANEL_ALT: Color32 = Color32::from_rgb(21, 20, 28);
const HEADER: Color32 = Color32::from_rgb(13, 12, 18);
const WHITE: Color32 = Color32::from_rgb(239, 237, 244);
const MUTED: Color32 = Color32::from_rgb(150, 145, 165);
const TEXT: Color32 = Color32::from_rgb(214, 210, 222);
const INTERVAL: Duration = Duration::from_secs(3600);
const AUTHOR: &str = "Suppaman80";

#[derive(Clone, Copy, PartialEq)]
enum View { Apps, Settings }

#[derive(Clone, Copy, PartialEq)]
enum ViewMode { Table, Grid }

#[derive(Clone, Copy, PartialEq)]
enum Sort { NameAsc, NameDesc, Newest }

#[derive(Clone, Copy, PartialEq)]
enum Status { Installed, Update, NotInstalled, Unknown }

#[derive(Default, Clone)]
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
    Saved(Result<(), String>),
}

enum Action { Install(usize), Open(usize), Recheck }

struct Launcher {
    settings: Settings,
    applied_theme: Theme,
    root: PathBuf,
    view: View,
    layout: ViewMode,
    sort: Sort,
    rows: [Row; APPS.len()],
    icons: Vec<egui::TextureHandle>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    query: String,
    login_pending: bool,
    notice: String,
    last_check: Instant,
    install_dir_text: String,
}

fn send(tx: &Sender<Event>, ctx: &egui::Context, event: Event) {
    if tx.send(event).is_ok() {
        ctx.request_repaint();
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let mut visuals = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::System | Theme::Dark => egui::Visuals::dark(),
    };
    visuals.panel_fill = Color32::from_rgb(17, 16, 24);
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.active.bg_fill = Color32::from_rgb(42, 38, 58);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(30, 27, 42);
    visuals.selection.bg_fill = Color32::from_rgb(72, 51, 105);
    visuals.hyperlink_color = ACCENT;
    ctx.set_visuals(visuals);
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { "just now".into() }
    else if s < 3600 { format!("{} min ago", s / 60) }
    else { format!("{} h ago", s / 3600) }
}

fn status_badge(ui: &mut egui::Ui, status: Status) {
    let (text, color) = match status {
        Status::Installed => ("INSTALLED", SUCCESS),
        Status::Update => ("UPDATE", PINK),
        Status::NotInstalled => ("NOT INSTALLED", MUTED),
        Status::Unknown => ("CHECKING", BLUE),
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.13))
        .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.45)))
        .corner_radius(4)
        .inner_margin(4)
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(9.0).strong().color(color));
        });
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 42, 58)))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(title).size(10.0).strong().color(MUTED));
            ui.add_space(6.0);
            body(ui);
        });
    ui.add_space(10.0);
}

impl Launcher {
    fn new(ctx: &egui::Context) -> Self {
        let settings = Settings::load();
        apply_theme(ctx, settings.theme);
        let icons = APPS.iter().map(|app| {
            let png = image::load_from_memory(app.icon).expect("Icona inclusa").into_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [png.width() as usize, png.height() as usize],
                png.as_raw(),
            );
            ctx.load_texture(app.repo, image, egui::TextureOptions::LINEAR)
        }).collect();
        let (tx, rx) = mpsc::channel();
        let mut l = Self {
            settings: settings.clone(),
            applied_theme: settings.theme,
            root: settings.install_root(),
            view: View::Apps,
            layout: ViewMode::Table,
            sort: Sort::NameAsc,
            rows: Default::default(),
            icons,
            tx,
            rx,
            query: String::new(),
            login_pending: false,
            notice: String::new(),
            last_check: Instant::now(),
            install_dir_text: settings.install_dir.clone().unwrap_or_default(),
        };
        l.check(ctx);
        l
    }

    fn check(&mut self, ctx: &egui::Context) {
        self.last_check = Instant::now();
        let include_pre = self.settings.include_prereleases;
        for (id, row) in self.rows.iter_mut().enumerate() {
            if row.checking || row.progress.is_some() { continue; }
            row.checking = true;
            row.message.clear();
            let (tx, ctx) = (self.tx.clone(), ctx.clone());
            thread::spawn(move || {
                let result = catalog::latest(&APPS[id], include_pre).map_err(|e| e.to_string());
                send(&tx, &ctx, Event::Checked(id, result));
            });
        }
    }

    fn install(&mut self, id: usize, ctx: &egui::Context) {
        let Some(release) = self.rows[id].available.clone() else { return; };
        self.rows[id].progress = Some(0.0);
        self.rows[id].message.clear();
        let (tx, ctx, root) = (self.tx.clone(), ctx.clone(), self.root.clone());
        thread::spawn(move || {
            let result = catalog::install(&APPS[id], &release, &root, |p| {
                send(&tx, &ctx, Event::Progress(id, p));
            }).map_err(|e| e.to_string());
            send(&tx, &ctx, Event::Installed(id, result));
        });
    }

    fn open(&mut self, id: usize) {
        if let Some((_, path)) = APPS[id].installed(&self.root) {
            match Command::new(path)
                .env_remove("APPIMAGE").env_remove("APPDIR").env_remove("ARGV0")
                .spawn()
            {
                Ok(mut child) => { thread::spawn(move || { let _ = child.wait(); }); }
                Err(error) => { self.rows[id].message = format!("Avvio non riuscito: {error}"); }
            }
        }
    }

    fn visible_ids(&self) -> Vec<usize> {
        let q = self.query.trim().to_lowercase();
        let mut ids: Vec<usize> = APPS.iter()
            .enumerate()
            .filter(|(_, a)| a.title.to_lowercase().contains(&q) || a.description.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        match self.sort {
            Sort::NameAsc => ids.sort_by(|&a, &b| APPS[a].title.to_lowercase().cmp(&APPS[b].title.to_lowercase())),
            Sort::NameDesc => ids.sort_by(|&a, &b| APPS[b].title.to_lowercase().cmp(&APPS[a].title.to_lowercase())),
            Sort::Newest => ids.sort_by(|&a, &b| {
                self.rows[b].available.as_ref().map(|x| x.version.clone())
                    .cmp(&self.rows[a].available.as_ref().map(|x| x.version.clone()))
            }),
        }
        ids
    }

    fn status_of(&self, id: usize) -> Status {
        let app = &APPS[id];
        let installed_version = app.installed(&self.root).map(|(v, _)| v);
        let available = self.rows[id].available.as_ref();
        let is_update = installed_version.as_ref().is_some_and(|iv| {
            available.map_or(false, |a| a.version.cmp(iv) == Ordering::Greater)
        });
        if self.rows[id].checking { Status::Unknown }
        else if is_update { Status::Update }
        else if installed_version.is_some() { Status::Installed }
        else { Status::NotInstalled }
    }

    fn table_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let visible = self.visible_ids();
        egui::Frame::new()
            .fill(PANEL)
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("apps_table")
                        .num_columns(6)
                        .spacing([16.0, 10.0])
                        .show(ui, |ui| {
                            for h in ["APP", "STATUS", "VERSION", "RELEASED", "AUTHOR", ""] {
                                ui.label(RichText::new(h).size(10.0).strong().color(MUTED));
                            }
                            ui.end_row();
                            for id in visible {
                                self.table_row(ui, id, actions);
                                ui.end_row();
                            }
                        });
                });
            });
    }

    fn table_row(&mut self, ui: &mut egui::Ui, id: usize, actions: &mut Vec<Action>) {
        let app = &APPS[id];
        let installed_version = app.installed(&self.root).map(|(v, _)| v);
        let (checking, progress, available_version, released) = {
            let row = &self.rows[id];
            (
                row.checking,
                row.progress,
                row.available.as_ref().map(|a| a.version.clone()),
                row.available.as_ref().map(|a| a.released.clone()).unwrap_or_else(|| "-".into()),
            )
        };
        let update = installed_version.as_ref().is_some_and(|iv| {
            available_version.as_ref().map_or(false, |av| av.cmp(iv) == Ordering::Greater)
        });
        let status = if checking { Status::Unknown }
            else if update { Status::Update }
            else if installed_version.is_some() { Status::Installed }
            else { Status::NotInstalled };

        ui.horizontal(|ui| {
            ui.image((self.icons[id].id(), Vec2::splat(30.0)));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(app.title).strong().size(14.0).color(TEXT));
                ui.label(RichText::new(app.description).size(10.0).color(MUTED));
            });
        });

        status_badge(ui, status);

        let version_text = match (&installed_version, &available_version) {
            (Some(iv), Some(av)) if av.cmp(iv) == Ordering::Greater => format!("{iv}  ->  {av}"),
            (Some(iv), _) => iv.to_string(),
            (None, Some(av)) => av.to_string(),
            (None, None) => "-".into(),
        };
        ui.label(RichText::new(version_text).monospace().size(12.0).color(TEXT));

        ui.label(RichText::new(released).size(11.0).color(MUTED));
        ui.label(RichText::new(AUTHOR).size(11.0).color(MUTED));

        if let Some(p) = progress {
            ui.add(egui::ProgressBar::new(p).desired_width(120.0).show_percentage());
        } else if checking {
            ui.add(egui::Spinner::new());
        } else if update {
            if ui.add_sized([120.0, 28.0], egui::Button::new(RichText::new("UPDATE").strong().color(WHITE))).clicked() {
                actions.push(Action::Install(id));
            }
        } else if installed_version.is_some() {
            if ui.add_sized([120.0, 28.0], egui::Button::new(RichText::new("OPEN").strong().color(WHITE))).clicked() {
                actions.push(Action::Open(id));
            }
        } else if available_version.is_some() {
            if ui.add_sized([120.0, 28.0], egui::Button::new(RichText::new("INSTALL").strong().color(WHITE))).clicked() {
                actions.push(Action::Install(id));
            }
        } else {
            ui.add_enabled(false, egui::Button::new("..."));
        }
    }

    fn grid_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let visible = self.visible_ids();
        let width = ui.available_width();
        let cols = ((width / 320.0).floor() as usize).clamp(1, 4);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for chunk in visible.chunks(cols) {
                ui.columns(cols, |columns| {
                    for (i, &id) in chunk.iter().enumerate() {
                        self.grid_card(&mut columns[i], id, actions);
                    }
                });
                ui.add_space(10.0);
            }
        });
    }

    fn grid_card(&mut self, ui: &mut egui::Ui, id: usize, actions: &mut Vec<Action>) {
        let app = &APPS[id];
        let installed_version = app.installed(&self.root).map(|(v, _)| v);
        let (checking, progress, available_version, released) = {
            let row = &self.rows[id];
            (
                row.checking,
                row.progress,
                row.available.as_ref().map(|a| a.version.clone()),
                row.available.as_ref().map(|a| a.released.clone()).unwrap_or_else(|| "-".into()),
            )
        };
        let update = installed_version.as_ref().is_some_and(|iv| {
            available_version.as_ref().map_or(false, |av| av.cmp(iv) == Ordering::Greater)
        });
        let status = if checking { Status::Unknown }
            else if update { Status::Update }
            else if installed_version.is_some() { Status::Installed }
            else { Status::NotInstalled };

        egui::Frame::new()
            .fill(PANEL_ALT)
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 42, 58)))
            .corner_radius(10)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_min_height(196.0);
                ui.horizontal(|ui| {
                    let vtxt = available_version.as_ref().map(|a| format!("v{a}")).unwrap_or_else(|| "v?".into());
                    ui.label(RichText::new(vtxt).monospace().size(10.0).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.hyperlink_to(RichText::new("GITHUB ->").size(9.0).strong().color(MUTED), app.github());
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.image((self.icons[id].id(), Vec2::splat(34.0)));
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(app.title).strong().size(15.0).color(TEXT));
                        ui.label(RichText::new(format!("by {}", app.author)).size(9.0).color(MUTED));
                    });
                });
                ui.add_space(8.0);
                ui.label(RichText::new(app.description).size(11.0).color(MUTED));
                ui.add_space(8.0);
                ui.label(RichText::new(format!("Last updated: {released}")).size(10.0).color(MUTED));
                ui.add_space(10.0);
                status_badge(ui, status);
                ui.add_space(10.0);
                let w = ui.available_width();
                let (label, enabled) = if update { ("UPDATE", !checking && progress.is_none()) }
                    else if installed_version.is_some() { ("OPEN", true) }
                    else if available_version.is_some() { ("INSTALL", !checking && progress.is_none()) }
                    else { ("...", false) };
                let btn = egui::Button::new(RichText::new(label).strong().color(WHITE));
                let clicked = if enabled {
                    ui.add_sized([w.max(60.0), 28.0], btn).clicked()
                } else {
                    ui.add_enabled(false, btn);
                    false
                };
                if clicked {
                    if update || available_version.is_some() {
                        actions.push(Action::Install(id));
                    } else if installed_version.is_some() {
                        actions.push(Action::Open(id));
                    }
                }
            });
    }
    fn settings_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.settings.install_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some()
            && self.install_dir_text != self.settings.install_dir.as_deref().unwrap_or("")
        {
            self.install_dir_text = self.settings.install_dir.as_deref().unwrap_or("").to_string();
        }
        let _ = self.settings.save();
        self.root = self.settings.install_root();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new("Settings").size(26.0).strong().color(ACCENT));
            ui.label(RichText::new(format!("Suppaworld Launcher v{}", env!("CARGO_PKG_VERSION"))).monospace().small().color(MUTED));
            ui.add_space(14.0);

            section(ui, "UPDATES", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Check for updates");
                    let mut auto = self.settings.auto_check;
                    egui::ComboBox::from_id_salt("updates_mode")
                        .selected_text(if auto { "Automatically" } else { "Manually" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut auto, true, "Automatically");
                            ui.selectable_value(&mut auto, false, "Manually");
                        });
                    self.settings.auto_check = auto;
                });
                let mut ai = self.settings.auto_install;
                if ui.checkbox(&mut ai, "Automatically download and install updates").changed() {
                    self.settings.auto_install = ai;
                    let _ = self.settings.save();
                }
                let mut nt = self.settings.notify_updates;
                if ui.checkbox(&mut nt, "Notify me when an installed app has an update").changed() {
                    self.settings.notify_updates = nt;
                }
                let mut pr = self.settings.include_prereleases;
                if ui.checkbox(&mut pr, "Include pre-releases (betas and release candidates)").changed() {
                    self.settings.include_prereleases = pr;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Launcher version");
                    ui.label(RichText::new(format!("v{} (latest release)", env!("CARGO_PKG_VERSION"))).monospace().color(MUTED));
                });
            });

            section(ui, "APPEARANCE", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings.theme, Theme::System, "Match system");
                    ui.selectable_value(&mut self.settings.theme, Theme::Dark, "Dark");
                    ui.selectable_value(&mut self.settings.theme, Theme::Light, "Light");
                });
                ui.label(RichText::new("Changes apply immediately.").size(10.0).color(MUTED));
            });

            section(ui, "DEFAULT INSTALL FOLDER", |ui| {
                ui.label("Install folder");
                ui.add(egui::TextEdit::singleline(&mut self.install_dir_text).desired_width(f32::INFINITY));
                if self.install_dir_text.trim().is_empty() {
                    self.settings.install_dir = None;
                } else {
                    self.settings.install_dir = Some(self.install_dir_text.trim().to_string());
                }
                self.root = self.settings.install_root();
                let _ = self.settings.save();
                ui.horizontal(|ui| {
                    if ui.button("USE DEFAULT").clicked() {
                        self.install_dir_text.clear();
                        self.settings.install_dir = None;
                        self.root = self.settings.install_root();
                        let _ = self.settings.save();
                    }
                    ui.label(RichText::new("Apps are placed inside this folder.").size(10.0).color(MUTED));
                });
            });

            section(ui, "INSTALLING ON LINUX", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Install new apps as");
                    ui.label(RichText::new("AppImage").monospace().color(TEXT));
                });
                ui.label(RichText::new("Updates always use the format an app was installed with.").size(10.0).color(MUTED));
            });

            section(ui, "STARTUP", |ui| {
                let mut autostart = self.settings.autostart;
                if ui.add_enabled(!self.login_pending, egui::Checkbox::new(&mut autostart, "Start Suppaworld when I log in")).changed() {
                    self.settings.autostart = autostart;
                    let _ = self.settings.save();
                    self.login_pending = true;
                    let (tx, ctx) = (self.tx.clone(), ctx.clone());
                    thread::spawn(move || {
                        let result = autostart::set(autostart).map_err(|e| e.to_string());
                        send(&tx, &ctx, Event::Autostart(result));
                    });
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("QUIT LAUNCHER").strong().color(PINK)).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(RichText::new("DONE").strong()).clicked() {
                        self.view = View::Apps;
                    }
                });
            });
        });
    }

    fn apply_action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Install(id) => self.install(id, ctx),
            Action::Open(id) => self.open(id),
            Action::Recheck => self.check(ctx),
        }
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
                Event::Saved(result) => {
                    if let Err(error) = result { self.notice = format!("Impossibile salvare: {error}"); }
                }
            }
        }
        if self.settings.auto_check && self.last_check.elapsed() >= INTERVAL {
            self.check(ctx);
        }
        let any_active = self.rows.iter().any(|row| row.checking || row.progress.is_some());
        if any_active { ctx.request_repaint_after(Duration::from_millis(100)); }
        else { ctx.request_repaint_after(Duration::from_secs(30)); }
        if self.applied_theme != self.settings.theme {
            apply_theme(ctx, self.settings.theme);
            self.applied_theme = self.settings.theme;
        }

        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::new().fill(HEADER).inner_margin(6))
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("SUPPAWORLD").strong().size(18.0).color(ACCENT));
                    ui.label(RichText::new("LAUNCHER").size(10.0).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.view == View::Settings {
                            if ui.button("← Apps").clicked() { self.view = View::Apps; }
                        } else {
                            if ui.button("⚙ Settings").clicked() { self.view = View::Settings; }
                        }
                    });
                });
                if self.view == View::Apps {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("APPS").strong().color(WHITE)).clicked() {}
                        ui.add_space(8.0);
                        if ui.button("⟳").clicked() { self.check(ctx); }
                        ui.add_space(12.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search apps").desired_width(200.0));
                            ui.add_space(10.0);
                            egui::ComboBox::from_id_salt("sort")
                                .selected_text(match self.sort { Sort::NameAsc => "A-Z", Sort::NameDesc => "Z-A", Sort::Newest => "Newest" })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.sort, Sort::NameAsc, "Name A-Z");
                                    ui.selectable_value(&mut self.sort, Sort::NameDesc, "Name Z-A");
                                    ui.selectable_value(&mut self.sort, Sort::Newest, "Newest first");
                                });
                            ui.add_space(8.0);
                            if ui.selectable_label(self.layout == ViewMode::Grid, "Grid").clicked() { self.layout = ViewMode::Grid; }
                            if ui.selectable_label(self.layout == ViewMode::Table, "List").clicked() { self.layout = ViewMode::Table; }
                        });
                    });
                    ui.add_space(4.0);
                }
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("CHECKED {} · USING GITHUB", ago(self.last_check.elapsed()))).size(9.0).monospace().color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).size(9.0).monospace().color(MUTED));
                });
            });
        });

        let mut actions: Vec<Action> = Vec::new();
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.view {
                View::Apps => match self.layout {
                    ViewMode::Table => self.table_view(ui, &mut actions),
                    ViewMode::Grid => self.grid_view(ui, &mut actions),
                },
                View::Settings => self.settings_view(ui, ctx),
            }
        });
        for action in actions {
            self.apply_action(action, ctx);
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 700.0])
            .with_min_inner_size([700.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Suppaworld Launcher",
        options,
        Box::new(|cc| Ok(Box::new(Launcher::new(&cc.egui_ctx)))),
    )
}