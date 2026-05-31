use crate::avatar::save_avatar_from_auth_method;
use crate::config::AccountConfig;
use crate::pages::Pages;
use crate::ui_sync::{save_config, save_ui_config, set_microsoft_error, update_account_ui};
use scl_core::auth::microsoft::MicrosoftOAuth;
use scl_core::auth::structs::AuthMethod;
use scl_core::client::{Client, ClientConfig};
use scl_core::java::JavaRuntime;
use scl_core::version::Version;
use scl_core::version::structs::VersionInfo;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, error, info, warn};

// 确保引入 ComponentHandle 接口，它是 as_weak() 能被调用的关键
use slint::ComponentHandle;
use crate::ui::AppWindow;

// ==================== 全局页面路由栈实现 ====================
static ROUTER: OnceLock<Mutex<Vec<i32>>> = OnceLock::new();

fn get_router() -> &'static Mutex<Vec<i32>> {
    ROUTER.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn navigate_to(to: i32, ui: &AppWindow) {
    let current = ui.get_page_index();
    if current == to {
        return;
    }
    {
        // 使用局部作用域及时释放锁，防止 UI 渲染时死锁
        let mut stack = get_router().lock().unwrap();
        stack.push(current);
    }
    ui.set_page_index(to);
}

pub fn navigate_back(ui: &AppWindow) {
    let target_page = {
        let mut stack = get_router().lock().unwrap();
        stack.pop()
    };
    if let Some(back) = target_page {
        ui.set_page_index(back);
    } else {
        // 如果栈空了，默认退回到主界面，不至于让界面卡住没反应
        ui.set_page_index(Pages::Launcher as i32);
    }
}
// ============================================================

pub fn register_launch_callback(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
    versions: Vec<Version>,
) {
    ui.on_launch_game({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        let versions = versions.clone();
        move || {
            debug!("[回调] 启动游戏");
            let ui = ui_weak.unwrap();
            
            // 启动过渡页也可以加入路由追踪
            navigate_to(10, &ui);
            ui.set_download_task_name("正在启动 Minecraft...".into());
            ui.set_download_progress(0.0);

            let selected_idx = ui.get_selected_instance_index();
            if selected_idx < 0 || (selected_idx as usize) >= versions.len() {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("错误：未选择有效的游戏版本".into());
                });
                return;
            }

            let account_config = {
                let cfg = config.lock().unwrap();
                cfg.auth.current_account().cloned()
            };

            let Some(account) = account_config else {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("错误：未选择账户".into());
                });
                return;
            };

            let mut auth_method = match account.to_auth_method() {
                Ok(m) => m,
                Err(e) => {
                    error!("构建 AuthMethod 失败: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.set_download_task_name(format!("错误：{}", e).into());
                    });
                    return;
                }
            };

            let (client_id, version_name, minecraft_dir, java_path, launch_cfg) = {
                let cfg = config.lock().unwrap();
                let client_id = cfg.auth.microsoft_client_id.trim().to_string();
                let version_name = versions[selected_idx as usize].name.clone();
                let minecraft_dir = cfg.game.resolved_minecraft_path();
                let java_path = cfg.game.resolved_java_path();
                let launch_cfg = cfg.launch.clone();
                (
                    client_id,
                    version_name,
                    minecraft_dir,
                    java_path,
                    launch_cfg,
                )
            };

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                let refresh_result = match &auth_method {
                    AuthMethod::Microsoft { .. } => {
                        if client_id.is_empty() {
                            warn!("Microsoft 账户未配置 client_id，跳过令牌刷新");
                            Ok(())
                        } else {
                            let oauth = MicrosoftOAuth::new(client_id.as_str());
                            oauth
                                .refresh_auth(&mut auth_method)
                                .await
                                .map_err(|e| e.to_string())
                        }
                    }
                    AuthMethod::AuthlibInjector { .. } => {
                        match scl_core::auth::authlib::refresh_token(auth_method.clone(), "", false)
                            .await
                        {
                            Ok(refreshed) => {
                                if let AuthMethod::AuthlibInjector { .. } = &refreshed {
                                    if let Some(account_updated) =
                                        AccountConfig::from_auth_method(&refreshed, None)
                                    {
                                        let _ = account_updated.save_secret(
                                            &account.load_secret().unwrap_or_default(),
                                        );
                                    }
                                }
                                auth_method = refreshed;
                                Ok(())
                            }
                            Err(e) => Err(e.to_string()),
                        }
                    }
                    _ => Ok(()),
                };

                if let Err(e) = refresh_result {
                    error!("刷新令牌失败: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.set_download_task_name(format!("刷新令牌失败: {}", e).into());
                    });
                    return;
                }

                if let AuthMethod::Microsoft { refresh_token, .. } = &auth_method {
                    if let Some(account_updated) =
                        AccountConfig::from_auth_method(&auth_method, None)
                    {
                        if let Err(e) = account_updated.save_secret(refresh_token.as_str()) {
                            warn!("回写 refresh_token 到 keyring 失败: {}", e);
                        }
                    }
                }

                let versions_dir = std::path::Path::new(&minecraft_dir).join("versions");
                let mut version_info = VersionInfo {
                    version_base: versions_dir.to_string_lossy().to_string(),
                    version: version_name.clone(),
                    ..Default::default()
                };

                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("正在加载版本信息...".into());
                });

                if let Err(e) = version_info.load().await {
                    error!("加载版本信息失败: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.set_download_task_name(format!("加载版本信息失败: {}", e).into());
                    });
                    return;
                }

                let resolved_max_mem = if launch_cfg.max_mem > 0 {
                    launch_cfg.max_mem
                } else {
                    let auto_mem = version_info.get_automated_maxium_memory().await;
                    debug!("自动分配最大内存: {}MB", auto_mem);
                    auto_mem as u32
                };

                let scl_launch = scl_core::version::structs::SCLLaunchConfig {
                    max_mem: Some(resolved_max_mem as usize),
                    java_path: java_path.clone(),
                    game_independent: launch_cfg.game_independent,
                    window_title: launch_cfg.window_title,
                    jvm_args: launch_cfg.jvm_args,
                    game_args: launch_cfg.game_args,
                    wrapper_path: launch_cfg.wrapper_path,
                    wrapper_args: launch_cfg.wrapper_args,
                };
                version_info.scl_launch_config = Some(scl_launch);

                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("正在检测 Java...".into());
                });

                let java_runtime = match JavaRuntime::from_java_path(&java_path).await {
                    Ok(jr) => jr,
                    Err(e) => {
                        error!("检测 Java 失败: {}", e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("检测 Java 失败: {}", e).into());
                        });
                        return;
                    }
                };

                let version_type = format!("{:?}", version_info.guess_version_type());

                let client_cfg = ClientConfig {
                    auth: auth_method,
                    version_info,
                    version_type,
                    custom_java_args: Vec::new(),
                    custom_args: Vec::new(),
                    java_runtime,
                    max_mem: resolved_max_mem,
                    recheck: launch_cfg.recheck,
                };

                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("正在组装启动参数...".into());
                });

                let mut client = match Client::new(client_cfg).await {
                    Ok(c) => c,
                    Err(e) => {
                        error!("组装启动参数失败: {}", e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("组装启动参数失败: {}", e).into());
                        });
                        return;
                    }
                };

                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("正在启动游戏...".into());
                    ui.set_download_progress(1.0);
                });

                match client.launch().await {
                    Ok(pid) => {
                        info!("游戏已启动，PID: {}", pid);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("游戏已启动 (PID: {})", pid).into());
                        });
                    }
                    Err(e) => {
                        error!("启动游戏失败: {}", e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("启动游戏失败: {}", e).into());
                        });
                    }
                }
            });
        }
    });
}

pub fn register_navigation_callbacks(ui: &AppWindow) {
    ui.on_manage_instances({
        move || {
            debug!("[回调] 管理实例");
        }
    });

    ui.on_open_download({
        let ui_weak = ui.as_weak();
        move || {
            debug!("[回调] 打开下载页面");
            let ui = ui_weak.unwrap();
            navigate_to(10, &ui); // 👉 使用路由栈管理
            ui.set_download_task_name("正在下载...".into());
            ui.set_download_progress(0.0);
        }
    });

    ui.on_open_settings({
        let ui_weak = ui.as_weak();
        move || {
            debug!("[回调] 打开设置");
            let ui = ui_weak.unwrap();
            navigate_to(Pages::Settings as i32, &ui); // 👉 使用路由栈管理
        }
    });

    ui.on_open_login({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            navigate_to(Pages::Login as i32, &ui); // 👉 使用路由栈管理
        }
    });

    ui.on_go_back({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            navigate_back(&ui); // 👉 抛弃原来硬编码的跳转，完美调用历史后退功能！
        }
    });

    ui.on_open_microsoft_login({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            navigate_to(Pages::MicrosoftLogin as i32, &ui); // 👉 使用路由栈管理
        }
    });
}

pub fn register_config_callback(ui: &AppWindow, config: Arc<Mutex<crate::config::SclConfig>>) {
    ui.on_config_changed({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        move || {
            let ui = ui_weak.unwrap();
            save_ui_config(&ui, &config);
        }
    });
}

pub fn register_auth_callbacks(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    register_external_login_callback(ui, config.clone(), rt_handle.clone());
    register_offline_login_callback(ui, config.clone());
    register_microsoft_login_callback(ui, config.clone(), rt_handle.clone());
    register_complete_microsoft_login_callback(ui, config, rt_handle);
}

fn register_external_login_callback(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    ui.on_start_external_login({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        move |server: slint::SharedString,
              email: slint::SharedString,
              password: slint::SharedString| {
            let server = server.to_string();
            let email = email.to_string();
            let password = password.to_string();

            if server.is_empty() {
                let ui = ui_weak.unwrap();
                ui.set_login_status("请输入认证服务器地址".into());
                return;
            }
            if email.is_empty() {
                let ui = ui_weak.unwrap();
                ui.set_login_status("请输入邮箱地址".into());
                return;
            }
            if password.is_empty() {
                let ui = ui_weak.unwrap();
                ui.set_login_status("请输入密码".into());
                return;
            }

            {
                let ui = ui_weak.unwrap();
                ui.set_login_status("正在登录...".into());
                ui.set_login_in_progress(true);
            }

            let ui_weak = ui_weak.clone();
            let config = config.clone();
            let email_for_save = email.clone();
            let password_for_save = password.clone();
            rt_handle.spawn(async move {
                // 👉 直接 await，抛弃 smol 和 spawn_blocking
                let result = scl_core::auth::authlib::start_auth(
                    scl_core::progress::NR,
                    &server,
                    email,
                    scl_core::password::Password::from(password),
                    "",
                )
                .await;

                // 因为去掉了 spawn_blocking，少了一层 Result 嵌套
                let result: Result<Vec<AuthMethod>, String> = match result {
                    Ok(methods) => Ok(methods),
                    Err(e) => Err(e.to_string()),
                };
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.set_login_in_progress(false);

                    match result {
                        Ok(methods) => {
                            let method = match methods.first() {
                                Some(m) => m.clone(),
                                None => {
                                    ui.set_login_status("该账户没有可用的角色".into());
                                    return;
                                }
                            };

                            let player_name = match &method {
                                AuthMethod::AuthlibInjector { player_name, .. } => {
                                    player_name.clone()
                                }
                                _ => String::new(),
                            };

                            save_avatar_from_auth_method(&method);

                            match AccountConfig::save_account(
                                &method,
                                Some(&email_for_save),
                                Some(&password_for_save),
                            ) {
                                Ok(account) => {
                                    {
                                        let mut cfg = config.lock().unwrap();
                                        cfg.auth.upsert_account(account);
                                        let new_idx = cfg
                                            .auth
                                            .accounts
                                            .as_ref()
                                            .map_or(0, |a| a.len().saturating_sub(1));
                                        cfg.auth.selected_account_index = new_idx;
                                        if let Err(e) = save_config(&cfg) {
                                            ui.set_login_status(
                                                format!("保存配置失败: {e}").into(),
                                            );
                                            return;
                                        }
                                    }

                                    {
                                        let cfg = config.lock().unwrap();
                                        ui.set_account_count(cfg.auth.account_count() as i32);
                                        ui.set_current_account_index(
                                            cfg.auth.selected_account_index as i32,
                                        );
                                        let default_avatar = ui.get_avatar_image();
                                        update_account_ui(
                                            &ui,
                                            cfg.auth.current_account(),
                                            &default_avatar,
                                        );
                                    }

                                    ui.set_login_status(format!("登录成功: {player_name}").into());

                                    let ui_weak = ui.as_weak();
                                    std::thread::spawn(move || {
                                        std::thread::sleep(std::time::Duration::from_secs(1));
                                        let _ = ui_weak.upgrade_in_event_loop(|ui| {
                                            // 登录成功属于自动重定向，直接设置主页
                                            ui.set_page_index(Pages::Launcher as i32);
                                        });
                                    });
                                }
                                Err(err) => {
                                    ui.set_login_status(format!("保存账户失败: {err}").into());
                                }
                            }
                        }
                        Err(err) => {
                            ui.set_login_status(format!("登录失败: {err}").into());
                        }
                    }
                });
            });
        }
    });
}

fn register_offline_login_callback(ui: &AppWindow, config: Arc<Mutex<crate::config::SclConfig>>) {
    ui.on_start_offline_login({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        move |username: slint::SharedString| {
            let username = username.to_string();

            if username.is_empty() {
                let ui = ui_weak.unwrap();
                ui.set_login_status("请输入玩家名称".into());
                return;
            }

            let uuid = format!("{:x}", scl_core::auth::generate_offline_uuid(&username));
            let account = AccountConfig::Offline {
                player_name: username.clone(),
                uuid,
            };

            {
                let mut cfg = config.lock().unwrap();
                cfg.auth.upsert_account(account);
                let new_idx = cfg
                    .auth
                    .accounts
                    .as_ref()
                    .map_or(0, |a| a.len().saturating_sub(1));
                cfg.auth.selected_account_index = new_idx;
                if let Err(e) = save_config(&cfg) {
                    let ui = ui_weak.unwrap();
                    ui.set_login_status(format!("保存配置失败: {e}").into());
                    return;
                }
            }

            {
                let ui = ui_weak.unwrap();
                let cfg = config.lock().unwrap();
                ui.set_account_count(cfg.auth.account_count() as i32);
                ui.set_current_account_index(cfg.auth.selected_account_index as i32);
                let default_avatar = ui.get_avatar_image();
                update_account_ui(&ui, cfg.auth.current_account(), &default_avatar);
            }

            {
                let ui = ui_weak.unwrap();
                ui.set_login_status(format!("登录成功: {username}").into());
                let ui_weak = ui_weak.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let _ = ui_weak.upgrade_in_event_loop(|ui| {
                        ui.set_page_index(Pages::Launcher as i32);
                    });
                });
            }
        }
    });
}

fn register_microsoft_login_callback(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    ui.on_start_microsoft_login({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        move || {
            let ui = ui_weak.unwrap();
            save_ui_config(&ui, &config);

            let client_id = config
                .lock()
                .unwrap()
                .auth
                .microsoft_client_id
                .trim()
                .to_string();
            if client_id.is_empty() {
                set_microsoft_error(
                    &ui,
                    "配置中未设置 Azure AD Client ID，请在 scl3.toml 中配置",
                );
                return;
            }

            ui.set_auth_microsoft_verification_uri("".into());
            ui.set_auth_microsoft_user_code("".into());
            ui.set_auth_microsoft_message("".into());
            ui.set_auth_microsoft_status("正在向 Microsoft 请求设备码...".into());
            ui.set_auth_microsoft_login_in_progress(true);
            ui.set_auth_microsoft_can_complete_login(false);

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                // 👉 直接在当前的 Tokio 运行时中等待结果
                let oauth = MicrosoftOAuth::new(client_id);
                let result = oauth.get_devicecode().await.map_err(|e| e.to_string());
                
                // 去掉了 Result 嵌套解包，直接匹配 result
                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
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
                        ui.set_auth_microsoft_device_code(code.device_code.clone().into());
                    }
                    Err(err) => {
                        ui.set_auth_microsoft_can_complete_login(false);
                        set_microsoft_error(&ui, format!("获取认证码失败: {err}"));
                    }
                });
            });
        }
    });
}

fn register_complete_microsoft_login_callback(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    ui.on_complete_microsoft_login({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        move || {
            let ui = ui_weak.unwrap();
            let client_id = config
                .lock()
                .unwrap()
                .auth
                .microsoft_client_id
                .trim()
                .to_string();
            if client_id.is_empty() {
                set_microsoft_error(
                    &ui,
                    "配置中未设置 Azure AD Client ID，请在 scl3.toml 中配置",
                );
                return;
            }

            let device_code_str = ui.get_auth_microsoft_device_code().to_string();
            if device_code_str.is_empty() {
                set_microsoft_error(&ui, "请先获取认证码");
                return;
            }

            ui.set_auth_microsoft_status("正在验证 Microsoft 登录结果...".into());
            ui.set_auth_microsoft_login_in_progress(true);

            let ui_weak = ui_weak.clone();
            let config = config.clone();
            let delay_handle = rt_handle.clone();
            rt_handle.spawn(async move {
                // 👉 利用 async 块替代 spawn_blocking + smol
                let result = async {
                    let oauth = MicrosoftOAuth::new(client_id);
                    let token = oauth
                        .verify_device_code(&device_code_str)
                        .await
                        .map_err(|e| e.to_string())?;

                    if !token.error.is_empty() {
                        return Err(format!("Microsoft 返回错误: {}", token.error));
                    }

                    let method = oauth
                        .start_auth(token.access_token.as_string(), &token.refresh_token)
                        .await
                        .map_err(|e| e.to_string())?;

                    Ok::<_, String>(method)
                }.await;

                // 去掉多余的 result match 解包，因为直接返回的就是 Result<AuthMethod, String>
                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                    Ok(method) => match AccountConfig::save_account(&method, None, None) {
                        Ok(account) => {
                            save_avatar_from_auth_method(&method);

                            {
                                let mut cfg = config.lock().unwrap();
                                cfg.auth.upsert_account(account);
                                let new_idx = cfg
                                    .auth
                                    .accounts
                                    .as_ref()
                                    .map_or(0, |a| a.len().saturating_sub(1));
                                cfg.auth.selected_account_index = new_idx;
                                if let Err(e) = save_config(&cfg) {
                                    set_microsoft_error(&ui, format!("保存配置失败: {e}"));
                                    return;
                                }
                            }

                            {
                                let cfg = config.lock().unwrap();
                                ui.set_account_count(cfg.auth.account_count() as i32);
                                ui.set_current_account_index(
                                    cfg.auth.selected_account_index as i32,
                                );
                                let default_avatar = ui.get_avatar_image();
                                update_account_ui(&ui, cfg.auth.current_account(), &default_avatar);
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

                            let ui_weak = ui.as_weak();
                            delay_handle.spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                                    ui.set_page_index(Pages::Launcher as i32);
                                });
                            });
                        }
                        Err(err) => {
                            set_microsoft_error(&ui, format!("保存账户密钥失败: {err}"));
                        }
                    },
                    Err(err) => {
                        ui.set_auth_microsoft_login_in_progress(false);
                        ui.set_auth_microsoft_can_complete_login(true);
                        ui.set_auth_microsoft_status(format!("登录失败: {err}").into());
                    }
                });
            });
        }
    });
}

pub fn register_account_callback(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    default_avatar: slint::Image,
) {
    ui.on_account_switched({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let default_avatar = default_avatar.clone();
        move |index: i32| {
            let ui = ui_weak.unwrap();
            let account_count = ui.get_account_count();
            if index == account_count {
                if let Err(e) = save_config(&config.lock().unwrap()) {
                    error!("保存配置失败: {}", e);
                }
                return;
            }
            let idx = index as usize;
            {
                let mut cfg = config.lock().unwrap();
                cfg.auth.selected_account_index = idx;
            }
            {
                let cfg = config.lock().unwrap();
                update_account_ui(&ui, cfg.auth.current_account(), &default_avatar);
            }
            let cfg = config.lock().unwrap();
            if let Err(e) = save_config(&cfg) {
                error!("保存配置失败: {}", e);
            }
        }
    });
}

pub fn register_instance_callback(ui: &AppWindow, config: Arc<Mutex<crate::config::SclConfig>>) {
    ui.on_instance_selected({
        let config = config.clone();
        move |value: slint::SharedString| {
            debug!("[回调] 选中实例: {}", value);
            config.lock().unwrap().launch.selected_instance = value.to_string();
        }
    });
}