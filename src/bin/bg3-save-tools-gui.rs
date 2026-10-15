#![windows_subsystem = "windows"]

use bg3_save_tools::{
    Backup, Config, Divine, HonourRun, LSLIB_RELEASES_URL, Paths, backup_run, download_lslib,
    game_running, honour_runs, restore_backup, revive,
};
use eframe::egui::{self, Color32, RichText};
use std::{
    path::Path,
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::Duration,
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BG3 Save Tools")
            .with_inner_size([780.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "BG3 Save Tools",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}

enum Msg {
    Runs(Result<Vec<HonourRun>, String>),
    Divine(Result<Divine, String>),
    Done(Result<String, String>),
    GameRunning(bool),
}

enum Confirm {
    Revive(HonourRun),
    Restore(Backup),
}

struct App {
    paths: Result<Paths, String>,
    divine: Option<Divine>,
    divine_error: Option<String>,
    runs: Vec<HonourRun>,
    busy: Option<String>,
    status: Option<Result<String, String>>,
    confirm: Option<Confirm>,
    game_running: bool,
    show_deleted: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();

        // poll for the game in the background; writes are refused while it runs
        let (game_tx, game_ctx) = (tx.clone(), ctx.clone());
        thread::spawn(move || {
            loop {
                if game_tx.send(Msg::GameRunning(game_running())).is_err() {
                    break;
                }
                game_ctx.request_repaint();
                thread::sleep(Duration::from_secs(3));
            }
        });

        let (divine, divine_error) = match Divine::locate(None) {
            Ok(d) => (Some(d), None),
            Err(e) => (None, Some(e)),
        };
        let mut app = Self {
            paths: Paths::detect(),
            divine,
            divine_error,
            runs: vec![],
            busy: None,
            status: None,
            confirm: None,
            game_running: false,
            show_deleted: false,
            tx,
            rx,
        };
        app.refresh(ctx);
        app
    }

    /// Runs `job` on a worker thread, showing `label` until it reports back.
    fn spawn(&mut self, ctx: &egui::Context, label: &str, job: impl FnOnce() -> Msg + Send + 'static) {
        self.busy = Some(label.to_string());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        let (Ok(paths), Some(divine)) = (self.paths.clone(), self.divine.clone()) else {
            return;
        };
        self.spawn(ctx, "Reading saves…", move || Msg::Runs(honour_runs(&divine, &paths)));
    }

    /// Runs a write action on a worker thread and refreshes afterwards.
    fn act(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        job: impl FnOnce(&Divine, &Paths) -> Result<String, String> + Send + 'static,
    ) {
        let (Ok(paths), Some(divine)) = (self.paths.clone(), self.divine.clone()) else {
            return;
        };
        self.spawn(ctx, label, move || Msg::Done(job(&divine, &paths)));
    }

    fn handle_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::GameRunning(running) => self.game_running = running,
                Msg::Runs(runs) => {
                    self.busy = None;
                    match runs {
                        Ok(runs) => self.runs = runs,
                        Err(e) => self.status = Some(Err(e)),
                    }
                }
                Msg::Divine(result) => {
                    self.busy = None;
                    match result {
                        Ok(divine) => {
                            let config = Config {
                                divine: Some(divine.exe().to_path_buf()),
                            };
                            if let Err(e) = config.save() {
                                self.status = Some(Err(e));
                            }
                            self.divine = Some(divine);
                            self.divine_error = None;
                            self.refresh(ctx);
                        }
                        Err(e) => self.divine_error = Some(e),
                    }
                }
                Msg::Done(result) => {
                    self.busy = None;
                    self.status = Some(result);
                    self.refresh(ctx);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages(ctx);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("BG3 Honour Mode Save Tools");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let idle = self.busy.is_none() && self.divine.is_some();
                    if ui.add_enabled(idle, egui::Button::new("⟳ Refresh")).clicked() {
                        self.refresh(ctx);
                    }
                    if let Ok(paths) = &self.paths
                        && ui.button("📁 Open backups folder").clicked()
                    {
                        let _ = std::fs::create_dir_all(paths.backups_dir());
                        open_folder(&paths.backups_dir());
                    }
                });
            });
            if self.game_running {
                ui.colored_label(
                    Color32::from_rgb(230, 150, 40),
                    "⚠ Baldur's Gate 3 is running. Close it before reviving or restoring.",
                );
            }
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            match (&self.busy, &self.status) {
                (Some(label), _) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(label);
                    });
                }
                (None, Some(Ok(msg))) => {
                    ui.colored_label(Color32::from_rgb(90, 180, 90), msg);
                }
                (None, Some(Err(msg))) => {
                    ui.colored_label(Color32::from_rgb(220, 80, 80), msg);
                }
                (None, None) => {
                    ui.weak("Ready");
                }
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Err(e) = &self.paths {
                ui.colored_label(Color32::RED, e);
                return;
            }
            if self.divine.is_none() {
                self.lslib_setup(ui, ctx);
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| self.run_list(ui, ctx));
        });

        self.confirm_dialog(ctx);
    }
}

impl App {
    fn lslib_setup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(20.0);
        ui.heading("One-time setup: LSLib");
        ui.label(
            "Reading and editing BG3 saves needs Divine.exe from LSLib, a free community tool \
             by Norbyte.",
        );
        if let Some(e) = &self.divine_error {
            ui.weak(e);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let idle = self.busy.is_none();
            if ui
                .add_enabled(idle, egui::Button::new("⬇ Download LSLib automatically"))
                .clicked()
            {
                self.spawn(ctx, "Downloading LSLib…", || Msg::Divine(download_lslib()));
            }
            if ui
                .add_enabled(idle, egui::Button::new("Browse for Divine.exe…"))
                .clicked()
                && let Some(exe) = rfd::FileDialog::new()
                    .add_filter("Divine.exe", &["exe"])
                    .pick_file()
            {
                let _ = self.tx.send(Msg::Divine(Divine::new(exe)));
            }
        });
        ui.add_space(6.0);
        ui.hyperlink_to("LSLib releases on GitHub", LSLIB_RELEASES_URL);
    }

    fn run_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let deleted = self
            .runs
            .iter()
            .filter(|r| r.lsv.is_none() && r.backups.is_empty())
            .count();
        if self.runs.len() == deleted && self.busy.is_none() {
            ui.label("No Honour Mode runs found.");
        }

        let runs = self.runs.clone();
        for run in runs.iter().filter(|r| r.lsv.is_some() || !r.backups.is_empty()) {
            self.run_card(ui, ctx, run);
            ui.add_space(8.0);
        }

        if deleted > 0 {
            ui.checkbox(
                &mut self.show_deleted,
                format!("Show {deleted} deleted run(s) with revoked Honour Mode"),
            );
            if self.show_deleted {
                for run in runs.iter().filter(|r| r.lsv.is_none() && r.backups.is_empty()) {
                    ui.weak(format!("{}  (save deleted)", run.session_id));
                }
            }
        }
    }

    fn run_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, run: &HonourRun) {
        let idle = self.busy.is_none();
        let writable = idle && !self.game_running;
        egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let summary = run.best_summary();

            ui.horizontal(|ui| {
                let leader = summary.map(|s| s.leader.as_str()).unwrap_or("Unknown party");
                ui.label(RichText::new(leader).heading().strong());
                if run.failed {
                    badge(ui, "Honour Mode revoked", Color32::from_rgb(200, 70, 70));
                } else {
                    badge(ui, "Honour Mode", Color32::from_rgb(60, 150, 80));
                }
                if run.lsv.is_none() {
                    badge(ui, "save deleted", Color32::DARK_GRAY);
                }
            });

            match summary {
                Some(s) => {
                    ui.label(format!(
                        "{} · {} played · last saved {}",
                        s.act(),
                        s.playtime(),
                        s.saved
                    ));
                    egui::Grid::new(("party", &run.session_id))
                        .num_columns(4)
                        .spacing([14.0, 2.0])
                        .show(ui, |ui| {
                            for m in s.members() {
                                ui.label(format!("Lv {}", m.level));
                                ui.label(RichText::new(m.name()).strong());
                                ui.label(&m.race);
                                ui.label(m.classes_text());
                                ui.end_row();
                            }
                        });
                    let summons: Vec<_> = s.summons().map(|m| m.race.as_str()).collect();
                    if !summons.is_empty() {
                        ui.weak(format!("Summons: {}", summons.join(", ")));
                    }
                }
                None => {
                    if let Some(e) = &run.summary_error {
                        ui.colored_label(Color32::RED, format!("Couldn't read save: {e}"));
                    }
                }
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if run.lsv.is_some()
                    && ui
                        .add_enabled(idle, egui::Button::new("💾 Back up now"))
                        .on_hover_text("Copy this save to the backups folder")
                        .clicked()
                {
                    let id = run.session_id.clone();
                    self.act(ctx, "Backing up…", move |divine, paths| {
                        backup_run(Some(divine), paths, &id, None)
                            .map(|b| format!("Backed up as {}", b.name))
                    });
                }
                if run.failed
                    && ui
                        .add_enabled(writable, egui::Button::new("✨ Revive Honour Mode"))
                        .on_hover_text("Undo the switch to Custom Mode after a party wipe")
                        .clicked()
                {
                    self.confirm = Some(Confirm::Revive(run.clone()));
                }
            });

            if !run.backups.is_empty() {
                egui::CollapsingHeader::new(format!("Backups ({})", run.backups.len()))
                    .id_salt(("backups", &run.session_id))
                    .show(ui, |ui| {
                        for b in &run.backups {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(writable, egui::Button::new("Restore"))
                                    .clicked()
                                {
                                    self.confirm = Some(Confirm::Restore(b.clone()));
                                }
                                ui.label(backup_label(b));
                            });
                        }
                    });
            }
            ui.weak(RichText::new(&run.session_id).small());
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let (title, text) = match confirm {
            Confirm::Revive(run) => (
                "Revive Honour Mode?",
                format!(
                    "This makes the {} run count as Honour Mode again. Your profile is backed \
                     up first.",
                    run.best_summary().map(|s| s.leader.as_str()).unwrap_or("selected")
                ),
            ),
            Confirm::Restore(b) => (
                "Restore this backup?",
                format!(
                    "The run's current save will be replaced by the backup from {}. The current \
                     save is backed up first, and Honour Mode is revived if it was revoked.",
                    b.name
                ),
            ),
        };

        let mut decision = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Yes, go ahead").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });

        match decision {
            Some(true) => match self.confirm.take() {
                Some(Confirm::Revive(run)) => {
                    let id = run.session_id;
                    self.act(ctx, "Reviving Honour Mode…", move |divine, paths| {
                        revive(divine, paths, std::slice::from_ref(&id))
                            .map(|_| "Honour Mode revived. If Steam asks about a cloud conflict, keep the local files.".to_string())
                    });
                }
                Some(Confirm::Restore(backup)) => {
                    self.act(ctx, "Restoring backup…", move |divine, paths| {
                        restore_backup(divine, paths, &backup).map(|r| {
                            let mut msg = format!("Restored backup {}.", backup.name);
                            if r.revived {
                                msg.push_str(" Honour Mode revived.");
                            }
                            msg
                        })
                    });
                }
                None => {}
            },
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| ui.label(RichText::new(text).color(Color32::WHITE).small()));
}

fn backup_label(b: &Backup) -> String {
    match &b.summary {
        Some(s) => format!("{} · {} · {} played", b.name, s.act(), s.playtime()),
        None => b.name.clone(),
    }
}

fn open_folder(path: &Path) {
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}
