#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod input;
mod matcher;
mod worker;

use config::Config;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

slint::include_modules!();

const MAX_LOG_LINES: usize = 1000;

fn timestamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

fn push_log(ui: &AppWindow, line: &str) {
    let model = ui.get_log_lines();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
        vm.insert(0, format!("[{}] {line}", timestamp()).into());
        if vm.row_count() > MAX_LOG_LINES {
            vm.remove(MAX_LOG_LINES);
        }
    }
}

fn read_settings(ui: &AppWindow, cfg: &mut Config) {
    cfg.max_loops = ui.get_max_loops().max(1) as u32;
    cfg.click_interval_ms = ui.get_click_interval_ms().max(0) as u32;
    cfg.loop_interval_ms = ui.get_loop_interval_ms().max(0) as u32;
    cfg.threshold = ui.get_threshold();
    cfg.minimize_on_start = ui.get_minimize_on_start();
}

struct State {
    base: PathBuf,
    images_dir: PathBuf,
    cfg: Config,
    image_paths: Vec<PathBuf>,
    stop: Option<Arc<AtomicBool>>,
}

impl State {
    fn save(&self, ui: &AppWindow) {
        if let Err(e) = self.cfg.save(&self.base) {
            push_log(ui, &format!("Could not save config: {e:#}"));
        }
    }
}

fn reload_images(ui: &AppWindow, state: &mut State, model: &VecModel<ImageItem>) {
    state.image_paths = config::list_images(&state.images_dir);
    let items: Vec<ImageItem> = state
        .image_paths
        .iter()
        .map(|p| {
            let name = file_name(p);
            ImageItem {
                enabled: !state.cfg.disabled_images.contains(&name),
                preview: slint::Image::load_from_path(p).unwrap_or_default(),
                name: name.into(),
            }
        })
        .collect();
    model.set_vec(items);
    if state.image_paths.is_empty() {
        push_log(
            ui,
            &format!("No .png images found in {}", state.images_dir.display()),
        );
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Copies `src` into `dir`, adding a numeric suffix if the name is taken.
fn copy_unique(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dest = dir.join(format!("{stem}.png"));
    let mut n = 2;
    while dest.exists() {
        dest = dir.join(format!("{stem}_{n}.png"));
        n += 1;
    }
    std::fs::copy(src, &dest)?;
    Ok(dest)
}

fn main() -> anyhow::Result<()> {
    input::enable_dpi_awareness();

    let base = config::base_dir();
    let images_dir = base.join(config::IMAGES_DIR);
    config::ensure_images_dir(&images_dir)?;
    let cfg = Config::load(&base);

    let ui = AppWindow::new()?;
    ui.set_max_loops(cfg.max_loops.min(i32::MAX as u32) as i32);
    ui.set_click_interval_ms(cfg.click_interval_ms as i32);
    ui.set_loop_interval_ms(cfg.loop_interval_ms as i32);
    ui.set_threshold(cfg.threshold);
    ui.set_minimize_on_start(cfg.minimize_on_start);

    let log_model = Rc::new(VecModel::<SharedString>::default());
    ui.set_log_lines(ModelRc::from(log_model.clone()));
    let images_model = Rc::new(VecModel::<ImageItem>::default());
    ui.set_images(ModelRc::from(images_model.clone()));

    let state = Rc::new(RefCell::new(State {
        base,
        images_dir,
        cfg,
        image_paths: Vec::new(),
        stop: None,
    }));
    reload_images(&ui, &mut state.borrow_mut(), &images_model);
    push_log(
        &ui,
        &format!("Images folder: {}", state.borrow().images_dir.display()),
    );

    ui.on_start({
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let images_model = images_model.clone();
        move || {
            let ui = ui_weak.unwrap();
            let mut st = state.borrow_mut();
            if st.stop.is_some() {
                return;
            }
            read_settings(&ui, &mut st.cfg);
            st.save(&ui);

            let images: Vec<PathBuf> = st
                .image_paths
                .iter()
                .zip(images_model.iter())
                .filter(|(_, item)| item.enabled)
                .map(|(p, _)| p.clone())
                .collect();
            if images.is_empty() {
                push_log(&ui, "Select at least one image before starting.");
                return;
            }

            let params = worker::Params {
                images,
                max_loops: st.cfg.max_loops,
                click_interval: Duration::from_millis(st.cfg.click_interval_ms as u64),
                loop_interval: Duration::from_millis(st.cfg.loop_interval_ms as u64),
                threshold: st.cfg.threshold,
            };
            let stop = Arc::new(AtomicBool::new(false));
            st.stop = Some(stop.clone());
            ui.set_running(true);
            ui.set_iteration(0);
            ui.set_status("Starting".into());
            if st.cfg.minimize_on_start {
                ui.window().set_minimized(true);
            }

            let ui_weak = ui.as_weak();
            std::thread::spawn(move || {
                worker::run(params, stop, move |event| {
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| match event {
                        worker::Event::Log(s) => push_log(&ui, &s),
                        worker::Event::Status(s) => ui.set_status(s.into()),
                        worker::Event::Iteration(i) => {
                            ui.set_iteration(i.min(i32::MAX as u32) as i32)
                        }
                        worker::Event::Finished(s) => {
                            ui.set_running(false);
                            ui.set_status(s.into());
                            ui.window().set_minimized(false);
                            ui.invoke_worker_finished();
                        }
                    });
                });
            });
        }
    });

    ui.on_stop({
        let state = state.clone();
        move || {
            if let Some(stop) = &state.borrow().stop {
                stop.store(true, Ordering::Relaxed);
            }
        }
    });

    ui.on_worker_finished({
        let state = state.clone();
        move || state.borrow_mut().stop = None
    });

    ui.on_image_toggled({
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let images_model = images_model.clone();
        move |index, enabled| {
            let ui = ui_weak.unwrap();
            let index = index as usize;
            if let Some(mut item) = images_model.row_data(index) {
                item.enabled = enabled;
                images_model.set_row_data(index, item.clone());
                let mut st = state.borrow_mut();
                let name = item.name.to_string();
                st.cfg.disabled_images.retain(|n| *n != name);
                if !enabled {
                    st.cfg.disabled_images.push(name);
                }
                st.save(&ui);
            }
        }
    });

    ui.on_add_images({
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let images_model = images_model.clone();
        move || {
            let ui = ui_weak.unwrap();
            let Some(files) = rfd::FileDialog::new()
                .set_title("Choose button screenshots")
                .add_filter("PNG image", &["png"])
                .pick_files()
            else {
                return;
            };
            let mut st = state.borrow_mut();
            for f in files {
                match copy_unique(&f, &st.images_dir) {
                    Ok(dest) => push_log(&ui, &format!("Added {}", file_name(&dest))),
                    Err(e) => push_log(&ui, &format!("Could not copy {}: {e}", f.display())),
                }
            }
            reload_images(&ui, &mut st, &images_model);
        }
    });

    ui.on_reload_images({
        let ui_weak = ui.as_weak();
        let state = state.clone();
        let images_model = images_model.clone();
        move || reload_images(&ui_weak.unwrap(), &mut state.borrow_mut(), &images_model)
    });

    ui.on_open_folder({
        let state = state.clone();
        move || {
            let _ = std::process::Command::new("explorer")
                .arg(&state.borrow().images_dir)
                .spawn();
        }
    });

    ui.on_clear_log({
        let log_model = log_model.clone();
        move || log_model.set_vec(Vec::new())
    });

    ui.window().on_close_requested({
        let state = state.clone();
        move || {
            if let Some(stop) = &state.borrow().stop {
                stop.store(true, Ordering::Relaxed);
            }
            slint::CloseRequestResponse::HideWindow
        }
    });

    ui.run()?;

    let mut st = state.borrow_mut();
    read_settings(&ui, &mut st.cfg);
    st.save(&ui);
    Ok(())
}
