#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod pages;

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use clapfig::{Clapfig, SearchPath};
use config::{AccountConfig, SclConfig};
use pages::Pages;
use scl_core::auth::microsoft::{DeviceCodeResponse, MicrosoftOAuth, TokenResponse};
use scl_core::auth::structs::AuthMethod;

slint::include_modules!();

#[derive(Debug, Default)]
struct MicrosoftLoginState {
    device_code: Option<DeviceCodeResponse>,
    device_code_result: Option<Result<DeviceCodeResponse, String>>,
    token_result: Option<Result<TokenResponse, String>>,
    auth_result: Option<Result<AuthMethod, String>>,
    working: bool,
}

fn resolve_config_path() -> std::path::PathBuf {
    std::path::PathBuf::from("scl3.toml")
}

fn save_config(config: &SclConfig) -> Result<(), Box<dyn Error>> {
    let path = resolve_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

fn save_ui_config(ui: &AppWindow, config: &Rc<RefCell<SclConfig>>) {
    let mut cfg = config.borrow_mut();
    sync_ui_to_config(ui, &mut cfg);
    if !cfg.download.curseforge_api_key.is_empty() {
        unsafe {
            std::env::set_var("CURSEFORGE_API_KEY", &cfg.download.curseforge_api_key);
        }
    }
    if let Err(e) = save_config(&cfg) {
        eprintln!("保存配置失败: {}", e);
    }
}

fn set_microsoft_error(ui: &AppWindow, status: impl Into<slint::SharedString>) {
    ui.set_auth_microsoft_status(status.into());
    ui.set_auth_microsoft_login_in_progress(false);
}

fn sync_config_to_ui(ui: &AppWindow, config: &SclConfig) {
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

    ui.set_auth_microsoft_client_id(config.auth.microsoft_client_id.clone().into());
}

fn sync_ui_to_config(ui: &AppWindow, config: &mut SclConfig) {
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

    config.auth.microsoft_client_id = ui.get_auth_microsoft_client_id().to_string();
}

fn main() -> Result<(), Box<dyn Error>> {
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

    let config = Rc::new(RefCell::new(config));

    if !config.borrow().download.curseforge_api_key.is_empty() {
        unsafe {
            std::env::set_var(
                "CURSEFORGE_API_KEY",
                &config.borrow().download.curseforge_api_key,
            );
        }
    }

    let minecraft_dir = config.borrow().game.resolved_minecraft_path();
    let versions_dir = std::path::Path::new(&minecraft_dir).join("versions");
    let versions = smol::block_on(scl_core::version::get_avaliable_versions(&versions_dir))?;
    println!("找到 {} 个可启动版本:", versions.len());
    for v in &versions {
        println!("  {} ({:?})", v.name, v.version_type);
    }

    let ui = AppWindow::new()?;
    sync_config_to_ui(&ui, &config.borrow());
    let microsoft_login = Arc::new(Mutex::new(MicrosoftLoginState::default()));

    ui.on_launch_game({
        let ui_weak = ui.as_weak();
        move || {
            println!("[回调] 启动游戏");
            let ui = ui_weak.unwrap();
            ui.set_page_index(10);
            ui.set_download_task_name("正在启动 Minecraft...".into());
            ui.set_download_progress(0.0);
        }
    });

    ui.on_manage_instances({
        move || {
            println!("[回调] 管理实例");
        }
    });

    ui.on_open_download({
        let ui_weak = ui.as_weak();
        move || {
            println!("[回调] 打开下载页面");
            let ui = ui_weak.unwrap();
            ui.set_page_index(10);
            ui.set_download_task_name("正在下载...".into());
            ui.set_download_progress(0.0);
        }
    });

    ui.on_open_settings({
        let ui_weak = ui.as_weak();
        move || {
            println!("[回调] 打开设置");
            let ui = ui_weak.unwrap();
            ui.set_page_index(Pages::Settings as i32);
        }
    });

    ui.on_go_back({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            if ui.get_page_index() == Pages::MicrosoftLogin as i32 {
                ui.set_page_index(Pages::Settings as i32);
            } else {
                ui.set_page_index(Pages::Launcher as i32);
            }
        }
    });

    ui.on_config_changed({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        move || {
            let ui = ui_weak.unwrap();
            save_ui_config(&ui, &config);
        }
    });

    ui.on_open_microsoft_login({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            ui.set_page_index(Pages::MicrosoftLogin as i32);
        }
    });

    ui.on_start_microsoft_login({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let microsoft_login = microsoft_login.clone();
        move || {
            let ui = ui_weak.unwrap();
            save_ui_config(&ui, &config);

            let client_id = ui.get_auth_microsoft_client_id().trim().to_string();
            if client_id.is_empty() {
                set_microsoft_error(&ui, "请先填写 Azure AD Client ID");
                return;
            }

            ui.set_auth_microsoft_verification_uri("".into());
            ui.set_auth_microsoft_user_code("".into());
            ui.set_auth_microsoft_message("".into());
            ui.set_auth_microsoft_status("正在向 Microsoft 请求设备码...".into());
            ui.set_auth_microsoft_login_in_progress(true);
            ui.set_auth_microsoft_can_complete_login(false);

            if let Ok(mut state) = microsoft_login.lock() {
                state.device_code = None;
                state.device_code_result = None;
                state.token_result = None;
                state.auth_result = None;
                state.working = true;
            }

            let state = microsoft_login.clone();
            std::thread::spawn(move || {
                let result = smol::block_on(async {
                    let oauth = MicrosoftOAuth::new(client_id);
                    oauth.get_devicecode().await.map_err(|e| e.to_string())
                });

                if let Ok(mut state) = state.lock() {
                    state.working = false;
                    state.device_code_result = Some(result);
                }
            });
        }
    });

    ui.on_complete_microsoft_login({
        let ui_weak = ui.as_weak();
        let microsoft_login = microsoft_login.clone();
        move || {
            let ui = ui_weak.unwrap();
            let client_id = ui.get_auth_microsoft_client_id().trim().to_string();
            if client_id.is_empty() {
                set_microsoft_error(&ui, "请先填写 Azure AD Client ID");
                return;
            }

            let device_code = {
                let state = microsoft_login.lock();
                match state {
                    Ok(state) => state
                        .device_code
                        .as_ref()
                        .map(|code| code.device_code.clone()),
                    Err(_) => None,
                }
            };

            let Some(device_code) = device_code else {
                set_microsoft_error(&ui, "请先获取认证码");
                return;
            };

            ui.set_auth_microsoft_status("正在验证 Microsoft 登录结果...".into());
            ui.set_auth_microsoft_login_in_progress(true);

            if let Ok(mut state) = microsoft_login.lock() {
                state.token_result = None;
                state.auth_result = None;
                state.working = true;
            }

            let state = microsoft_login.clone();
            std::thread::spawn(move || {
                let result = smol::block_on(async {
                    let oauth = MicrosoftOAuth::new(client_id);
                    let token = oauth
                        .verify_device_code(&device_code)
                        .await
                        .map_err(|e| e.to_string())?;

                    if !token.error.is_empty() {
                        return Err(format!("Microsoft 返回错误: {}", token.error));
                    }

                    let method = oauth
                        .start_auth(token.access_token.as_string(), &token.refresh_token)
                        .await
                        .map_err(|e| e.to_string())?;

                    Ok((token, method))
                });

                if let Ok(mut state) = state.lock() {
                    state.working = false;
                    match result {
                        Ok((token, method)) => {
                            state.token_result = Some(Ok(token));
                            state.auth_result = Some(Ok(method));
                        }
                        Err(err) => {
                            state.auth_result = Some(Err(err));
                        }
                    }
                }
            });
        }
    });

    let login_timer = slint::Timer::default();
    login_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(250),
        {
            let ui_weak = ui.as_weak();
            let config = config.clone();
            let microsoft_login = microsoft_login.clone();
            move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };

                let mut state = match microsoft_login.lock() {
                    Ok(state) => state,
                    Err(_) => {
                        set_microsoft_error(&ui, "微软登录状态锁定失败");
                        return;
                    }
                };

                if let Some(result) = state.device_code_result.take() {
                    match result {
                        Ok(code) => {
                            let message = if code.message.is_empty() {
                                format!(
                                    "请打开 {} 并输入代码 {}",
                                    code.verification_uri, code.user_code
                                )
                            } else {
                                code.message.clone()
                            };

                            ui.set_auth_microsoft_verification_uri(
                                code.verification_uri.clone().into(),
                            );
                            ui.set_auth_microsoft_user_code(code.user_code.clone().into());
                            ui.set_auth_microsoft_message(message.into());
                            ui.set_auth_microsoft_status(
                                "请在浏览器中完成验证，然后点击完成登录".into(),
                            );
                            ui.set_auth_microsoft_login_in_progress(false);
                            ui.set_auth_microsoft_can_complete_login(true);
                            state.device_code = Some(code);
                        }
                        Err(err) => {
                            ui.set_auth_microsoft_can_complete_login(false);
                            set_microsoft_error(&ui, format!("获取认证码失败: {err}"));
                        }
                    }
                }

                if let Some(result) = state.auth_result.take() {
                    match result {
                        Ok(method) => {
                            drop(state);

                            match AccountConfig::save_account(&method, None, None) {
                                Ok(account) => {
                                    {
                                        let mut cfg = config.borrow_mut();
                                        cfg.auth.upsert_account(account);
                                        if let Err(e) = save_config(&cfg) {
                                            set_microsoft_error(&ui, format!("保存配置失败: {e}"));
                                            return;
                                        }
                                    }

                                    if let AuthMethod::Microsoft { player_name, .. } = method {
                                        ui.set_auth_microsoft_status(
                                            format!("登录成功: {player_name}").into(),
                                        );
                                    } else {
                                        ui.set_auth_microsoft_status("登录成功".into());
                                    }
                                    ui.set_auth_microsoft_login_in_progress(false);
                                    ui.set_auth_microsoft_can_complete_login(false);
                                }
                                Err(err) => {
                                    set_microsoft_error(&ui, format!("保存账户密钥失败: {err}"));
                                }
                            }
                        }
                        Err(err) => {
                            ui.set_auth_microsoft_login_in_progress(false);
                            ui.set_auth_microsoft_can_complete_login(true);
                            ui.set_auth_microsoft_status(format!("登录失败: {err}").into());
                        }
                    }
                } else if state.working {
                    ui.set_auth_microsoft_login_in_progress(true);
                }
            }
        },
    );

    ui.on_instance_selected({
        move |value: slint::SharedString| {
            println!("[回调] 选中实例: {}", value);
        }
    });

    ui.run()?;

    save_config(&config.borrow())?;
    Ok(())
}
