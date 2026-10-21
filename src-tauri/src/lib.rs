pub mod commands;
pub mod connections;
pub mod error;
pub mod metrics;
pub mod oauth;
pub mod search;
pub mod share;
pub mod sizes;
pub mod sync;
pub mod thumbs;
pub mod transfer;
pub mod tray;
pub mod vfs;
pub mod volumes;
pub mod watcher;

use commands::AppState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{Emitter, Manager, Runtime};
use tauri_plugin_opener::OpenerExt;

/// How often the transfer list is pushed to the UI while something changes.
const TRANSFER_TICK: Duration = Duration::from_millis(120);
/// I/O monitor sampling period.
const METRICS_TICK: Duration = Duration::from_millis(500);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = build(tauri::Builder::default())
        // Closing the window keeps 5files running in the tray (syncs continue), unless
        // the user turned that off or chose Quit.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if tray::KEEP_RUNNING.load(Ordering::Relaxed) && window.app_handle().tray_by_id("main").is_some() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|_app, _event| {
        // macOS: clicking the Dock icon brings the hidden window back.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            tray::show_main(_app);
        }
    });
}

/// Directory for config + secrets fallback. `FIVEFILES_CONFIG_DIR` overrides it (portable
/// installs, tests).
fn config_dir<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<std::path::PathBuf> {
    match std::env::var_os("FIVEFILES_CONFIG_DIR") {
        Some(d) => Ok(d.into()),
        None => app.path().app_config_dir(),
    }
}

/// Everything except the context, so tests can drive the exact same app with the
/// mock runtime.
pub fn build<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("thumb", |ctx, request, responder| {
            let cache = ctx.app_handle().path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir().join("5files")).join("thumbs");
            let uri = request.uri().to_string();
            tauri::async_runtime::spawn(async move {
                responder.respond(thumbs::respond(uri, cache).await);
            });
        })
        .setup(|app| init(app))
        .invoke_handler(tauri::generate_handler![
            commands::list_dir,
            commands::stat,
            commands::create_dir,
            commands::create_file,
            commands::rename,
            commands::trash,
            commands::check_conflicts,
            commands::transfer_enqueue,
            commands::transfers_list,
            commands::transfer_pause,
            commands::transfer_cancel,
            commands::transfer_retry,
            commands::transfer_remove,
            commands::transfers_clear,
            commands::read_text,
            commands::read_data_url,
            commands::dir_size,
            commands::get_volumes,
            commands::get_places,
            commands::platform,
            commands::connections_list,
            commands::connection_save,
            commands::connection_delete,
            commands::connection_connect,
            commands::connection_disconnect,
            commands::connection_test,
            commands::gdrive_sign_in,
            commands::watch_set,
            commands::search_start,
            commands::search_cancel,
            commands::open_item,
            commands::reveal_item,
            commands::frontend_log,
            commands::e2e_dir,
            commands::upload_begin,
            commands::upload_chunk,
            commands::upload_end,
            commands::folder_sizes,
            commands::sync_list,
            commands::sync_preview,
            commands::sync_save,
            commands::sync_delete,
            commands::sync_run,
            commands::sync_pause,
            commands::eject_volume,
            commands::set_background_mode,
            commands::share_list,
            commands::share_start,
            commands::share_update,
            commands::share_stop,
        ])
}

/// Creates the app state and background tasks. Runs from `setup`; tests call it
/// directly on a mock app (whose event loop never starts).
pub fn init<R: Runtime>(app: &mut tauri::App<R>) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let config_dir = config_dir(app)?;
    let temp_dir = config_dir.join("cache").join("open");
    // Old "open remote file" downloads are disposable.
    let _ = std::fs::remove_dir_all(&temp_dir);

    let local: vfs::DynProvider = Arc::new(vfs::local::LocalFs);
    let use_keychain = std::env::var_os("FIVEFILES_NO_KEYCHAIN").is_none();
    let conns = Arc::new(connections::Connections::load_with(config_dir.clone(), use_keychain));

    let dirty = Arc::new(AtomicBool::new(true));
    let resolve: transfer::Resolver = {
        let (local, conns) = (local.clone(), conns.clone());
        Arc::new(move |fs: String| {
            let (local, conns) = (local.clone(), conns.clone());
            Box::pin(async move { if fs == "local" { Ok(local) } else { conns.provider(&fs).await } })
        })
    };
    let notify: transfer::Notify = {
        let dirty = dirty.clone();
        Arc::new(move || dirty.store(true, Ordering::Relaxed))
    };
    let sizes = Arc::new(sizes::SizeCache::default());
    let on_done: transfer::OnDone = {
        let handle = handle.clone();
        let sizes = sizes.clone();
        Arc::new(move |job: &transfer::JobSnapshot| {
            // Folder sizes containing anything this job touched are stale now.
            for s in &job.sources {
                sizes.invalidate(&s.fs, &s.path);
            }
            if let Some(d) = &job.dest {
                for o in &job.outputs {
                    sizes.invalidate(&d.fs, o);
                }
            }
            if job.open_after
                && let Some(path) = job.outputs.first()
            {
                let _ = handle.opener().open_path(path, None::<&str>);
            }
            let _ = handle.emit("transfer-done", job.clone());
        })
    };
    let transfers = transfer::Manager::new(3, resolve.clone(), notify, on_done);

    // Folder syncs: share the drive resolver; push status changes like transfers do.
    let sync_dirty = Arc::new(AtomicBool::new(true));
    let syncs = {
        let d = sync_dirty.clone();
        sync::Engine::new(config_dir.clone(), resolve, Arc::new(move || d.store(true, Ordering::Relaxed)))
    };
    syncs.start();
    let (syncs_for_tray, transfers_for_tray) = (syncs.clone(), transfers.clone());

    // Folders shared on the local network: pushed to the UI like syncs.
    let share_dirty = Arc::new(AtomicBool::new(true));
    let shares = {
        let d = share_dirty.clone();
        let thumbs = handle.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir().join("5files")).join("thumbs");
        share::Shares::new(thumbs, Arc::new(move || d.store(true, Ordering::Relaxed)))
    };
    let shares_for_tray = shares.clone();

    let watcher = {
        let handle = handle.clone();
        let sizes = sizes.clone();
        watcher::Watcher::new(move |dirs| {
            for d in &dirs {
                sizes.invalidate("local", d);
            }
            let _ = handle.emit("fs-changed", dirs);
        })
    };

    app.manage(AppState {
        local,
        conns,
        transfers: transfers.clone(),
        syncs: syncs.clone(),
        watcher,
        searches: Default::default(),
        sizes,
        temp_dir,
        uploads: Default::default(),
        shares: shares.clone(),
    });

    // Transfer updates: coalesce many progress ticks into ~8 UI updates/second.
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let mut iv = tokio::time::interval(TRANSFER_TICK);
            loop {
                iv.tick().await;
                if dirty.swap(false, Ordering::Relaxed) {
                    let _ = handle.emit("transfers", transfers.snapshots());
                }
                if sync_dirty.swap(false, Ordering::Relaxed) {
                    let _ = handle.emit("syncs", syncs.views());
                }
                shares.expire().await;
                if share_dirty.swap(false, Ordering::Relaxed) {
                    let _ = handle.emit("shares", shares.views());
                }
            }
        });
    }
    // Safety net: the UI shows the window after its first paint; if that never
    // happens (e.g. a script error), show it anyway rather than staying invisible.
    {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Some(w) = handle.get_webview_window("main")
                && !w.is_visible().unwrap_or(true)
            {
                eprintln!("[5files] window was not shown by the UI; showing it");
                let _ = w.show();
            }
        });
    }
    // Tray icon (optional: some Linux desktops have no tray).
    // FIVEFILES_NO_TRAY: headless test runs (native menus need the main thread).
    let tray_icon = if std::env::var_os("FIVEFILES_NO_TRAY").is_some() {
        None
    } else {
        match tray::Tray::build(&handle, syncs_for_tray.clone(), shares_for_tray.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("[5files] no system tray: {e}");
                None
            }
        }
    };
    // App-scoped disk / network throughput, and the tray's live status.
    {
        let handle = handle.clone();
        let transfers = transfers_for_tray;
        tauri::async_runtime::spawn(async move {
            let mut sampler = metrics::RateSampler::new(120);
            let mut iv = tokio::time::interval(METRICS_TICK);
            loop {
                iv.tick().await;
                let _ = handle.emit("metrics", sampler.sample());
                if let Some(t) = &tray_icon {
                    t.update(&transfers.snapshots(), &syncs_for_tray.views(), &shares_for_tray.brief());
                }
            }
        });
    }
    Ok(())
}
