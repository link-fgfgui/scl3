use crate::avatar::save_avatar_from_auth_method;
use crate::config::AccountConfig;
use crate::pages::Pages;
use crate::ui_sync::{save_config, save_ui_config, set_microsoft_error, update_account_ui};
use scl_core::auth::microsoft::MicrosoftOAuth;
use scl_core::auth::structs::AuthMethod;
use scl_core::client::{Client, ClientConfig};
use scl_core::download::{
    FabricDownloadExt, ForgeDownloadExt, GameDownload, NeoForgeDownloadExt,
    OptifineDownloadExt, QuiltMCDownloadExt, VanillaDownloadExt,
};
use scl_core::java::{search_for_java, JavaRuntime};
use scl_core::version::Version;
// Slint 生成的 crate::ui 模块含同名 VersionInfo，使用别名避免冲突
use scl_core::version::structs::VersionInfo as LocalVersionInfo;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, error, info, warn};

use slint::{ComponentHandle, Model};
use crate::ui::AppWindow;

static ROUTER: OnceLock<Mutex<Vec<i32>>> = OnceLock::new();

fn get_router() -> &'static Mutex<Vec<i32>> {
    ROUTER.get_or_init(|| Mutex::new(Vec::new()))
}

struct RawVersionGroup {
    title: String,
    items: Vec<(String, String)>,
    expanded: bool,
}

impl RawVersionGroup {
    fn to_version_group(&self) -> crate::ui::VersionGroup {
        crate::ui::VersionGroup {
            title: self.title.clone().into(),
            items: slint::ModelRc::new(slint::VecModel::from(
                self.items
                    .iter()
                    .map(|(id, text)| crate::ui::VersionItem {
                        id: id.clone().into(),
                        text: text.clone().into(),
                    })
                    .collect::<Vec<_>>(),
            )),
            expanded: self.expanded,
        }
    }
}

fn raw_groups_to_model(groups: Vec<RawVersionGroup>) -> slint::ModelRc<crate::ui::VersionGroup> {
    slint::ModelRc::new(slint::VecModel::from(
        groups.iter().map(|g| g.to_version_group()).collect::<Vec<_>>(),
    ))
}

struct DownloaderParams {
    source: scl_core::download::DownloadSource,
    minecraft_dir: String,
    java_path: String,
    verify_data: bool,
    parallel: usize,
    game_independent: bool,
}

impl DownloaderParams {
    fn from_config(config: &Arc<Mutex<crate::config::SclConfig>>) -> Self {
        let cfg = config.lock().unwrap();
        Self {
            source: cfg.download.resolved_source(),
            minecraft_dir: cfg.game.resolved_minecraft_path(),
            java_path: cfg.game.resolved_java_path(),
            verify_data: cfg.download.verify_data,
            parallel: cfg.download.parallel_amount,
            game_independent: cfg.launch.game_independent,
        }
    }

    fn build_downloader(&self) -> scl_core::download::Downloader<()> {
        let mut d = self.build_with_java()
            .with_parallel_amount(self.parallel)
            .with_game_independent(self.game_independent);
        if self.verify_data {
            d = d.with_verify_data();
        }
        d
    }

    fn build_minimal(&self) -> scl_core::download::Downloader<()> {
        scl_core::download::Downloader::<()>::default()
            .with_source(self.source.clone())
            .with_minecraft_path(&self.minecraft_dir)
    }

    fn build_with_java(&self) -> scl_core::download::Downloader<()> {
        self.build_minimal()
            .with_java(self.java_path.clone())
    }
}

pub fn navigate_to(to: i32, ui: &AppWindow) {
    let current = ui.get_page_index();
    if current == to {
        return;
    }
    // 使用局部作用域及时释放锁，防止 UI 渲染时死锁
    {
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
    // 如果栈空了，默认退回到主界面，不至于让界面卡住没反应
    if let Some(back) = target_page {
        ui.set_page_index(back);
    } else {
        ui.set_page_index(Pages::Launcher as i32);
    }
}

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

            ui.set_progress_visible(true);
            ui.set_download_task_name("正在启动 Minecraft...".into());
            ui.set_download_progress(0.0);

            let selected_idx = ui.get_selected_instance_index();
            if selected_idx < 0 || (selected_idx as usize) >= versions.len() {
                let _ = ui_weak.upgrade_in_event_loop(|ui| {
                    ui.set_download_task_name("错误：未选择有效的游戏版本".into());
                    ui.set_progress_visible(false);
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
                    ui.set_progress_visible(false);
                });
                return;
            };

            let mut auth_method = match account.to_auth_method() {
                Ok(m) => m,
                Err(e) => {
                    error!("构建 AuthMethod 失败: {}", e);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        ui.set_download_task_name(format!("错误：{}", e).into());
                        ui.set_progress_visible(false);
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
                (client_id, version_name, minecraft_dir, java_path, launch_cfg)
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
                        ui.set_progress_visible(false);
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
                let mut version_info = LocalVersionInfo {
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
                        ui.set_progress_visible(false);
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
                            ui.set_progress_visible(false);
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
                            ui.set_progress_visible(false);
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
                            ui.set_progress_visible(false);
                        });
                    }
                    Err(e) => {
                        error!("启动游戏失败: {}", e);
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("启动游戏失败: {}", e).into());
                            ui.set_progress_visible(false);
                        });
                    }
                }
            });
        }
    });
}

pub fn register_navigation_callbacks(ui: &AppWindow) {
    ui.on_manage_instances({
        let ui_weak = ui.as_weak();
        move || {
            debug!("[回调] 管理实例");
            let ui = ui_weak.unwrap();
            navigate_to(Pages::DirManage as i32, &ui);
        }
    });

    ui.on_open_download({
        let ui_weak = ui.as_weak();
        move || {
            debug!("[回调] 打开下载页面");
            let ui = ui_weak.unwrap();
            navigate_to(Pages::GameDownload as i32, &ui);
        }
    });

    ui.on_open_settings({
        let ui_weak = ui.as_weak();
        move || {
            debug!("[回调] 打开设置");
            let ui = ui_weak.unwrap();
            navigate_to(Pages::Settings as i32, &ui);
        }
    });

    ui.on_open_login({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            navigate_to(Pages::Login as i32, &ui);
        }
    });

    ui.on_go_back({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            navigate_back(&ui);
        }
    });

    ui.on_open_microsoft_login({
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            navigate_to(Pages::MicrosoftLogin as i32, &ui);
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
                // 直接 await，抛弃 smol 和 spawn_blocking
                let result = scl_core::auth::authlib::start_auth(
                    scl_core::progress::NR,
                    &server,
                    email,
                    scl_core::password::Password::from(password),
                    "",
                )
                .await;

                let result: Result<Vec<AuthMethod>, String> = match result {
                    Ok(methods) => Ok(methods),
                // 因为去掉了 spawn_blocking，少了一层 Result 嵌套
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
                                        let cfg_clone = cfg.clone();
                                        if let Err(e) = save_config(&cfg_clone) {
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
                let cfg_clone = cfg.clone();
                if let Err(e) = save_config(&cfg_clone) {
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
                // 直接在当前的 Tokio 运行时中等待结果
                let oauth = MicrosoftOAuth::new(client_id);
                let result = oauth.get_devicecode().await.map_err(|e| e.to_string());

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
                // 利用 async 块替代 spawn_blocking + smol
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

                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                // 去掉多余的 result match 解包，直接匹配 result
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
                                let cfg_clone = cfg.clone();
                                if let Err(e) = save_config(&cfg_clone) {
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
                let cfg = config.lock().unwrap().clone();
                if let Err(e) = save_config(&cfg) {
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
            let cfg = config.lock().unwrap().clone();
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

pub fn register_download_callbacks(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    ui.on_component_clicked({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        move |component_type: slint::SharedString| {
            let ct = component_type.to_string();
            let ui = ui_weak.unwrap();

            match ct.as_str() {
                "vanilla" => {
                    ui.set_vc_page_title("选择 Minecraft 版本".into());
                    ui.set_vc_show_skip_option(false);
                    ui.set_vc_component_type("vanilla".into());
                    ui.set_vc_selected_id(ui.get_dl_vanilla_selected());
                    navigate_to(Pages::VersionChoose as i32, &ui);

                    let params = DownloaderParams::from_config(&config);
                    let ui_weak = ui_weak.clone();
                    rt_handle.spawn(async move {
                        let downloader = params.build_minimal();
                        let manifest = match downloader.get_avaliable_vanilla_versions().await {
                            Ok(m) => m,
                            Err(e) => {
                                error!("获取原版版本列表失败: {}", e);
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                    ui.set_progress_visible(true);
                                    ui.set_download_task_name(
                                        format!("获取版本列表失败: {}", e).into(),
                                    );
                                });
                                return;
                            }
                        };

                        let mut releases = Vec::new();
                        let mut snapshots = Vec::new();
                        for v in &manifest.versions {
                            if v.version_type == "release" {
                                releases.push((v.id.clone(), v.id.clone()));
                            } else if v.version_type == "snapshot" {
                                snapshots.push((v.id.clone(), v.id.clone()));
                            }
                        }

                        let groups = vec![
                            RawVersionGroup {
                                title: "正式版 (Release)".into(),
                                items: releases,
                                expanded: true,
                            },
                            RawVersionGroup {
                                title: "快照版 (Snapshot)".into(),
                                items: snapshots,
                                expanded: false,
                            },
                        ];

                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_vc_groups(raw_groups_to_model(groups));
                        });
                    });
                }
                "forge" | "neoforge" | "fabric" | "quilt" | "optifine" => {
                    let vanilla_version = ui.get_dl_vanilla_selected().to_string();
                    if vanilla_version.is_empty() {
                        ui.set_progress_visible(true);
                        ui.set_download_task_name("请先选择 Minecraft 版本".into());
                        return;
                    }

                    let title = match ct.as_str() {
                        "forge" => "选择 Forge 版本",
                        "neoforge" => "选择 NeoForge 版本",
                        "fabric" => "选择 Fabric 版本",
                        "quilt" => "选择 QuiltMC 版本",
                        "optifine" => "选择 Optifine 版本",
                        _ => "选择版本",
                    };

                    ui.set_vc_page_title(title.into());
                    ui.set_vc_show_skip_option(true);
                    ui.set_vc_component_type(ct.clone().into());

                    let selected = match ct.as_str() {
                        "forge" => ui.get_dl_forge_selected(),
                        "neoforge" => ui.get_dl_neoforge_selected(),
                        "fabric" => ui.get_dl_fabric_selected(),
                        "quilt" => ui.get_dl_quilt_selected(),
                        "optifine" => ui.get_dl_optifine_selected(),
                        _ => Default::default(),
                    };
                    ui.set_vc_selected_id(selected);
                    navigate_to(Pages::VersionChoose as i32, &ui);

                    let params = DownloaderParams::from_config(&config);
                    let ui_weak = ui_weak.clone();
                    let ct = ct.clone();
                    rt_handle.spawn(async move {
                        let downloader = params.build_with_java();
                        let groups = fetch_loader_groups(&downloader, &ct, &vanilla_version).await;

                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            if ui.get_vc_component_type() != slint::SharedString::from(ct.as_str()) {
                                return;
                            }
                            ui.set_vc_groups(raw_groups_to_model(groups));
                        });
                    });
                }
                _ => {
                    warn!("未知的组件类型: {}", ct);
                }
            }
        }
    });

    ui.on_toggle_group({
        let ui_weak = ui.as_weak();
        move |index: i32| {
            let ui = ui_weak.unwrap();
            let groups = ui.get_vc_groups();
            let mut new_groups: Vec<crate::ui::VersionGroup> = Vec::new();
            for i in 0..groups.row_count() {
                let mut g = groups.row_data(i).unwrap_or_default();
                if i == index as usize {
                    g.expanded = !g.expanded;
                }
                new_groups.push(g);
            }
            ui.set_vc_groups(slint::ModelRc::new(slint::VecModel::from(new_groups)));
        }
    });

    ui.on_version_choose_selected({
        let ui_weak = ui.as_weak();
        move |id: slint::SharedString| {
            let id_str = id.to_string();
            let ui = ui_weak.unwrap();
            let component_type = ui.get_vc_component_type().to_string();
            match component_type.as_str() {
                "vanilla" => {
                    ui.set_dl_vanilla_selected(id_str.clone().into());
                    if ui.get_dl_version_name().is_empty() {
                        ui.set_dl_version_name(id_str.into());
                    }
                }
                "forge" => ui.set_dl_forge_selected(id_str.into()),
                "neoforge" => ui.set_dl_neoforge_selected(id_str.into()),
                "fabric" => ui.set_dl_fabric_selected(id_str.into()),
                "quilt" => ui.set_dl_quilt_selected(id_str.into()),
                "optifine" => ui.set_dl_optifine_selected(id_str.into()),
                _ => {}
            }
            navigate_back(&ui);
        }
    });

    ui.on_install_game({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        move || {
            let ui = ui_weak.unwrap();
            let version_name = ui.get_dl_version_name().to_string();
            if version_name.is_empty() {
                ui.set_progress_visible(true);
                ui.set_download_task_name("请输入版本名称".into());
                return;
            }

            let vanilla_id = ui.get_dl_vanilla_selected().to_string();
            if vanilla_id.is_empty() {
                ui.set_progress_visible(true);
                ui.set_download_task_name("请选择 Minecraft 版本".into());
                return;
            }

            let forge_version = ui.get_dl_forge_selected().to_string();
            let fabric_version = ui.get_dl_fabric_selected().to_string();
            let quilt_version = ui.get_dl_quilt_selected().to_string();
            let neoforge_version = ui.get_dl_neoforge_selected().to_string();
            let optifine_version = ui.get_dl_optifine_selected().to_string();

            let params = DownloaderParams::from_config(&config);

            ui.set_progress_visible(true);
            ui.set_download_task_name(format!("正在安装 {}...", version_name).into());
            ui.set_download_progress(0.0);

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                let downloader = params.build_downloader();

                let manifest = match downloader.get_avaliable_vanilla_versions().await {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(
                                format!("获取版本清单失败: {}", e).into(),
                            );
                            ui.set_progress_visible(false);
                        });
                        return;
                    }
                };

                let vanilla_info = match manifest.versions.iter().find(|v| v.id == vanilla_id) {
                    Some(v) => v.clone(),
                    None => {
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name("找不到指定的原版版本".into());
                            ui.set_progress_visible(false);
                        });
                        return;
                    }
                };

                let version_name_display = version_name.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.set_download_task_name(
                        format!("正在下载 {}...", version_name_display).into(),
                    );
                });

                match downloader
                    .download_game(
                        &version_name,
                        vanilla_info,
                        &fabric_version,
                        &quilt_version,
                        &forge_version,
                        &neoforge_version,
                        &optifine_version,
                    )
                    .await
                {
                    Ok(()) => {
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(
                                format!("{} 安装完成！", version_name).into(),
                            );
                            ui.set_download_progress(1.0);
                            ui.set_progress_visible(false);
                        });
                    }
                    Err(e) => {
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("安装失败: {}", e).into());
                            ui.set_progress_visible(false);
                        });
                    }
                }
            });
        }
    });
}

async fn fetch_loader_groups(
    downloader: &scl_core::download::Downloader<()>,
    component_type: &str,
    vanilla_version: &str,
) -> Vec<RawVersionGroup> {
    match component_type {
        "forge" => match ForgeDownloadExt::get_avaliable_installers(downloader, vanilla_version).await {
            Ok(data) => {
                let mut groups = Vec::new();
                if let Some(rec) = &data.recommended {
                    groups.push(RawVersionGroup {
                        title: "推荐版本".into(),
                        items: vec![(rec.version.clone(), rec.version.clone())],
                        expanded: true,
                    });
                }
                let all_items: Vec<(String, String)> = data
                    .all_versions
                    .iter()
                    .map(|v| (v.version.clone(), v.version.clone()))
                    .collect();
                if !all_items.is_empty() {
                    groups.push(RawVersionGroup {
                        title: "所有版本".into(),
                        items: all_items,
                        expanded: true,
                    });
                }
                if groups.is_empty() {
                    groups.push(RawVersionGroup {
                        title: "无可用版本".into(),
                        items: vec![],
                        expanded: true,
                    });
                }
                groups
            }
            Err(_) => vec![RawVersionGroup {
                title: "获取失败".into(),
                items: vec![],
                expanded: true,
            }],
        },
        "neoforge" => match NeoForgeDownloadExt::get_avaliable_installers(downloader, vanilla_version).await {
            Ok(data) => {
                let mut groups = Vec::new();
                if let Some(latest) = &data.latest {
                    groups.push(RawVersionGroup {
                        title: "最新版本".into(),
                        items: vec![(latest.version.clone(), latest.version.clone())],
                        expanded: true,
                    });
                }
                let all_items: Vec<(String, String)> = data
                    .all_versions
                    .iter()
                    .map(|v| (v.version.clone(), v.version.clone()))
                    .collect();
                if !all_items.is_empty() {
                    groups.push(RawVersionGroup {
                        title: "所有版本".into(),
                        items: all_items,
                        expanded: true,
                    });
                }
                if groups.is_empty() {
                    groups.push(RawVersionGroup {
                        title: "无可用版本".into(),
                        items: vec![],
                        expanded: true,
                    });
                }
                groups
            }
            Err(_) => vec![RawVersionGroup {
                title: "获取失败".into(),
                items: vec![],
                expanded: true,
            }],
        },
        "fabric" => match FabricDownloadExt::get_avaliable_loaders(downloader, vanilla_version).await {
            Ok(loaders) => {
                let items: Vec<(String, String)> = loaders
                    .iter()
                    .map(|l| (l.loader.version.clone(), l.loader.version.clone()))
                    .collect();
                if items.is_empty() {
                    vec![RawVersionGroup {
                        title: "无可用版本".into(),
                        items: vec![],
                        expanded: true,
                    }]
                } else {
                    vec![RawVersionGroup {
                        title: "加载器版本".into(),
                        items,
                        expanded: true,
                    }]
                }
            }
            Err(_) => vec![RawVersionGroup {
                title: "获取失败".into(),
                items: vec![],
                expanded: true,
            }],
        },
        "quilt" => match QuiltMCDownloadExt::get_avaliable_loaders(downloader, vanilla_version).await {
            Ok(loaders) => {
                let items: Vec<(String, String)> = loaders
                    .iter()
                    .map(|l| (l.loader.version.clone(), l.loader.version.clone()))
                    .collect();
                if items.is_empty() {
                    vec![RawVersionGroup {
                        title: "无可用版本".into(),
                        items: vec![],
                        expanded: true,
                    }]
                } else {
                    vec![RawVersionGroup {
                        title: "加载器版本".into(),
                        items,
                        expanded: true,
                    }]
                }
            }
            Err(_) => vec![RawVersionGroup {
                title: "获取失败".into(),
                items: vec![],
                expanded: true,
            }],
        },
        "optifine" => match OptifineDownloadExt::get_avaliable_installers(downloader, vanilla_version).await {
            Ok(versions) => {
                let items: Vec<(String, String)> = versions
                    .iter()
                    .map(|v| {
                        let label = format!("{} {}", v.version_type, v.patch);
                        (label.clone(), label)
                    })
                    .collect();
                if items.is_empty() {
                    vec![RawVersionGroup {
                        title: "无可用版本".into(),
                        items: vec![],
                        expanded: true,
                    }]
                } else {
                    vec![RawVersionGroup {
                        title: "Optifine 版本".into(),
                        items,
                        expanded: true,
                    }]
                }
            }
            Err(_) => vec![RawVersionGroup {
                title: "获取失败".into(),
                items: vec![],
                expanded: true,
            }],
        },
        _ => vec![],
    }
}

pub fn register_mod_callbacks(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    ui.on_search_mods({
        let ui_weak = ui.as_weak();
        let rt_handle = rt_handle.clone();
        move || {
            let ui = ui_weak.unwrap();
            let query = ui.get_mod_search_query().to_string();
            let source_index = ui.get_mod_search_source_index();

            ui.set_mod_search_is_searching(true);

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                let results = if source_index == 0 {
                    match scl_core::download::modrinth::search_mods(
                        scl_core::download::modrinth::SearchParams {
                            search_filter: query,
                            index: 1,
                            page_size: 20,
                        },
                    ).await {
                        Ok(hits) => hits.iter().map(|m| crate::ui::ModItem {
                            project_id: m.project_id.clone().into(),
                            title: m.title.clone().into(),
                            description: m.description.clone().into(),
                            icon_url: m.icon_url.clone().into(),
                        }).collect(),
                        Err(e) => {
                            error!("Modrinth 搜索失败: {}", e);
                            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                ui.set_progress_visible(true);
                                ui.set_download_task_name(format!("Modrinth 搜索失败: {}", e).into());
                            });
                            vec![]
                        }
                    }
                } else {
                    match scl_core::download::curseforge::search_mods(
                        scl_core::download::curseforge::SearchParams {
                            search_filter: query,
                            ..Default::default()
                        },
                    ).await {
                        Ok(hits) => hits.iter().map(|m| crate::ui::ModItem {
                            project_id: m.id.to_string().into(),
                            title: m.name.clone().into(),
                            description: m.summary.clone().into(),
                            icon_url: m.logo.as_ref().map(|l| l.thumbnail_url.clone()).unwrap_or_default().into(),
                        }).collect(),
                        Err(e) => {
                            error!("CurseForge 搜索失败: {}", e);
                            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                ui.set_progress_visible(true);
                                ui.set_download_task_name(format!("CurseForge 搜索失败: {}", e).into());
                            });
                            vec![]
                        }
                    }
                };

                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.set_mod_search_results(slint::ModelRc::new(slint::VecModel::from(results)));
                    ui.set_mod_search_is_searching(false);
                });
            });
        }
    });

    ui.on_mod_selected({
        let ui_weak = ui.as_weak();
        let rt_handle = rt_handle.clone();
        move |project_id: slint::SharedString| {
            let ui = ui_weak.unwrap();
            let pid = project_id.to_string();
            let source_index = ui.get_mod_search_source_index();

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                let (title, description, files) = if source_index == 0 {
                    match scl_core::download::modrinth::get_mod_files(&pid).await {
                        Ok(versions) => {
                            let info = scl_core::download::modrinth::get_mod_info(&pid).await.ok();
                            let title = info.as_ref().map(|i| i.title.clone()).unwrap_or_default();
                            let desc = info.as_ref().map(|i| i.description.clone()).unwrap_or_default();
                            let files: Vec<crate::ui::ModFileItem> = versions.iter().flat_map(|v| {
                                v.files.iter().map(|f| crate::ui::ModFileItem {
                                    filename: f.filename.clone().into(),
                                    game_versions: v.game_versions.join(", ").into(),
                                    loaders: v.loaders.join(", ").into(),
                                    download_url: f.url.clone().into(),
                                    primary: f.primary,
                                })
                            }).collect();
                            (title, desc, files)
                        }
                        Err(e) => {
                            error!("获取 Modrinth 模组文件失败: {}", e);
                            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                ui.set_progress_visible(true);
                                ui.set_download_task_name(format!("获取模组文件失败: {}", e).into());
                            });
                            (String::new(), String::new(), vec![])
                        }
                    }
                } else {
                    if let Ok(mod_id) = pid.parse::<u64>() {
                        match scl_core::download::curseforge::get_mod_files(mod_id).await {
                            Ok(cf_files) => {
                                let info = scl_core::download::curseforge::get_mod_info(mod_id).await.ok();
                                let title = info.as_ref().map(|i| i.name.clone()).unwrap_or_default();
                                let desc = info.as_ref().map(|i| i.summary.clone()).unwrap_or_default();
                                let files: Vec<crate::ui::ModFileItem> = cf_files.iter().map(|f| {
                                    crate::ui::ModFileItem {
                                        filename: f.file_name.clone().into(),
                                        game_versions: f.game_versions.join(", ").into(),
                                        loaders: String::new().into(),
                                        download_url: f.download_url.clone().into(),
                                        primary: false,
                                    }
                                }).collect();
                                (title, desc, files)
                            }
                            Err(e) => {
                                error!("获取 CurseForge 模组文件失败: {}", e);
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                                    ui.set_progress_visible(true);
                                    ui.set_download_task_name(format!("获取模组文件失败: {}", e).into());
                                });
                                (String::new(), String::new(), vec![])
                            }
                        }
                    } else {
                        (String::new(), String::new(), vec![])
                    }
                };

                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.set_mod_detail_title(title.into());
                    ui.set_mod_detail_description(description.into());
                    ui.set_mod_detail_files(slint::ModelRc::new(slint::VecModel::from(files)));
                    navigate_to(Pages::ModDetail as i32, &ui);
                });
            });
        }
    });

    ui.on_mod_download_file({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        let rt_handle = rt_handle.clone();
        move |index: i32| {
            if index < 0 {
                return;
            }
            let ui = ui_weak.unwrap();
            let files = ui.get_mod_detail_files();
            if (index as usize) >= files.row_count() {
                return;
            }
            let file_item = files.row_data(index as usize).unwrap_or_default();
            let url = file_item.download_url.to_string();
            let filename = file_item.filename.to_string();
            let minecraft_dir = config.lock().unwrap().game.resolved_minecraft_path();

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                let mods_dir = std::path::Path::new(&minecraft_dir).join("mods");
                let _ = std::fs::create_dir_all(&mods_dir);
                let dest = mods_dir.join(&filename);
                let filename_display = filename.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.set_progress_visible(true);
                    ui.set_download_task_name(format!("正在下载 {}...", filename_display).into());
                    ui.set_download_progress(0.0);
                });

                match scl_core::http::download(&[&url], dest.to_string_lossy().as_ref(), 0).await {
                    Ok(()) => {
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("{} 下载完成", filename).into());
                            ui.set_download_progress(1.0);
                            ui.set_progress_visible(false);
                        });
                    }
                    Err(e) => {
                        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                            ui.set_download_task_name(format!("下载失败: {}", e).into());
                            ui.set_progress_visible(false);
                        });
                    }
                }
            });
        }
    });

    ui.on_mod_filter_changed({
        let ui_weak = ui.as_weak();
        move || {
            // TODO: 实现客户端文件过滤，按 loader 和 mc_version 筛选 mod-files
            let _ = ui_weak.upgrade_in_event_loop(|_| {});
        }
    });
}

pub fn register_java_callbacks(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
    rt_handle: tokio::runtime::Handle,
) {
    ui.on_search_java({
        let ui_weak = ui.as_weak();
        let rt_handle = rt_handle.clone();
        move || {
            let ui = ui_weak.unwrap();
            ui.set_java_is_searching(true);

            let ui_weak = ui_weak.clone();
            rt_handle.spawn(async move {
                let java_paths = search_for_java().await;
                let mut java_items = Vec::new();

                for path in java_paths {
                    if let Ok(runtime) = JavaRuntime::from_java_path(&path).await {
                        java_items.push(crate::ui::JavaItem {
                            path: runtime.path().into(),
                            version: runtime.version().into(),
                            main_version: runtime.main_version() as i32,
                            is_64bit: runtime.is_64bit(),
                        });
                    }
                }

                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.set_java_list(slint::ModelRc::new(slint::VecModel::from(java_items)));
                    ui.set_java_is_searching(false);
                });
            });
        }
    });

    ui.on_java_selected({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        move |index: i32| {
            if index < 0 {
                return;
            }
            let ui = ui_weak.unwrap();
            let java_list = ui.get_java_list();
            if (index as usize) >= java_list.row_count() {
                return;
            }
            let java_item = java_list.row_data(index as usize).unwrap_or_default();
            let path = java_item.path.to_string();
            {
                let mut cfg = config.lock().unwrap();
                cfg.game.java_path = Some(vec![path.clone()]);
            }
            ui.set_game_java_path(path.into());
            let cfg = config.lock().unwrap().clone();
            if let Err(e) = save_config(&cfg) {
                error!("保存配置失败: {}", e);
            }
        }
    });

    ui.on_add_java_path({
        let ui_weak = ui.as_weak();
        move || {
            // TODO: 弹出文件选择对话框，让用户手动选择 Java 可执行文件
            let _ = ui_weak.upgrade_in_event_loop(|_| {});
        }
    });
}

pub fn register_dir_callbacks(
    ui: &AppWindow,
    config: Arc<Mutex<crate::config::SclConfig>>,
) {
    ui.on_add_dir({
        let ui_weak = ui.as_weak();
        move || {
            // TODO: 弹出目录选择对话框，让用户手动选择 Minecraft 目录
            let _ = ui_weak.upgrade_in_event_loop(|_| {});
        }
    });

    ui.on_remove_dir({
        let ui_weak = ui.as_weak();
        move |index: i32| {
            if index < 0 {
                return;
            }
            let ui = ui_weak.unwrap();
            let dir_list = ui.get_dir_list();
            if (index as usize) >= dir_list.row_count() {
                return;
            }
            // TODO: 从 dir-list 中移除选中项并更新 config
            let _ = ui_weak.upgrade_in_event_loop(|_| {});
        }
    });

    ui.on_dir_selected({
        let ui_weak = ui.as_weak();
        let config = config.clone();
        move |index: i32| {
            if index < 0 {
                return;
            }
            let ui = ui_weak.unwrap();
            let dir_list = ui.get_dir_list();
            if (index as usize) >= dir_list.row_count() {
                return;
            }
            let item = dir_list.row_data(index as usize).unwrap_or_default();
            let path = item.path.to_string();
            {
                let mut cfg = config.lock().unwrap();
                cfg.game.minecraft_path = Some(vec![path.clone()]);
            }
            ui.set_game_minecraft_path(path.into());
            let cfg = config.lock().unwrap().clone();
            if let Err(e) = save_config(&cfg) {
                error!("保存配置失败: {}", e);
            }
        }
    });
}
