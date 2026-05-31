use std::error::Error;
use std::sync::{Arc, Mutex};

use crate::avatar::load_avatar_image;
use crate::config::{AccountConfig, SclConfig};
use tracing::error;

use slint::ComponentHandle;
use crate::ui::AppWindow;

pub fn resolve_config_path() -> std::path::PathBuf {
    std::path::PathBuf::from("scl3.toml")
}

pub fn save_config(config: &SclConfig) -> Result<(), Box<dyn Error>> {
    let path = resolve_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

pub fn save_ui_config(ui: &AppWindow, config: &Arc<Mutex<SclConfig>>) {
    let mut cfg = config.lock().unwrap();
    sync_ui_to_config(ui, &mut cfg);
    if !cfg.download.curseforge_api_key.is_empty() {
        unsafe {
            std::env::set_var("CURSEFORGE_API_KEY", &cfg.download.curseforge_api_key);
        }
    }
    if let Err(e) = save_config(&cfg) {
        error!("保存配置失败: {}", e);
    }
}

pub fn set_microsoft_error(ui: &AppWindow, status: impl Into<slint::SharedString>) {
    ui.set_auth_microsoft_status(status.into());
    ui.set_auth_microsoft_login_in_progress(false);
}

pub fn update_account_ui(
    ui: &AppWindow,
    account: Option<&AccountConfig>,
    default_avatar: &slint::Image,
) {
    match account {
        Some(acct) => {
            ui.set_player_name(acct.player_name().into());
            let (login_type, uuid): (i32, Option<&str>) = match acct {
                AccountConfig::Offline { .. } => (0, None),
                AccountConfig::Microsoft { uuid, .. } => (1, Some(uuid.as_str())),
                AccountConfig::AuthlibInjector { uuid, .. } => (2, Some(uuid.as_str())),
            };
            ui.set_login_method_type(login_type);
            if let Some(uuid) = uuid {
                if let Some(avatar) = load_avatar_image(uuid) {
                    ui.set_avatar_image(avatar);
                } else {
                    ui.set_avatar_image(default_avatar.clone());
                }
            } else {
                ui.set_avatar_image(default_avatar.clone());
            }
        }
        None => {
            ui.set_player_name("Player".into());
            ui.set_login_method_type(0);
            ui.set_avatar_image(default_avatar.clone());
        }
    }
}

pub fn sync_config_to_ui(ui: &AppWindow, config: &SclConfig, default_avatar: &slint::Image) {
    ui.set_game_minecraft_path(config.game.resolved_minecraft_path().into());
    ui.set_game_java_path(config.game.resolved_java_path().into());

    let source_index = match config.download.source.as_str() {
        "BMCLAPI" => 1,
        _ => 0,
    };
    ui.set_download_source_index(source_index);
    ui.set_download_parallel_amount(config.download.parallel_amount as i32);
    ui.set_download_verify_data(config.download.verify_data);
    ui.set_download_curseforge_api_key(config.download.curseforge_api_key.clone().into());

    ui.set_launch_max_mem(config.launch.max_mem as i32);
    ui.set_launch_game_independent(config.launch.game_independent);
    ui.set_launch_recheck(config.launch.recheck);
    ui.set_launch_jvm_args(config.launch.jvm_args.clone().into());
    ui.set_launch_game_args(config.launch.game_args.clone().into());
    ui.set_launch_window_title(config.launch.window_title.clone().into());
    ui.set_launch_wrapper_path(config.launch.wrapper_path.clone().into());
    ui.set_launch_wrapper_args(config.launch.wrapper_args.clone().into());

    let lang_index = match config.appearance.language.as_str() {
        "en-US" => 1,
        _ => 0,
    };
    ui.set_appearance_language_index(lang_index);
    ui.set_appearance_auto_check_update(config.appearance.auto_check_update);

    ui.set_account_count(config.auth.account_count() as i32);
    let selected = if config.auth.account_count() == 0 {
        0
    } else {
        config
            .auth
            .selected_account_index
            .min(config.auth.account_count().saturating_sub(1)) as i32
    };
    ui.set_current_account_index(selected);
    update_account_ui(ui, config.auth.current_account(), default_avatar);
}

pub fn sync_ui_to_config(ui: &AppWindow, config: &mut SclConfig) {
    let minecraft_path = ui.get_game_minecraft_path().to_string();
    config.game.minecraft_path = if minecraft_path.is_empty() {
        None
    } else {
        Some(vec![minecraft_path])
    };

    let java_path = ui.get_game_java_path().to_string();
    config.game.java_path = if java_path.is_empty() {
        None
    } else {
        Some(vec![java_path])
    };

    let source_index = ui.get_download_source_index();
    config.download.source = match source_index {
        1 => "BMCLAPI".to_string(),
        _ => "Default".to_string(),
    };
    config.download.parallel_amount = ui.get_download_parallel_amount() as usize;
    config.download.verify_data = ui.get_download_verify_data();
    config.download.curseforge_api_key = ui.get_download_curseforge_api_key().to_string();

    config.launch.max_mem = ui.get_launch_max_mem() as u32;
    config.launch.game_independent = ui.get_launch_game_independent();
    config.launch.recheck = ui.get_launch_recheck();
    config.launch.jvm_args = ui.get_launch_jvm_args().to_string();
    config.launch.game_args = ui.get_launch_game_args().to_string();
    config.launch.window_title = ui.get_launch_window_title().to_string();
    config.launch.wrapper_path = ui.get_launch_wrapper_path().to_string();
    config.launch.wrapper_args = ui.get_launch_wrapper_args().to_string();

    let lang_index = ui.get_appearance_language_index();
    config.appearance.language = match lang_index {
        1 => "en-US".to_string(),
        _ => "zh-CN".to_string(),
    };
    config.appearance.auto_check_update = ui.get_appearance_auto_check_update();
}
