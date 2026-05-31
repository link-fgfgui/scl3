#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod avatar;
mod callbacks;
mod config;
mod pages;
mod ui_sync;

use std::error::Error;
use std::sync::{Arc, Mutex};

use clapfig::{Clapfig, SearchPath};
use config::SclConfig;
use tracing::info;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
use ui_sync::{save_config, sync_config_to_ui};

use slint::ComponentHandle;
pub mod ui {
    slint::include_modules!();
}

fn init_tracing() {
    let log_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("scl3")
        .join("logs");
    let file_appender = tracing_appender::rolling::daily(log_dir, "scl3.log");
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(if cfg!(debug_assertions) {
            "debug"
        } else {
            "info"
        })
    });

    if cfg!(debug_assertions) {
        let stdout_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stdout_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .init();
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let rt_handle = rt.handle().clone();

    let config: SclConfig = Clapfig::builder()
        .app_name("scl3")
        .file_name("scl3.toml")
        .search_paths(vec![
            SearchPath::Platform,
            SearchPath::Home(".scl3"),
            SearchPath::Cwd,
        ])
        .persist_scope("local", SearchPath::Cwd)
        .load()?;

    let config = Arc::new(Mutex::new(config));

    {
        let cfg = config.lock().unwrap();
        if !cfg.download.curseforge_api_key.is_empty() {
            unsafe {
                std::env::set_var("CURSEFORGE_API_KEY", &cfg.download.curseforge_api_key);
            }
        }
    }

    let minecraft_dir = config.lock().unwrap().game.resolved_minecraft_path();
    let versions_dir = std::path::Path::new(&minecraft_dir).join("versions");
    let versions = rt.block_on(scl_core::version::get_avaliable_versions(&versions_dir))?;
    info!("找到 {} 个可启动版本:", versions.len());
    for v in &versions {
        info!("  {} ({:?})", v.name, v.version_type);
    }

    let ui = ui::AppWindow::new()?;
    let default_avatar = ui.get_avatar_image();
    {
        let cfg = config.lock().unwrap();
        sync_config_to_ui(&ui, &cfg, &default_avatar);
    }

    let instance_model: slint::ModelRc<slint::SharedString> =
        slint::ModelRc::new(slint::VecModel::from(
            versions
                .iter()
                .map(|v| slint::SharedString::from(v.name.as_str()))
                .collect::<Vec<_>>(),
        ));
    ui.set_instance_model(instance_model);

    {
        let cfg = config.lock().unwrap();
        let saved_instance = cfg.launch.selected_instance.clone();
        if !saved_instance.is_empty() {
            if let Some(idx) = versions.iter().position(|v| v.name == saved_instance) {
                ui.set_selected_instance_index(idx as i32);
            }
        }
    }

    callbacks::register_launch_callback(&ui, config.clone(), rt_handle.clone(), versions.clone());
    callbacks::register_navigation_callbacks(&ui);
    callbacks::register_config_callback(&ui, config.clone());
    callbacks::register_auth_callbacks(&ui, config.clone(), rt_handle.clone());
    callbacks::register_account_callback(&ui, config.clone(), default_avatar.clone());
    callbacks::register_instance_callback(&ui, config.clone());

    ui.run()?;

    {
        let idx = ui.get_selected_instance_index();
        if idx >= 0 && (idx as usize) < versions.len() {
            config.lock().unwrap().launch.selected_instance = versions[idx as usize].name.clone();
        }
    }
    save_config(&config.lock().unwrap())?;
    Ok(())
}
