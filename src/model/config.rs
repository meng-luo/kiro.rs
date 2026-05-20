use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsBackend {
    Rustls,
    NativeTls,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_update_channel")]
    pub channel: String,
    #[serde(default = "default_update_github_repo")]
    pub github_repo: String,
    #[serde(default = "default_update_artifact_name_template")]
    pub artifact_name_template: String,
    #[serde(default = "default_update_download_dir")]
    pub download_dir: String,
    #[serde(default = "default_update_backup_dir")]
    pub backup_dir: String,
    #[serde(default = "default_update_max_backups")]
    pub max_backups: usize,
    #[serde(default = "default_update_healthcheck_url")]
    pub healthcheck_url: String,
    #[serde(default = "default_update_healthcheck_timeout_seconds")]
    pub healthcheck_timeout_seconds: u64,
    #[serde(default)]
    pub restart_command: String,
    #[serde(default)]
    pub update_command: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub allow_prerelease: bool,
    #[serde(default = "default_update_build_type")]
    pub build_type: String,
    #[serde(default = "default_update_deployment_mode")]
    pub deployment_mode: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: default_update_channel(),
            github_repo: default_update_github_repo(),
            artifact_name_template: default_update_artifact_name_template(),
            download_dir: default_update_download_dir(),
            backup_dir: default_update_backup_dir(),
            max_backups: default_update_max_backups(),
            healthcheck_url: default_update_healthcheck_url(),
            healthcheck_timeout_seconds: default_update_healthcheck_timeout_seconds(),
            restart_command: String::new(),
            update_command: String::new(),
            proxy_url: None,
            allow_prerelease: false,
            build_type: default_update_build_type(),
            deployment_mode: default_update_deployment_mode(),
        }
    }
}

impl Default for TlsBackend {
    fn default() -> Self {
        Self::Rustls
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsConfig {
    #[serde(default = "default_diagnostics_enabled")]
    pub enabled: bool,
    #[serde(default = "default_diagnostics_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_diagnostics_retention_hours")]
    pub retention_hours: i64,
    #[serde(default = "default_diagnostics_persist")]
    pub persist: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: default_diagnostics_enabled(),
            max_entries: default_diagnostics_max_entries(),
            retention_hours: default_diagnostics_retention_hours(),
            persist: default_diagnostics_persist(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitCooldownConfig {
    #[serde(default = "default_normal_429_cooldown_seconds")]
    pub normal_429_seconds: i64,
    #[serde(default = "default_suspicious_first_cooldown_seconds")]
    pub suspicious_first_seconds: i64,
    #[serde(default = "default_suspicious_repeated_cooldown_seconds")]
    pub suspicious_repeated_seconds: i64,
    #[serde(default = "default_suspicious_repeat_window_seconds")]
    pub suspicious_repeat_window_seconds: i64,
    #[serde(default = "default_refresh_429_cooldown_seconds")]
    pub refresh_429_seconds: i64,
}

impl Default for RateLimitCooldownConfig {
    fn default() -> Self {
        Self {
            normal_429_seconds: default_normal_429_cooldown_seconds(),
            suspicious_first_seconds: default_suspicious_first_cooldown_seconds(),
            suspicious_repeated_seconds: default_suspicious_repeated_cooldown_seconds(),
            suspicious_repeat_window_seconds: default_suspicious_repeat_window_seconds(),
            refresh_429_seconds: default_refresh_429_cooldown_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdminUiConfig {
    #[serde(default = "default_admin_theme")]
    pub theme: String,
    #[serde(default = "default_accounts_page_size")]
    pub accounts_page_size: usize,
    #[serde(default = "default_records_page_size")]
    pub records_page_size: usize,
}

impl Default for AdminUiConfig {
    fn default() -> Self {
        Self {
            theme: default_admin_theme(),
            accounts_page_size: default_accounts_page_size(),
            records_page_size: default_records_page_size(),
        }
    }
}

/// KNA 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_region")]
    pub region: String,

    /// Auth Region（用于 Token 刷新），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_region: Option<String>,

    /// API Region（用于 API 请求），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,

    #[serde(default = "default_kiro_version")]
    pub kiro_version: String,

    #[serde(default)]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default = "default_system_version")]
    pub system_version: String,

    #[serde(default = "default_node_version")]
    pub node_version: String,

    #[serde(default = "default_tls_backend")]
    pub tls_backend: TlsBackend,

    /// 外部 count_tokens API 地址（可选）
    #[serde(default)]
    pub count_tokens_api_url: Option<String>,

    /// count_tokens API 密钥（可选）
    #[serde(default)]
    pub count_tokens_api_key: Option<String>,

    /// count_tokens API 认证类型（可选，"x-api-key" 或 "bearer"，默认 "x-api-key"）
    #[serde(default = "default_count_tokens_auth_type")]
    pub count_tokens_auth_type: String,

    /// HTTP 代理地址（可选）
    /// 支持格式: http://host:port, https://host:port, socks5://host:port
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// 代理认证用户名（可选）
    #[serde(default)]
    pub proxy_username: Option<String>,

    /// 代理认证密码（可选）
    #[serde(default)]
    pub proxy_password: Option<String>,

    /// Admin API 密钥（可选，启用 Admin API 功能）
    #[serde(default)]
    pub admin_api_key: Option<String>,

    /// Redis 连接 URL（可选，启用 Prompt Cache 展示）
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redis_url: Option<String>,

    /// 负载均衡模式（"priority" 或 "balanced"）
    #[serde(default = "default_load_balancing_mode")]
    pub load_balancing_mode: String,

    /// 是否开启非流式响应的 thinking 块提取（默认 true）
    ///
    /// 启用后，非流式响应中的 `<thinking>...</thinking>` 标签会被解析为
    /// 独立的 `{"type": "thinking", ...}` 内容块,与流式响应行为一致。
    #[serde(default = "default_extract_thinking")]
    pub extract_thinking: bool,

    /// 默认端点名称（凭据未显式指定 endpoint 时使用，默认 "ide"）
    #[serde(default = "default_endpoint")]
    pub default_endpoint: String,

    /// 端点特定的配置
    ///
    /// 键为端点名（如 "ide" / "cli"），值为该端点自由定义的参数对象。
    /// 未在此表出现的端点沿用实现内置默认值。
    #[serde(default)]
    pub endpoints: HashMap<String, serde_json::Value>,

    /// 在线更新 / 回滚配置
    #[serde(default)]
    pub update: UpdateConfig,

    /// 请求诊断配置
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,

    /// Admin UI 配置
    #[serde(default)]
    pub admin_ui: AdminUiConfig,

    /// 限频冷却配置
    #[serde(default)]
    pub rate_limit_cooldown: RateLimitCooldownConfig,

    /// 配置文件路径（运行时元数据，不写入 JSON）
    #[serde(skip)]
    config_path: Option<PathBuf>,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_kiro_version() -> String {
    "0.11.107".to_string()
}

fn default_system_version() -> String {
    const SYSTEM_VERSIONS: &[&str] = &["darwin#24.6.0", "win32#10.0.22631"];
    SYSTEM_VERSIONS[fastrand::usize(..SYSTEM_VERSIONS.len())].to_string()
}

fn default_node_version() -> String {
    "22.22.0".to_string()
}

fn default_count_tokens_auth_type() -> String {
    "x-api-key".to_string()
}

fn default_tls_backend() -> TlsBackend {
    TlsBackend::Rustls
}

fn default_load_balancing_mode() -> String {
    "priority".to_string()
}

fn default_extract_thinking() -> bool {
    true
}

fn default_admin_theme() -> String {
    "system".to_string()
}

fn default_accounts_page_size() -> usize {
    20
}

fn default_records_page_size() -> usize {
    10
}

fn default_endpoint() -> String {
    crate::kiro::endpoint::ide::IDE_ENDPOINT_NAME.to_string()
}

fn default_diagnostics_enabled() -> bool {
    true
}

fn default_diagnostics_max_entries() -> usize {
    20_000
}

fn default_diagnostics_retention_hours() -> i64 {
    72
}

fn default_diagnostics_persist() -> bool {
    true
}

fn default_normal_429_cooldown_seconds() -> i64 {
    5 * 60
}

fn default_suspicious_first_cooldown_seconds() -> i64 {
    15 * 60
}

fn default_suspicious_repeated_cooldown_seconds() -> i64 {
    30 * 60
}

fn default_suspicious_repeat_window_seconds() -> i64 {
    60 * 60
}

fn default_refresh_429_cooldown_seconds() -> i64 {
    5 * 60
}

fn default_update_channel() -> String {
    "stable".to_string()
}

fn default_update_github_repo() -> String {
    "shusfun/kiro.rs".to_string()
}

fn default_update_artifact_name_template() -> String {
    "kiro-rs-{version}-{target}.tar.gz".to_string()
}

fn default_update_download_dir() -> String {
    "./downloads".to_string()
}

fn default_update_backup_dir() -> String {
    "./backups".to_string()
}

fn default_update_max_backups() -> usize {
    5
}

fn default_update_healthcheck_url() -> String {
    "http://127.0.0.1:8991/health".to_string()
}

fn default_update_healthcheck_timeout_seconds() -> u64 {
    30
}

fn default_update_build_type() -> String {
    "release".to_string()
}

fn default_update_deployment_mode() -> String {
    "binary".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            region: default_region(),
            auth_region: None,
            api_region: None,
            kiro_version: default_kiro_version(),
            machine_id: None,
            api_key: None,
            system_version: default_system_version(),
            node_version: default_node_version(),
            tls_backend: default_tls_backend(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            admin_api_key: None,
            redis_url: None,
            load_balancing_mode: default_load_balancing_mode(),
            extract_thinking: default_extract_thinking(),
            default_endpoint: default_endpoint(),
            endpoints: HashMap::new(),
            update: UpdateConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            admin_ui: AdminUiConfig::default(),
            rate_limit_cooldown: RateLimitCooldownConfig::default(),
            config_path: None,
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        "config.json"
    }

    /// 获取有效的 Auth Region（用于 Token 刷新）
    /// 优先使用 auth_region，未配置时回退到 region
    pub fn effective_auth_region(&self) -> &str {
        self.auth_region.as_deref().unwrap_or(&self.region)
    }

    /// 获取有效的 API Region（用于 API 请求）
    /// 优先使用 api_region，未配置时回退到 region
    pub fn effective_api_region(&self) -> &str {
        self.api_region.as_deref().unwrap_or(&self.region)
    }

    /// 从文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            // 配置文件不存在，返回默认配置
            let mut config = Self::default();
            config.config_path = Some(path.to_path_buf());
            return Ok(config);
        }

        let content = fs::read_to_string(path)?;
        let mut config: Config = serde_json::from_str(&content)?;
        config.config_path = Some(path.to_path_buf());
        Ok(config)
    }

    /// 获取配置文件路径（如果有）
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// 将当前配置写回原始配置文件
    pub fn save(&self) -> anyhow::Result<()> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("配置文件路径未知，无法保存配置"))?;

        let content = serde_json::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, content)
            .with_context(|| format!("写入配置文件失败: {}", path.display()))?;
        Ok(())
    }
}
