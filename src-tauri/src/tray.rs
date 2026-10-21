//! System tray / menu bar icon: live transfer and sync status, quick actions, and a
//! place for 5files to keep running (syncs continue) after the window is closed.

use crate::sync;
use crate::transfer::{self, JobKind};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Closing the window hides it to the tray instead of quitting (setting, default on).
pub static KEEP_RUNNING: AtomicBool = AtomicBool::new(true);

const ICON_IDLE: &[u8] = include_bytes!("../icons/tray-idle.png");
const ICON_ACTIVE: &[u8] = include_bytes!("../icons/tray-active.png");
#[cfg(not(target_os = "macos"))]
const ICON_APP: &[u8] = include_bytes!("../icons/32x32.png");

pub struct Tray<R: Runtime> {
    icon: TrayIcon<R>,
    transfers: MenuItem<R>,
    syncs: MenuItem<R>,
    sync_all: MenuItem<R>,
    pause_all: CheckMenuItem<R>,
    last: Mutex<(String, String, bool, bool)>,
}

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn pct(done: u64, total: u64) -> u64 {
    if total == 0 { 0 } else { (done * 100 / total).min(100) }
}

/// "Copying photo.jpg · 45%", "3 transfers · 45%", "No transfers running".
pub fn transfers_text(jobs: &[transfer::JobSnapshot]) -> (String, bool) {
    let active: Vec<_> = jobs.iter().filter(|j| !j.status.is_final()).collect();
    match active.as_slice() {
        [] => ("No transfers running".into(), false),
        [j] => {
            let verb = match j.kind {
                JobKind::Copy => "Copying",
                JobKind::Move => "Moving",
                JobKind::Delete => "Deleting",
            };
            (format!("{verb} {} · {}%", j.title, pct(j.done_bytes, j.total_bytes)), true)
        }
        many => {
            let (d, t) = many.iter().fold((0, 0), |(d, t), j| (d + j.done_bytes, t + j.total_bytes));
            (format!("{} transfers · {}%", many.len(), pct(d, t)), true)
        }
    }
}

/// One line for the state of all syncs, most urgent first.
pub fn syncs_text(views: &[sync::SyncView]) -> (String, bool) {
    use sync::State;
    if views.is_empty() {
        return ("No folder syncs".into(), false);
    }
    if let Some(v) = views.iter().find(|v| matches!(v.state, State::Syncing { .. })) {
        if let State::Syncing { done, total, .. } = v.state {
            return (format!("Syncing “{}” · {}%", v.pair.name, pct(done as u64, total.max(1) as u64)), true);
        }
    }
    if let Some(v) = views.iter().find(|v| matches!(v.state, State::Review { .. })) {
        return (format!("“{}” needs your review", v.pair.name), false);
    }
    if let Some(v) = views.iter().find(|v| matches!(v.state, State::Error { .. })) {
        return (format!("“{}” couldn't sync", v.pair.name), false);
    }
    if views.iter().any(|v| matches!(v.state, State::Scanning)) {
        return ("Checking syncs for changes…".into(), true);
    }
    if views.iter().all(|v| v.pair.paused) {
        return ("Syncs paused".into(), false);
    }
    match views {
        [v] => (format!("“{}” is up to date", v.pair.name), false),
        _ => (format!("{} syncs up to date", views.len()), false),
    }
}

impl<R: Runtime> Tray<R> {
    pub fn build(app: &AppHandle<R>, syncs: Arc<sync::Engine>) -> tauri::Result<Arc<Self>> {
        let open = MenuItem::with_id(app, "open", "Open 5files", true, None::<&str>)?;
        let transfers = MenuItem::with_id(app, "transfers", "No transfers running", true, None::<&str>)?;
        let syncs_item = MenuItem::with_id(app, "syncs", "No folder syncs", true, None::<&str>)?;
        let sync_all = MenuItem::with_id(app, "sync-all", "Sync all now", false, None::<&str>)?;
        let pause_all = CheckMenuItem::with_id(app, "pause-all", "Pause syncs", true, false, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit 5files", true, None::<&str>)?;
        let menu = Menu::with_items(
            app,
            &[
                &open,
                &PredefinedMenuItem::separator(app)?,
                &transfers,
                &syncs_item,
                &sync_all,
                &pause_all,
                &PredefinedMenuItem::separator(app)?,
                &quit,
            ],
        )?;

        let engine = syncs.clone();
        let builder = TrayIconBuilder::with_id("main")
            .tooltip("5files")
            .menu(&menu)
            .on_menu_event(move |app, event| match event.id.as_ref() {
                "open" => show_main(app),
                "transfers" | "syncs" => {
                    show_main(app);
                    let _ = app.emit("tray-open", event.id.as_ref());
                }
                "sync-all" => engine.run_all(),
                "pause-all" => {
                    let pause = !engine.all_paused();
                    engine.pause_all(pause);
                }
                "quit" => {
                    KEEP_RUNNING.store(false, Ordering::Relaxed);
                    app.exit(0);
                }
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                // Windows/Linux convention: left click opens the window, right click the menu.
                if cfg!(not(target_os = "macos")) {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                }
            });

        #[cfg(target_os = "macos")]
        let builder = builder.icon(Image::from_bytes(ICON_IDLE)?).icon_as_template(true);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.icon(Image::from_bytes(ICON_APP)?).show_menu_on_left_click(false);

        let icon = builder.build(app)?;
        Ok(Arc::new(Self { icon, transfers, syncs: syncs_item, sync_all, pause_all, last: Mutex::new(Default::default()) }))
    }

    /// Refreshes texts and the activity icon (only touches what changed).
    pub fn update(&self, jobs: &[transfer::JobSnapshot], views: &[sync::SyncView]) {
        let (t_text, t_active) = transfers_text(jobs);
        let (s_text, s_active) = syncs_text(views);
        let active = t_active || s_active;
        let paused = !views.is_empty() && views.iter().all(|v| v.pair.paused);
        let mut last = self.last.lock();
        let next = (t_text.clone(), s_text.clone(), active, paused);
        if *last == next {
            return;
        }
        if last.0 != t_text {
            let _ = self.transfers.set_text(&t_text);
        }
        if last.1 != s_text {
            let _ = self.syncs.set_text(&s_text);
        }
        let _ = self.sync_all.set_enabled(!views.is_empty() && !paused);
        let _ = self.pause_all.set_enabled(!views.is_empty());
        let _ = self.pause_all.set_checked(paused);
        let _ = self.icon.set_tooltip(Some(format!("5files\n{t_text}\n{s_text}")));
        #[cfg(target_os = "macos")]
        if last.2 != active {
            if let Ok(img) = Image::from_bytes(if active { ICON_ACTIVE } else { ICON_IDLE }) {
                let _ = self.icon.set_icon(Some(img));
                let _ = self.icon.set_icon_as_template(true);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ICON_ACTIVE;
        *last = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{JobSnapshot, Status};

    fn job(kind: JobKind, status: Status, done: u64, total: u64) -> JobSnapshot {
        JobSnapshot {
            id: "j".into(),
            kind,
            status,
            title: "photo.jpg".into(),
            src_fs: "local".into(),
            dest: None,
            total_bytes: total,
            done_bytes: done,
            total_files: 1,
            done_files: 0,
            skipped: 0,
            current: None,
            speed: 0.0,
            eta_secs: None,
            error: None,
            created_at: 0,
            finished_at: None,
            open_after: false,
            outputs: vec![],
            sources: vec![],
        }
    }

    #[test]
    fn transfer_lines() {
        assert_eq!(transfers_text(&[]).0, "No transfers running");
        assert_eq!(transfers_text(&[job(JobKind::Copy, Status::Running, 45, 100)]).0, "Copying photo.jpg · 45%");
        let two = [
            job(JobKind::Move, Status::Running, 10, 100),
            job(JobKind::Copy, Status::Running, 90, 100),
            job(JobKind::Copy, Status::Done, 5, 5),
        ];
        assert_eq!(transfers_text(&two), ("2 transfers · 50%".to_string(), true));
    }
}
