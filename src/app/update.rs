//! Updating the app: the notice in the top bar and the dialog from the question to the restart.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align, Id, Layout, RichText, Sense, Vec2};
use egui_phosphor::regular as ph;
use kage::config;
use kage::update::{self, Error, Release};

use super::settings::open_in_explorer;
use super::App;
use crate::i18n::T;
use crate::theme::{dialog_frame, primary_button, AMBER, BLUE, LINE, MUTED, RED, SECTION_GAP, SIZE_BODY, SIZE_TITLE};

const DIALOG_WIDTH: f32 = 420.0;
const PROGRESS_HEIGHT: f32 = 6.0;
const PROGRESS_REPAINT: Duration = Duration::from_millis(100);
const BYTES_PER_MB: f64 = 1_000_000.0;

pub(super) enum Dialog {
    Confirm,
    Download { rx: Receiver<Result<(), Error>>, progress: Arc<AtomicU64>, cancel: Arc<AtomicBool> },
    Failed { message: &'static str, detail: String },
}

#[derive(Default)]
pub(super) struct Update {
    check_rx: Option<Receiver<Option<Release>>>,
    pub(super) release: Option<Release>,
    pub(super) dialog: Option<Dialog>,
    /// Started once the settings are saved, as the last thing the app does.
    pub(super) restart: Option<PathBuf>,
}

impl App {
    /// In the background: a slow or missing connection must not be noticed.
    pub(super) fn check_for_update(&mut self, ctx: &egui::Context) {
        let test = std::env::var_os(update::URL_ENV).is_some();
        if !test && (!self.cfg.check_updates || std::env::var_os("KAGE_SHOT").is_some()) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (ctx, log) = (ctx.clone(), self.log.clone());
        std::thread::spawn(move || {
            let release = update::check().unwrap_or_else(|e| {
                log.info(format!("Update check failed: {e}"));
                None
            });
            let _ = tx.send(release);
            ctx.request_repaint();
        });
        self.update.check_rx = Some(rx);
    }

    fn can_update(&self) -> bool {
        !self.running() && self.export_rx.is_none() && !self.export_pending
    }

    /// Right to left: the button first, its text next to it.
    pub(super) fn update_notice(&mut self, ui: &mut egui::Ui, t: &'static T) {
        if self.update.release.is_none() {
            return;
        }
        ui.separator();
        let button = egui::Button::new(format!("{}  {}", ph::ARROW_CIRCLE_UP, t.update));
        if ui.add_enabled(self.can_update(), button).on_disabled_hover_text(t.update_busy).clicked() {
            self.update.dialog = Some(Dialog::Confirm);
        }
        ui.label(RichText::new(t.update_available).color(BLUE));
    }

    pub(super) fn start_update(&mut self, ctx: &egui::Context) {
        let Some(release) = self.update.release.clone() else { return };
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (thread_progress, thread_cancel, ctx) = (progress.clone(), cancel.clone(), ctx.clone());
        let exe = std::env::current_exe().unwrap_or_else(|_| config::exe_dir().join("AtozukeKageMOD.exe"));
        std::thread::spawn(move || {
            let dir = update::work_dir(&exe);
            let result = update::download(&release, &dir, &thread_progress, &thread_cancel).and_then(|archive| update::unpack(&archive, &dir));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.update.dialog = Some(Dialog::Download { rx, progress, cancel });
    }

    fn update_failed(&mut self, error: &Error) {
        let t = self.t();
        let message = match error {
            Error::Cancelled => {
                self.update.dialog = None;
                return;
            }
            Error::Download(_) => t.update_errors[0],
            Error::Verify(_) => t.update_errors[1],
            Error::Install(_) => t.update_errors[2],
        };
        self.log.warn(format!("Update failed: {}", error.detail()));
        self.update.dialog = Some(Dialog::Failed { message, detail: error.detail().to_string() });
    }

    /// The new exe is in place: it starts when this app has closed.
    fn finish_update(&mut self, ctx: &egui::Context) {
        let result = std::env::current_exe().map_err(|e| Error::Install(e.to_string())).and_then(|exe| {
            update::install(&update::work_dir(&exe), &exe)?;
            Ok(exe)
        });
        match result {
            Ok(exe) => {
                self.log.info(format!("Updated to {}", self.update.release.as_ref().map_or("?", |r| &r.version)));
                self.update.dialog = None;
                self.update.restart = Some(exe);
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.update_failed(&e),
        }
    }

    pub(super) fn tick_update(&mut self, ctx: &egui::Context) {
        if let Some(found) = self.update.check_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.update.check_rx = None;
            match &found {
                Some(release) => self.log.info(format!("Update available: {}", release.version)),
                None => self.log.info("No update found"),
            }
            self.update.release = found;
        }
        let Some(Dialog::Download { rx, .. }) = &self.update.dialog else { return };
        ctx.request_repaint_after(PROGRESS_REPAINT);
        match rx.try_recv() {
            Ok(Ok(())) => self.finish_update(ctx),
            Ok(Err(e)) => self.update_failed(&e),
            Err(_) => {}
        }
    }

    /// Runs the new exe. The test hooks must not carry over, or it would update itself again.
    pub(super) fn restart_after_update(&mut self) {
        let Some(exe) = self.update.restart.take() else { return };
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Err(e) = std::process::Command::new(&exe).current_dir(dir).env_remove("KAGE_AUTO").spawn() {
            self.log.error(format!("Could not start {}: {e}", exe.display()));
        }
    }

    pub(super) fn update_modal(&mut self, ctx: &egui::Context) {
        let t = self.t();
        let version = self.update.release.as_ref().map(|r| r.version.clone()).unwrap_or_default();
        let total = self.update.release.as_ref().map_or(0, |r| r.size);
        let unsaved = self.history.iter().any(|e| !e.exported);
        let mut start = false;
        let mut close = false;
        let dialog = &self.update.dialog;
        let resp = egui::Modal::new(Id::new("update")).frame(dialog_frame()).show(ctx, |ui| {
            ui.set_width(DIALOG_WIDTH);
            ui.label(RichText::new(t.update).strong().size(SIZE_TITLE));
            ui.separator();
            match dialog {
                Some(Dialog::Confirm) => {
                    ui.add(egui::Label::new(t.update_confirm.replace("{}", &version)).wrap());
                    if unsaved {
                        ui.label(RichText::new(t.update_unsaved).color(AMBER));
                    }
                }
                Some(Dialog::Download { progress, .. }) => {
                    let done = progress.load(Ordering::SeqCst);
                    ui.label(t.update_downloading.replace("{}", &version));
                    let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PROGRESS_HEIGHT), Sense::hover());
                    ui.painter().rect_filled(bar, egui::CornerRadius::same(1), LINE);
                    let part = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                    let fill = egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * part, bar.height()));
                    ui.painter().rect_filled(fill, egui::CornerRadius::same(1), BLUE);
                    ui.label(RichText::new(format!("{:.1} / {:.1} MB", done as f64 / BYTES_PER_MB, total as f64 / BYTES_PER_MB)).monospace().color(MUTED));
                }
                Some(Dialog::Failed { message, detail }) => {
                    ui.label(RichText::new(t.update_failed).color(RED));
                    ui.add(egui::Label::new(t.update_failed_note).wrap());
                    ui.add(egui::Label::new(RichText::new(*message).monospace().color(MUTED)).wrap()).on_hover_text(detail);
                }
                None => {}
            }
            ui.add_space(SECTION_GAP);
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match dialog {
                Some(Dialog::Confirm) => {
                    if ui.add(primary_button(t.update_go, SIZE_BODY, true)).clicked() {
                        start = true;
                    }
                    close = ui.button(t.cancel).clicked();
                }
                Some(Dialog::Download { cancel, .. }) => {
                    if ui.button(t.cancel).clicked() {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
                _ => {
                    close = ui.button(t.close).clicked();
                    if ui.button(format!("{}  {}", ph::LINK, t.update_page)).on_hover_text(update::RELEASES_PAGE).clicked() {
                        open_in_explorer(Path::new(update::RELEASES_PAGE));
                    }
                }
            });
        });
        // Clicking beside the dialog or Esc: a running download has its own Cancel.
        if close || (resp.should_close() && !matches!(self.update.dialog, Some(Dialog::Download { .. }))) {
            self.update.dialog = None;
        }
        if start {
            self.start_update(ctx);
        }
    }
}
