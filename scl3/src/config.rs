use anyhow::Result;
use clapfig::Config;
use keyring::Entry;
use scl_core::auth::structs::AuthMethod;
use scl_core::download::DownloadSource;
use scl_core::password::Password;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "scl3";

#[derive(Config, Serialize, Deserialize, Debug)]
pub struct SclConfig {
    #[config(nested)]
    pub game: GameConfig,

    #[config(nested)]
    pub download: DownloadConfig,

    #[config(nested)]
    pub launch: LaunchConfig,

    #[config(nested)]
    pub appearance: AppearanceConfig,

    #[config(nested)]
    pub auth: AuthConfig,
}

#[derive(Config, Serialize, Deserialize, Debug)]
pub struct GameConfig {
    /// Minecraft 游戏主目录路径列表，留空则使用系统默认路径
    pub minecraft_path: Option<Vec<String>>,

    /// 默认 Java 运行时可执行文件路径列表，留空则使用系统默认路径
    pub java_path: Option<Vec<String>>,
}

impl GameConfig {
    pub fn resolved_minecraft_path(&self) -> String {
        self.minecraft_path
            .as_ref()
            .and_then(|v| v.iter().find(|p| !p.is_empty()).cloned())
            .unwrap_or_else(default_minecraft_path)
    }

    pub fn resolved_java_path(&self) -> String {
        self.java_path
            .as_ref()
            .and_then(|v| v.iter().find(|p| !p.is_empty()).cloned())
            .unwrap_or_else(default_java_path)
    }
}

fn default_minecraft_path() -> String {
    #[cfg(target_os = "windows")]
    {
        ".minecraft".into()
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if let Some(dir) = dirs::home_dir() {
            dir.join(".minecraft").to_str().unwrap().to_string()
        } else {
            "~/.minecraft".into()
        }
    }
}

fn default_java_path() -> String {
    #[cfg(windows)]
    {
        "javaw.exe".into()
    }
    #[cfg(not(windows))]
    {
        "java".into()
    }
}

#[derive(Config, Serialize, Deserialize, Debug)]
pub struct DownloadConfig {
    /// 下载源，可选 "Default"、"BMCLAPI" 或自定义镜像源 URL
    #[config(default = "Default")]
    pub source: String,

    /// 下载并发量，0 表示不限制
    #[config(default = 64)]
    pub parallel_amount: usize,

    /// 是否验证已存在文件的 SHA1 完整性
    #[config(default = false)]
    pub verify_data: bool,

    /// CurseForge API Key，留空则从环境变量 CURSEFORGE_API_KEY 读取
    #[config(default = "")]
    pub curseforge_api_key: String,
}

impl DownloadConfig {
    pub fn resolved_source(&self) -> DownloadSource {
        self.source.parse().unwrap_or(DownloadSource::Default)
    }
}

#[derive(Config, Serialize, Deserialize, Debug, Clone)]
pub struct LaunchConfig {
    /// 默认最大内存，单位 MB，0 表示自动分配
    #[config(default = 0)]
    pub max_mem: u32,

    /// 默认是否使用版本独立模式
    #[config(default = true)]
    pub game_independent: bool,

    /// 是否在启动前进行资源及依赖检查
    #[config(default = true)]
    pub recheck: bool,

    /// 默认额外 JVM 参数，将会附加到 Class Path 前面
    #[config(default = "")]
    pub jvm_args: String,

    /// 默认额外游戏参数，将会附加到参数末尾
    #[config(default = "")]
    pub game_args: String,

    /// 默认游戏窗口标题，留空则使用默认标题
    #[config(default = "")]
    pub window_title: String,

    /// 包装器执行文件路径，对于某些 Linux 用户有用，用于指定 Java 前的执行文件
    #[config(default = "")]
    pub wrapper_path: String,

    /// 包装器执行文件参数，将会附加到包装器执行文件后
    #[config(default = "")]
    pub wrapper_args: String,

    #[config(default = "")]
    pub selected_instance: String,
}

#[derive(Config, Serialize, Deserialize, Debug)]
pub struct AppearanceConfig {
    /// 界面语言
    #[config(default = "zh-CN")]
    pub language: String,

    /// 是否在启动时检查更新
    #[config(default = true)]
    pub auto_check_update: bool,
}

#[derive(Config, Serialize, Deserialize, Debug)]
pub struct AuthConfig {
    #[config(default = "")]
    pub microsoft_client_id: String,

    #[config(default = 0)]
    pub selected_account_index: usize,

    pub accounts: Option<Vec<AccountConfig>>,
}

impl AuthConfig {
    pub fn current_account(&self) -> Option<&AccountConfig> {
        self.accounts.as_ref().and_then(|accounts| {
            accounts.get(
                self.selected_account_index
                    .min(accounts.len().saturating_sub(1)),
            )
        })
    }

    pub fn account_count(&self) -> usize {
        self.accounts.as_ref().map_or(0, |a| a.len())
    }

    pub fn upsert_account(&mut self, account: AccountConfig) {
        let accounts = self.accounts.get_or_insert_with(Vec::new);
        if let Some(existing) = accounts
            .iter_mut()
            .find(|existing| existing.same_identity(&account))
        {
            *existing = account;
        } else {
            accounts.push(account);
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum AccountConfig {
    #[serde(rename = "offline")]
    Offline { player_name: String, uuid: String },
    #[serde(rename = "microsoft")]
    Microsoft {
        player_name: String,
        uuid: String,
        xuid: String,
    },
    #[serde(rename = "authlib")]
    AuthlibInjector {
        player_name: String,
        uuid: String,
        api_location: String,
        server_name: String,
        server_homepage: String,
        server_meta: String,
        username: String,
    },
}

impl AccountConfig {
    pub fn player_name(&self) -> &str {
        match self {
            Self::Offline { player_name, .. } => player_name,
            Self::Microsoft { player_name, .. } => player_name,
            Self::AuthlibInjector { player_name, .. } => player_name,
        }
    }

    fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Offline { uuid: a, .. }, Self::Offline { uuid: b, .. }) => a == b,
            (Self::Microsoft { uuid: a, .. }, Self::Microsoft { uuid: b, .. }) => a == b,
            (
                Self::AuthlibInjector {
                    uuid: a,
                    api_location: api_a,
                    ..
                },
                Self::AuthlibInjector {
                    uuid: b,
                    api_location: api_b,
                    ..
                },
            ) => a == b && api_a == api_b,
            _ => false,
        }
    }

    fn keyring_key(&self) -> Option<String> {
        match self {
            Self::Offline { .. } => None,
            Self::Microsoft { uuid, .. } => Some(format!("microsoft:{uuid}")),
            Self::AuthlibInjector { uuid, .. } => Some(format!("authlib:{uuid}")),
        }
    }

    pub fn save_secret(&self, secret: &str) -> Result<()> {
        if let Some(key) = self.keyring_key() {
            let entry = Entry::new(KEYRING_SERVICE, &key)?;
            entry.set_password(secret)?;
        }
        Ok(())
    }

    pub fn load_secret(&self) -> Option<String> {
        let key = self.keyring_key()?;
        let entry = Entry::new(KEYRING_SERVICE, &key).ok()?;
        entry.get_password().ok()
    }

    pub fn delete_secret(&self) {
        if let Some(key) = self.keyring_key() {
            if let Ok(entry) = Entry::new(KEYRING_SERVICE, &key) {
                let _ = entry.delete_credential();
            }
        }
    }

    pub fn to_auth_method(&self) -> Result<AuthMethod> {
        match self {
            Self::Offline { player_name, uuid } => Ok(AuthMethod::Offline {
                player_name: player_name.clone(),
                uuid: uuid.clone(),
            }),
            Self::Microsoft {
                player_name,
                uuid,
                xuid,
            } => {
                let refresh_token = self.load_secret().ok_or_else(|| {
                    anyhow::anyhow!("无法从 keyring 读取 Microsoft refresh_token，请重新登录")
                })?;
                Ok(AuthMethod::Microsoft {
                    access_token: Password::default(),
                    refresh_token: refresh_token.into(),
                    uuid: uuid.clone(),
                    xuid: xuid.clone(),
                    player_name: player_name.clone(),
                    head_skin: Vec::new(),
                    hat_skin: Vec::new(),
                })
            }
            Self::AuthlibInjector {
                player_name,
                uuid,
                api_location,
                server_name,
                server_homepage,
                server_meta,
                username: _,
            } => {
                let _password = self.load_secret().ok_or_else(|| {
                    anyhow::anyhow!("无法从 keyring 读取 Authlib 密码，请重新登录")
                })?;
                Ok(AuthMethod::AuthlibInjector {
                    api_location: api_location.clone(),
                    server_name: server_name.clone(),
                    server_homepage: server_homepage.clone(),
                    server_meta: server_meta.clone(),
                    access_token: Password::default(),
                    uuid: uuid.clone(),
                    player_name: player_name.clone(),
                    head_skin: Vec::new(),
                    hat_skin: Vec::new(),
                })
            }
        }
    }

    pub fn from_auth_method(method: &AuthMethod, authlib_username: Option<&str>) -> Option<Self> {
        match method {
            AuthMethod::Offline { player_name, uuid } => Some(Self::Offline {
                player_name: player_name.clone(),
                uuid: uuid.clone(),
            }),
            AuthMethod::Microsoft {
                player_name,
                uuid,
                xuid,
                ..
            } => Some(Self::Microsoft {
                player_name: player_name.clone(),
                uuid: uuid.clone(),
                xuid: xuid.clone(),
            }),
            AuthMethod::AuthlibInjector {
                player_name,
                uuid,
                api_location,
                server_name,
                server_homepage,
                server_meta,
                ..
            } => Some(Self::AuthlibInjector {
                player_name: player_name.clone(),
                uuid: uuid.clone(),
                api_location: api_location.clone(),
                server_name: server_name.clone(),
                server_homepage: server_homepage.clone(),
                server_meta: server_meta.clone(),
                username: authlib_username?.to_string(),
            }),
            _ => None,
        }
    }

    pub fn save_account(
        method: &AuthMethod,
        authlib_username: Option<&str>,
        authlib_password: Option<&str>,
    ) -> Result<Self> {
        let account = Self::from_auth_method(method, authlib_username)
            .ok_or_else(|| anyhow::anyhow!("不支持的登录方式"))?;
        match method {
            AuthMethod::Microsoft { refresh_token, .. } => {
                account.save_secret(refresh_token.as_str())?;
            }
            AuthMethod::AuthlibInjector { .. } => {
                let password =
                    authlib_password.ok_or_else(|| anyhow::anyhow!("外置登录需要提供密码"))?;
                account.save_secret(password)?;
            }
            _ => {}
        }
        Ok(account)
    }
}
