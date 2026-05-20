//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use flate2::read::GzDecoder;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::Archive;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::anthropic::cache::PromptCacheManager;
use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::diagnostics::{
    DiagnosticsQuery, DiagnosticsRequestsResponse, DiagnosticsSummaryResponse,
    RequestDiagnosticEntry,
};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::events::Event;
use crate::kiro::model::requests::conversation::{
    ConversationState, CurrentMessage, UserInputMessage,
};
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::kiro::provider::KiroProvider;
use crate::kiro::token_manager::{AcquireOptions, MultiTokenManager};
use crate::model::config::{AdminUiConfig, Config};

use super::error::AdminServiceError;
use super::proxy_store::{ProxyItem, ProxyListItem, ProxyStore};
use super::types::{
    AddCredentialRequest, AddCredentialResponse, AdminSettingsRequest, AdminSettingsResponse,
    BalanceResponse, BatchBalanceResponse, BatchCredentialUpdateRequest, BatchDisabledRequest,
    BatchIdsRequest, BatchOperationResponse, CachedBalanceStatus, CredentialStatusItem,
    CredentialTestRequest, CredentialsStatusResponse, DiagnosticsCliResponse,
    DiagnosticsQueryRequest, LoadBalancingModeResponse, PromptCacheConfigRequest,
    PromptCacheConfigResponse, ProxyListResponse, ProxyUpsertRequest, SetLoadBalancingModeRequest,
    SetMaxConcurrentRequest, SystemOperationJobResponse, SystemRollbackRequest,
    SystemUpdateRequest, SystemVersionResponse,
};

pub type TestEventStream =
    std::pin::Pin<Box<dyn Stream<Item = Result<serde_json::Value, AdminServiceError>> + Send>>;

/// 余额缓存过期时间（秒），5 分钟
const BALANCE_CACHE_TTL_SECS: i64 = 300;
/// 后台余额刷新周期（秒），低频小批量避免打扰主请求。
const BALANCE_REFRESH_INTERVAL_SECS: u64 = 180;
/// 后台每轮最多刷新账号数。
const BALANCE_REFRESH_BATCH_SIZE: usize = 3;
/// GitHub API 地址
const GITHUB_API_BASE: &str = "https://api.github.com/repos";

#[derive(Debug, Clone)]
struct CachedVersionState {
    response: SystemVersionResponse,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    published_at: Option<String>,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone)]
struct DockerVersionInfo {
    version: String,
    published_at: Option<String>,
    release_notes_url: Option<String>,
    update_available: bool,
}

#[derive(Debug, Clone)]
struct AdminPageSettings {
    accounts_page_size: usize,
    records_page_size: usize,
}

/// 缓存的余额条目（含时间戳）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBalance {
    /// 缓存时间（Unix 秒）
    cached_at: f64,
    /// 缓存的余额数据
    data: BalanceResponse,
}

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
    provider: Arc<KiroProvider>,
    balance_cache: Mutex<HashMap<u64, CachedBalance>>,
    cache_path: Option<PathBuf>,
    /// 已注册的端点名称集合（用于 add_credential 校验）
    known_endpoints: HashSet<String>,
    current_version: String,
    current_commit: Option<String>,
    version_state: Mutex<CachedVersionState>,
    system_jobs: Mutex<HashMap<String, SystemOperationJobResponse>>,
    http_client: Client,
    prompt_cache: Arc<PromptCacheManager>,
    proxy_store: ProxyStore,
    admin_theme: Mutex<String>,
    admin_page_settings: Mutex<AdminPageSettings>,
}

impl AdminService {
    pub fn new(
        token_manager: Arc<MultiTokenManager>,
        provider: Arc<KiroProvider>,
        known_endpoints: impl IntoIterator<Item = String>,
        prompt_cache: Arc<PromptCacheManager>,
    ) -> Self {
        let cache_path = token_manager
            .cache_dir()
            .map(|d| d.join("kiro_balance_cache.json"));
        let proxy_path = token_manager
            .cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kiro_proxies.json");

        let balance_cache = Self::load_balance_cache_from(&cache_path);
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        let current_commit = Self::detect_current_commit();
        let system_job_path = cache_path
            .as_ref()
            .and_then(|path| path.parent().map(|dir| dir.join("kiro_system_job.json")));
        let persisted_latest_job = Self::load_system_job_from(&system_job_path);
        let initial_version_state = CachedVersionState {
            response: Self::build_default_version_response(
                &token_manager.config(),
                current_version.clone(),
                current_commit.clone(),
                persisted_latest_job.clone(),
            ),
        };
        let http_client =
            Self::build_admin_http_client(token_manager.config()).unwrap_or_else(|e| {
                tracing::warn!("构建版本治理 HTTP 客户端失败，回退到默认客户端: {}", e);
                Client::builder().build().expect("创建默认 HTTP 客户端失败")
            });
        let admin_theme = Self::normalize_admin_theme(&token_manager.config().admin_ui.theme);
        let admin_page_settings =
            Self::normalize_admin_page_settings(&token_manager.config().admin_ui);
        let mut system_jobs = HashMap::new();
        if let Some(job) = persisted_latest_job {
            system_jobs.insert(job.job_id.clone(), job);
        }

        Self {
            token_manager,
            provider,
            balance_cache: Mutex::new(balance_cache),
            cache_path,
            known_endpoints: known_endpoints.into_iter().collect(),
            current_version,
            current_commit,
            version_state: Mutex::new(initial_version_state),
            system_jobs: Mutex::new(system_jobs),
            http_client,
            prompt_cache,
            proxy_store: ProxyStore::new(proxy_path),
            admin_theme: Mutex::new(admin_theme),
            admin_page_settings: Mutex::new(admin_page_settings),
        }
    }

    pub fn start_balance_refresh_task(self: &Arc<Self>) {
        if self.cache_path.is_none() {
            return;
        }

        let service = self.clone();
        tokio::spawn(async move {
            let initial_delay = fastrand::u64(15..=60);
            tokio::time::sleep(Duration::from_secs(initial_delay)).await;
            let mut ticker =
                tokio::time::interval(Duration::from_secs(BALANCE_REFRESH_INTERVAL_SECS));

            loop {
                ticker.tick().await;
                service.refresh_random_balance_batch().await;
            }
        });
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();
        let default_endpoint = self.token_manager.config().default_endpoint.clone();
        let balance_lookup = self.cached_balance_lookup();

        let proxy_data = self.proxy_store.load().unwrap_or_default();
        let proxy_lookup: HashMap<u64, ProxyItem> = proxy_data
            .proxies
            .into_iter()
            .map(|proxy| (proxy.id, proxy))
            .collect();

        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| {
                let proxy = entry.proxy_id.and_then(|id| proxy_lookup.get(&id));
                let cached_balance = balance_lookup.get(&entry.id).cloned();
                CredentialStatusItem {
                    id: entry.id,
                    priority: entry.priority,
                    disabled: entry.disabled,
                    failure_count: entry.failure_count,
                    is_current: entry.id == snapshot.current_id,
                    expires_at: entry.expires_at,
                    auth_method: entry.auth_method,
                    has_profile_arn: entry.has_profile_arn,
                    refresh_token_hash: entry.refresh_token_hash,
                    api_key_hash: entry.api_key_hash,
                    masked_api_key: entry.masked_api_key,
                    email: entry.email,
                    subscription_title: entry.subscription_title,
                    cached_balance,
                    success_count: entry.success_count,
                    last_used_at: entry.last_used_at.clone(),
                    has_proxy: entry.has_proxy,
                    proxy_url: entry.proxy_url,
                    proxy_mode: entry.proxy_mode,
                    proxy_id: entry.proxy_id,
                    proxy_name: proxy.map(|item| item.name.clone()),
                    proxy_status: proxy.map(|item| {
                        if item.disabled {
                            "disabled".to_string()
                        } else {
                            item.last_test_status
                                .clone()
                                .unwrap_or_else(|| "unknown".to_string())
                        }
                    }),
                    refresh_failure_count: entry.refresh_failure_count,
                    disabled_reason: entry.disabled_reason,
                    endpoint: entry.endpoint.unwrap_or_else(|| default_endpoint.clone()),
                    dispatch_state: entry.dispatch_state,
                    current_concurrent: entry.current_concurrent,
                    max_concurrent: entry.max_concurrent,
                    cooldown_remaining_ms: entry.cooldown_remaining_ms,
                    last_rate_limit_kind: entry.last_rate_limit_kind,
                    recent_429_count: entry.recent_429_count,
                    recent_suspicious_count: entry.recent_suspicious_count,
                    sticky_session_count: entry.sticky_session_count,
                    sticky_detached: entry.sticky_detached,
                    dispatch_path: entry.dispatch_path,
                    soft_fallback_eligible: entry.soft_fallback_eligible,
                    last_soft_fallback_at: entry.last_soft_fallback_at,
                }
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            enabled_count: snapshot.enabled_count,
            schedulable_count: snapshot.schedulable_count,
            current_id: snapshot.current_id,
            credentials,
        }
    }

    pub fn diagnostics_summary(
        &self,
        query: DiagnosticsQueryRequest,
    ) -> DiagnosticsSummaryResponse {
        let query = Self::build_diagnostics_query(query);
        self.token_manager.diagnostics_summary(&query)
    }

    pub fn diagnostics_requests(
        &self,
        query: DiagnosticsQueryRequest,
    ) -> DiagnosticsRequestsResponse {
        let query = Self::build_diagnostics_query(query);
        self.token_manager.query_diagnostics(&query)
    }

    pub fn diagnostic_request(
        &self,
        request_id: &str,
    ) -> Result<RequestDiagnosticEntry, AdminServiceError> {
        self.token_manager
            .get_diagnostic(request_id)
            .ok_or_else(|| {
                AdminServiceError::InvalidCredential(format!("请求记录不存在: {}", request_id))
            })
    }

    pub fn diagnostics_cli(&self, query: DiagnosticsQueryRequest) -> DiagnosticsCliResponse {
        let mut parts = Vec::new();
        Self::push_query_part(&mut parts, "since", query.since.as_deref());
        Self::push_query_part(&mut parts, "until", query.until.as_deref());
        Self::push_query_part(
            &mut parts,
            "credentialId",
            query
                .credential_id
                .as_ref()
                .map(|v| v.to_string())
                .as_deref(),
        );
        Self::push_query_part(&mut parts, "model", query.model.as_deref());
        Self::push_query_part(
            &mut parts,
            "success",
            query.success.as_ref().map(|v| v.to_string()).as_deref(),
        );
        Self::push_query_part(&mut parts, "keyword", query.keyword.as_deref());
        Self::push_query_part(
            &mut parts,
            "rateLimitKind",
            query.rate_limit_kind.as_deref(),
        );
        Self::push_query_part(
            &mut parts,
            "rateLimitOnly",
            query
                .rate_limit_only
                .as_ref()
                .map(|v| v.to_string())
                .as_deref(),
        );
        Self::push_query_part(&mut parts, "dispatchPath", query.dispatch_path.as_deref());
        Self::push_query_part(
            &mut parts,
            "limit",
            query.limit.as_ref().map(|v| v.to_string()).as_deref(),
        );

        let query_string = if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        };
        DiagnosticsCliResponse {
            command: format!(
                "curl -sS -H \"x-api-key: $ADMIN_API_KEY\" \"http://127.0.0.1:8991/api/admin/diagnostics/requests{}\" | jq",
                query_string
            ),
        }
    }

    fn build_diagnostics_query(query: DiagnosticsQueryRequest) -> DiagnosticsQuery {
        DiagnosticsQuery {
            since: query
                .since
                .as_deref()
                .and_then(Self::parse_diagnostics_time),
            until: query
                .until
                .as_deref()
                .and_then(Self::parse_diagnostics_time),
            credential_id: query.credential_id,
            model: query.model.filter(|value| !value.trim().is_empty()),
            success: query.success,
            keyword: query.keyword.filter(|value| !value.trim().is_empty()),
            rate_limit_only: query.rate_limit_only,
            rate_limit_kind: query
                .rate_limit_kind
                .filter(|value| !value.trim().is_empty()),
            dispatch_path: query.dispatch_path.filter(|value| !value.trim().is_empty()),
            limit: query.limit,
            cursor: query.cursor,
        }
    }

    fn parse_diagnostics_time(value: &str) -> Option<DateTime<Utc>> {
        let trimmed = value.trim();
        if let Some(hours) = trimmed
            .strip_suffix('h')
            .and_then(|v| v.parse::<i64>().ok())
        {
            return Some(Utc::now() - ChronoDuration::hours(hours.max(1)));
        }
        if let Some(minutes) = trimmed
            .strip_suffix('m')
            .and_then(|v| v.parse::<i64>().ok())
        {
            return Some(Utc::now() - ChronoDuration::minutes(minutes.max(1)));
        }
        DateTime::parse_from_rfc3339(trimmed)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    fn push_query_part(parts: &mut Vec<String>, key: &str, value: Option<&str>) {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            return;
        };
        parts.push(format!("{}={}", key, urlencoding::encode(value)));
    }

    /// 设置凭据禁用状态
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), AdminServiceError> {
        // 先获取当前凭据 ID，用于判断是否需要切换
        let snapshot = self.token_manager.snapshot();
        let current_id = snapshot.current_id;

        self.token_manager
            .set_disabled(id, disabled)
            .map_err(|e| self.classify_error(e, id))?;

        // 只有禁用的是当前凭据时才尝试切换到下一个
        if disabled && id == current_id {
            let _ = self.token_manager.switch_to_next();
        }
        Ok(())
    }

    /// 设置凭据优先级
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_priority(id, priority)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 重置失败计数并重新启用
    pub fn reset_and_enable(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .reset_and_enable(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 手动恢复本地运行态阻塞
    pub fn recover_credential(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .recover_runtime_state(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 设置账号并发上限
    pub fn set_max_concurrent(
        &self,
        id: u64,
        payload: SetMaxConcurrentRequest,
    ) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_max_concurrent(id, payload.max_concurrent)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 获取凭据余额（带缓存）
    pub async fn get_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        // 先查缓存
        {
            let cache = self.balance_cache.lock();
            if let Some(cached) = cache.get(&id) {
                let now = Utc::now().timestamp() as f64;
                if (now - cached.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    tracing::debug!("凭据 #{} 余额命中缓存", id);
                    return Ok(cached.data.clone());
                }
            }
        }

        // 缓存未命中或已过期，从上游获取
        let balance = self.fetch_balance(id).await?;

        // 更新缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.insert(
                id,
                CachedBalance {
                    cached_at: Utc::now().timestamp() as f64,
                    data: balance.clone(),
                },
            );
        }
        self.save_balance_cache();

        Ok(balance)
    }

    /// 从上游获取余额（无缓存）
    async fn fetch_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        let usage = self
            .token_manager
            .get_usage_limits_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let current_usage = usage.current_usage();
        let usage_limit = usage.usage_limit();
        let remaining = (usage_limit - current_usage).max(0.0);
        let usage_percentage = if usage_limit > 0.0 {
            (current_usage / usage_limit * 100.0).min(100.0)
        } else {
            0.0
        };

        Ok(BalanceResponse {
            id,
            subscription_title: usage.subscription_title().map(|s| s.to_string()),
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: usage.next_date_reset,
        })
    }

    async fn refresh_random_balance_batch(&self) {
        let snapshot = self.token_manager.snapshot();
        let mut ids = snapshot
            .entries
            .iter()
            .filter(|entry| !entry.disabled)
            .filter(|entry| {
                entry.last_used_at.is_some()
                    || self
                        .balance_cache
                        .lock()
                        .get(&entry.id)
                        .is_none_or(|cached| {
                            let now = Utc::now().timestamp() as f64;
                            (now - cached.cached_at) >= BALANCE_CACHE_TTL_SECS as f64
                        })
            })
            .map(|entry| entry.id)
            .collect::<Vec<_>>();

        if ids.is_empty() {
            return;
        }

        fastrand::shuffle(&mut ids);
        for id in ids.into_iter().take(BALANCE_REFRESH_BATCH_SIZE) {
            match timeout(Duration::from_secs(30), self.fetch_balance(id)).await {
                Ok(Ok(balance)) => {
                    {
                        let mut cache = self.balance_cache.lock();
                        cache.insert(
                            id,
                            CachedBalance {
                                cached_at: Utc::now().timestamp() as f64,
                                data: balance,
                            },
                        );
                    }
                    self.save_balance_cache();
                }
                Ok(Err(error)) => tracing::debug!("后台刷新凭据 #{} 余额失败: {}", id, error),
                Err(_) => tracing::debug!("后台刷新凭据 #{} 余额超时", id),
            }

            tokio::time::sleep(Duration::from_secs(fastrand::u64(2..=8))).await;
        }
    }

    fn cached_balance_lookup(&self) -> HashMap<u64, CachedBalanceStatus> {
        let now = Utc::now().timestamp() as f64;
        let cache = self.balance_cache.lock();
        cache
            .iter()
            .filter_map(|(id, cached)| {
                let cached_at_secs = cached.cached_at as i64;
                let cached_at = DateTime::<Utc>::from_timestamp(cached_at_secs, 0)?;
                Some((
                    *id,
                    CachedBalanceStatus {
                        cached_at: cached_at.to_rfc3339(),
                        fresh: (now - cached.cached_at) < BALANCE_CACHE_TTL_SECS as f64,
                        balance: cached.data.clone(),
                    },
                ))
            })
            .collect()
    }

    /// 添加新凭据
    pub async fn add_credential(
        &self,
        req: AddCredentialRequest,
    ) -> Result<AddCredentialResponse, AdminServiceError> {
        // 校验端点名：未指定则默认合法，指定则必须已注册
        if let Some(ref name) = req.endpoint {
            if !self.known_endpoints.contains(name) {
                let mut known: Vec<&str> =
                    self.known_endpoints.iter().map(|s| s.as_str()).collect();
                known.sort();
                return Err(AdminServiceError::InvalidCredential(format!(
                    "未知端点 \"{}\"，已注册端点: {:?}",
                    name, known
                )));
            }
        }

        // 构建凭据对象
        let email = req.email.clone();
        let new_cred = KiroCredentials {
            id: None,
            access_token: None,
            refresh_token: req.refresh_token,
            profile_arn: None,
            expires_at: None,
            auth_method: Some(req.auth_method),
            client_id: req.client_id,
            client_secret: req.client_secret,
            priority: req.priority,
            max_concurrent: req.max_concurrent,
            region: req.region,
            auth_region: req.auth_region,
            api_region: req.api_region,
            machine_id: req.machine_id,
            email: req.email,
            subscription_title: None, // 将在首次获取使用额度时自动更新
            proxy_url: req.proxy_url,
            proxy_username: req.proxy_username,
            proxy_password: req.proxy_password,
            proxy_mode: None,
            proxy_id: None,
            disabled: false, // 新添加的凭据默认启用
            kiro_api_key: req.kiro_api_key,
            endpoint: req.endpoint,
        };

        // 调用 token_manager 添加凭据
        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| self.classify_add_error(e))?;

        // 主动获取订阅等级，避免首次请求时 Free 账号绕过 Opus 模型过滤
        if let Err(e) = self.token_manager.get_usage_limits_for(credential_id).await {
            tracing::warn!("添加凭据后获取订阅等级失败（不影响凭据添加）: {}", e);
        }

        Ok(AddCredentialResponse {
            success: true,
            message: format!("凭据添加成功，ID: {}", credential_id),
            credential_id,
            email,
        })
    }

    /// 删除凭据
    pub fn delete_credential(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .delete_credential(id)
            .map_err(|e| self.classify_delete_error(e, id))?;

        // 清理已删除凭据的余额缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.remove(&id);
        }
        self.save_balance_cache();

        Ok(())
    }

    /// 获取负载均衡模式
    pub fn get_load_balancing_mode(&self) -> LoadBalancingModeResponse {
        LoadBalancingModeResponse {
            mode: self.token_manager.get_load_balancing_mode(),
        }
    }

    pub fn get_prompt_cache_config(&self) -> PromptCacheConfigResponse {
        let status = self.prompt_cache.status();
        PromptCacheConfigResponse {
            configured: status.configured,
            connected: status.connected,
            redis_url: status.redis_url,
            last_error: status.last_error,
        }
    }

    pub async fn set_prompt_cache_config(
        &self,
        req: PromptCacheConfigRequest,
    ) -> Result<PromptCacheConfigResponse, AdminServiceError> {
        let previous_url = self.prompt_cache.raw_redis_url();
        let requested_url = req
            .redis_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let status = self
            .prompt_cache
            .apply_redis_url(requested_url.clone())
            .await
            .map_err(|e| AdminServiceError::InvalidCredential(format!("Redis 连接失败: {}", e)))?;

        if let Err(error) = self.persist_prompt_cache_url(requested_url.clone()) {
            if let Err(rollback_error) = self.prompt_cache.apply_redis_url(previous_url).await {
                tracing::warn!("回滚 Redis 连接失败: {}", rollback_error);
            }
            return Err(AdminServiceError::InternalError(error.to_string()));
        }

        Ok(PromptCacheConfigResponse {
            configured: status.configured,
            connected: status.connected,
            redis_url: status.redis_url,
            last_error: status.last_error,
        })
    }

    pub fn get_admin_settings(&self) -> AdminSettingsResponse {
        let page_settings = self.current_admin_page_settings();
        AdminSettingsResponse {
            theme: self.current_admin_theme(),
            prompt_cache: self.get_prompt_cache_config(),
            accounts_page_size: page_settings.accounts_page_size,
            records_page_size: page_settings.records_page_size,
        }
    }

    pub async fn set_admin_settings(
        &self,
        req: AdminSettingsRequest,
    ) -> Result<AdminSettingsResponse, AdminServiceError> {
        if let Some(theme) = req.theme.as_deref() {
            if !matches!(theme, "light" | "dark" | "system") {
                return Err(AdminServiceError::InvalidCredential(
                    "theme 必须是 light、dark 或 system".to_string(),
                ));
            }
            self.persist_admin_theme(theme.to_string())?;
        }

        if req.accounts_page_size.is_some() || req.records_page_size.is_some() {
            self.persist_admin_page_settings(req.accounts_page_size, req.records_page_size)?;
        }

        if req.redis_url.is_some() {
            self.set_prompt_cache_config(PromptCacheConfigRequest {
                redis_url: req.redis_url,
            })
            .await?;
        }

        Ok(self.get_admin_settings())
    }

    pub fn list_proxies(&self) -> Result<ProxyListResponse, AdminServiceError> {
        let data = self
            .proxy_store
            .load()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        let snapshot = self.token_manager.snapshot();
        let proxies = data
            .proxies
            .iter()
            .map(|proxy| {
                let account_count = snapshot
                    .entries
                    .iter()
                    .filter(|entry| entry.proxy_id == Some(proxy.id))
                    .count();
                Self::proxy_list_item(proxy, account_count)
            })
            .collect::<Vec<_>>();
        Ok(ProxyListResponse {
            total: proxies.len(),
            enabled_count: proxies.iter().filter(|item| !item.disabled).count(),
            proxies,
        })
    }

    pub fn create_proxy(
        &self,
        req: ProxyUpsertRequest,
    ) -> Result<ProxyListItem, AdminServiceError> {
        let mut data = self
            .proxy_store
            .load()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Self::validate_proxy_request(&req)?;
        let now = ProxyItem::now_timestamp();
        let proxy = ProxyItem {
            id: ProxyStore::next_id(&data),
            name: req.name.trim().to_string(),
            protocol: req.protocol.trim().to_lowercase(),
            host: req.host.trim().to_string(),
            port: req.port,
            username: req.username.filter(|value| !value.trim().is_empty()),
            password: req.password.filter(|value| !value.trim().is_empty()),
            disabled: req.disabled,
            last_tested_at: None,
            last_test_status: None,
            last_latency_ms: None,
            last_error: None,
            quality_checked_at: None,
            quality_score: None,
            quality_grade: None,
            exit_ip: None,
            country: None,
            city: None,
            quality_error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        data.proxies.push(proxy.clone());
        self.proxy_store
            .save(&data)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(Self::proxy_list_item(&proxy, 0))
    }

    pub fn update_proxy(
        &self,
        id: u64,
        req: ProxyUpsertRequest,
    ) -> Result<ProxyListItem, AdminServiceError> {
        let mut data = self
            .proxy_store
            .load()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Self::validate_proxy_request(&req)?;
        let proxy = data
            .proxies
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| AdminServiceError::InvalidCredential(format!("代理不存在: {}", id)))?;
        proxy.name = req.name.trim().to_string();
        proxy.protocol = req.protocol.trim().to_lowercase();
        proxy.host = req.host.trim().to_string();
        proxy.port = req.port;
        proxy.username = req.username.filter(|value| !value.trim().is_empty());
        if let Some(password) = req.password.filter(|value| !value.trim().is_empty()) {
            proxy.password = Some(password);
        }
        proxy.disabled = req.disabled;
        proxy.updated_at = ProxyItem::now_timestamp();
        let result = Self::proxy_list_item(proxy, self.proxy_account_ids(id).len());
        self.proxy_store
            .save(&data)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(result)
    }

    pub fn delete_proxy(&self, id: u64) -> Result<(), AdminServiceError> {
        let accounts = self.proxy_account_ids(id);
        if !accounts.is_empty() {
            return Err(AdminServiceError::InvalidCredential(format!(
                "代理已关联 {} 个账号，请先迁移或解绑",
                accounts.len()
            )));
        }
        let mut data = self
            .proxy_store
            .load()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        let before = data.proxies.len();
        data.proxies.retain(|item| item.id != id);
        if data.proxies.len() == before {
            return Err(AdminServiceError::InvalidCredential(format!(
                "代理不存在: {}",
                id
            )));
        }
        self.proxy_store
            .save(&data)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(())
    }

    pub async fn test_proxy(&self, id: u64) -> Result<ProxyListItem, AdminServiceError> {
        let mut data = self
            .proxy_store
            .load()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        let proxy = data
            .proxies
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| AdminServiceError::InvalidCredential(format!("代理不存在: {}", id)))?;
        let started = std::time::Instant::now();
        let proxy_config = Self::proxy_config_from_item(proxy);
        let test_result = match build_client(
            Some(&proxy_config),
            15,
            self.token_manager.config().tls_backend,
        ) {
            Ok(client) => client
                .get("https://www.google.com/generate_204")
                .send()
                .await
                .and_then(|response| response.error_for_status())
                .map(|_| ())
                .map_err(anyhow::Error::from),
            Err(error) => Err(error),
        };
        proxy.last_tested_at = Some(ProxyItem::now_timestamp());
        proxy.last_latency_ms =
            Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        match test_result {
            Ok(_) => {
                proxy.last_test_status = Some("ok".to_string());
                proxy.last_error = None;
            }
            Err(error) => {
                proxy.last_test_status = Some("failed".to_string());
                proxy.last_error = Some(error.to_string());
            }
        }
        proxy.updated_at = ProxyItem::now_timestamp();
        let result = Self::proxy_list_item(proxy, self.proxy_account_ids(id).len());
        self.proxy_store
            .save(&data)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(result)
    }

    pub async fn batch_test_proxies(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_proxy_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.test_proxy(id).await {
                Ok(item) => {
                    if item.last_test_status.as_deref() == Some("ok") {
                        response.success_count += 1;
                    } else {
                        response.fail_count += 1;
                        response.messages.push(format!(
                            "#{}: {}",
                            id,
                            item.last_error.unwrap_or_else(|| "测试未通过".to_string())
                        ));
                    }
                }
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    pub fn batch_delete_proxies(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_proxy_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.delete_proxy(id) {
                Ok(_) => response.success_count += 1,
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    pub async fn batch_quality_check_proxies(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_proxy_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.quality_check_proxy(id).await {
                Ok(item) => {
                    if item.quality_error.is_none() {
                        response.success_count += 1;
                    } else {
                        response.fail_count += 1;
                        response.messages.push(format!(
                            "#{}: {}",
                            id,
                            item.quality_error
                                .unwrap_or_else(|| "质量检测失败".to_string())
                        ));
                    }
                }
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    async fn quality_check_proxy(&self, id: u64) -> Result<ProxyListItem, AdminServiceError> {
        let mut data = self
            .proxy_store
            .load()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        let proxy = data
            .proxies
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| AdminServiceError::InvalidCredential(format!("代理不存在: {}", id)))?;

        let started = std::time::Instant::now();
        let proxy_config = Self::proxy_config_from_item(proxy);
        let client = build_client(
            Some(&proxy_config),
            20,
            self.token_manager.config().tls_backend,
        )
        .map_err(|error| AdminServiceError::InternalError(error.to_string()))?;

        let payload = match client.get("https://ipapi.co/json/").send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response.json::<serde_json::Value>().await,
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };

        proxy.quality_checked_at = Some(ProxyItem::now_timestamp());
        proxy.last_latency_ms =
            Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        match payload {
            Ok(value) => {
                let latency = proxy.last_latency_ms.unwrap_or(0);
                let score = if latency <= 80 {
                    95
                } else if latency <= 150 {
                    85
                } else if latency <= 300 {
                    72
                } else {
                    55
                };
                proxy.quality_score = Some(score);
                proxy.quality_grade = Some(
                    if score >= 90 {
                        "A"
                    } else if score >= 80 {
                        "B"
                    } else if score >= 70 {
                        "C"
                    } else {
                        "D"
                    }
                    .to_string(),
                );
                proxy.exit_ip = value.get("ip").and_then(|v| v.as_str()).map(str::to_string);
                proxy.country = value
                    .get("country_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                proxy.city = value
                    .get("city")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                proxy.quality_error = None;
                proxy.last_test_status = Some("ok".to_string());
                proxy.last_error = None;
            }
            Err(error) => {
                proxy.quality_score = None;
                proxy.quality_grade = None;
                proxy.exit_ip = None;
                proxy.country = None;
                proxy.city = None;
                proxy.quality_error = Some(error.to_string());
                proxy.last_test_status = Some("failed".to_string());
                proxy.last_error = Some(error.to_string());
            }
        }
        proxy.updated_at = ProxyItem::now_timestamp();
        let result = Self::proxy_list_item(proxy, self.proxy_account_ids(id).len());
        self.proxy_store
            .save(&data)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        Ok(result)
    }

    pub fn proxy_accounts(&self, id: u64) -> Vec<CredentialStatusItem> {
        self.get_all_credentials()
            .credentials
            .into_iter()
            .filter(|credential| credential.proxy_id == Some(id))
            .collect()
    }

    pub fn batch_set_disabled(
        &self,
        req: BatchDisabledRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_batch_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.set_disabled(id, req.disabled) {
                Ok(_) => response.success_count += 1,
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    pub fn batch_reset(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_batch_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.reset_and_enable(id) {
                Ok(_) => response.success_count += 1,
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    pub async fn batch_refresh(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_batch_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.force_refresh_token(id).await {
                Ok(_) => response.success_count += 1,
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    pub async fn batch_balance(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchBalanceResponse, AdminServiceError> {
        self.validate_batch_ids(&req.ids)?;
        let mut response = BatchBalanceResponse {
            success_count: 0,
            fail_count: 0,
            balances: Vec::new(),
            messages: Vec::new(),
        };
        for id in req.ids {
            match self.fetch_balance(id).await {
                Ok(balance) => {
                    {
                        let mut cache = self.balance_cache.lock();
                        cache.insert(
                            id,
                            CachedBalance {
                                cached_at: Utc::now().timestamp() as f64,
                                data: balance.clone(),
                            },
                        );
                    }
                    response.success_count += 1;
                    response.balances.push(balance);
                }
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        self.save_balance_cache();
        Ok(response)
    }

    pub fn batch_delete(
        &self,
        req: BatchIdsRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_batch_ids(&req.ids)?;
        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            match self.delete_credential(id) {
                Ok(_) => response.success_count += 1,
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    pub fn batch_update_credentials(
        &self,
        req: BatchCredentialUpdateRequest,
    ) -> Result<BatchOperationResponse, AdminServiceError> {
        self.validate_batch_ids(&req.ids)?;
        if req.priority.is_none()
            && req.max_concurrent.is_none()
            && req.disabled.is_none()
            && req.proxy_mode.is_none()
        {
            return Err(AdminServiceError::InvalidCredential(
                "请选择至少一个要更新的内容".to_string(),
            ));
        }

        let proxy_binding = if let Some(mode) = req.proxy_mode.as_deref() {
            Some(self.resolve_proxy_binding(mode, req.proxy_id)?)
        } else {
            None
        };

        let mut response = BatchOperationResponse::default();
        for id in req.ids {
            let result = (|| -> Result<(), AdminServiceError> {
                if let Some(priority) = req.priority {
                    self.set_priority(id, priority)?;
                }
                if let Some(max_concurrent) = req.max_concurrent {
                    self.token_manager
                        .set_max_concurrent(id, max_concurrent)
                        .map_err(|e| self.classify_error(e, id))?;
                }
                if let Some(disabled) = req.disabled {
                    self.set_disabled(id, disabled)?;
                }
                if let Some((mode, proxy_id, proxy_url, proxy_username, proxy_password)) =
                    proxy_binding.clone()
                {
                    self.token_manager
                        .update_proxy_binding(
                            id,
                            mode,
                            proxy_id,
                            proxy_url,
                            proxy_username,
                            proxy_password,
                        )
                        .map_err(|e| self.classify_error(e, id))?;
                }
                Ok(())
            })();

            match result {
                Ok(_) => response.success_count += 1,
                Err(error) => {
                    response.fail_count += 1;
                    response.messages.push(format!("#{}: {}", id, error));
                }
            }
        }
        Ok(response)
    }

    fn current_admin_theme(&self) -> String {
        self.admin_theme.lock().clone()
    }

    fn current_admin_page_settings(&self) -> AdminPageSettings {
        self.admin_page_settings.lock().clone()
    }

    fn normalize_admin_theme(theme: &str) -> String {
        match theme {
            "light" | "dark" | "system" => theme.to_string(),
            _ => "system".to_string(),
        }
    }

    fn normalize_admin_page_settings(config: &AdminUiConfig) -> AdminPageSettings {
        AdminPageSettings {
            accounts_page_size: Self::normalize_admin_page_size(config.accounts_page_size, 20),
            records_page_size: Self::normalize_admin_page_size(config.records_page_size, 10),
        }
    }

    fn normalize_admin_page_size(value: usize, fallback: usize) -> usize {
        match value {
            10 | 20 | 50 | 100 => value,
            _ => fallback,
        }
    }

    fn validate_admin_page_size(value: usize) -> Result<usize, AdminServiceError> {
        if matches!(value, 10 | 20 | 50 | 100) {
            Ok(value)
        } else {
            Err(AdminServiceError::InvalidCredential(
                "每页数量只能是 10、20、50 或 100".to_string(),
            ))
        }
    }

    fn persist_admin_theme(&self, theme: String) -> Result<(), AdminServiceError> {
        use anyhow::Context;

        let config_path = self
            .token_manager
            .config()
            .config_path()
            .ok_or_else(|| {
                AdminServiceError::InternalError("配置文件路径未知，无法保存主题设置".to_string())
            })?
            .to_path_buf();

        let mut config = Config::load(&config_path)
            .with_context(|| format!("重新加载配置失败: {}", config_path.display()))
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        config.admin_ui.theme = theme.clone();
        config
            .save()
            .with_context(|| format!("保存主题设置失败: {}", config_path.display()))
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        *self.admin_theme.lock() = theme;
        Ok(())
    }

    fn persist_admin_page_settings(
        &self,
        accounts_page_size: Option<usize>,
        records_page_size: Option<usize>,
    ) -> Result<(), AdminServiceError> {
        use anyhow::Context;

        let current = self.current_admin_page_settings();
        let next_accounts_page_size = accounts_page_size
            .map(Self::validate_admin_page_size)
            .transpose()?
            .unwrap_or(current.accounts_page_size);
        let next_records_page_size = records_page_size
            .map(Self::validate_admin_page_size)
            .transpose()?
            .unwrap_or(current.records_page_size);

        let config_path = self
            .token_manager
            .config()
            .config_path()
            .ok_or_else(|| {
                AdminServiceError::InternalError("配置文件路径未知，无法保存分页设置".to_string())
            })?
            .to_path_buf();

        let mut config = Config::load(&config_path)
            .with_context(|| format!("重新加载配置失败: {}", config_path.display()))
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        config.admin_ui.accounts_page_size = next_accounts_page_size;
        config.admin_ui.records_page_size = next_records_page_size;
        config
            .save()
            .with_context(|| format!("保存分页设置失败: {}", config_path.display()))
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        *self.admin_page_settings.lock() = AdminPageSettings {
            accounts_page_size: next_accounts_page_size,
            records_page_size: next_records_page_size,
        };
        Ok(())
    }

    fn validate_proxy_request(req: &ProxyUpsertRequest) -> Result<(), AdminServiceError> {
        let name = req.name.trim();
        let protocol = req.protocol.trim().to_lowercase();
        let host = req.host.trim();

        if name.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "代理名称不能为空".to_string(),
            ));
        }
        if !matches!(protocol.as_str(), "http" | "https" | "socks5" | "socks5h") {
            return Err(AdminServiceError::InvalidCredential(
                "代理协议必须是 http、https、socks5 或 socks5h".to_string(),
            ));
        }
        if host.is_empty() || host.contains('/') || host.contains('\\') {
            return Err(AdminServiceError::InvalidCredential(
                "代理地址格式不正确".to_string(),
            ));
        }
        if req.port == 0 {
            return Err(AdminServiceError::InvalidCredential(
                "代理端口必须大于 0".to_string(),
            ));
        }
        Ok(())
    }

    fn proxy_list_item(proxy: &ProxyItem, account_count: usize) -> ProxyListItem {
        ProxyListItem {
            id: proxy.id,
            name: proxy.name.clone(),
            protocol: proxy.protocol.clone(),
            host: proxy.host.clone(),
            port: proxy.port,
            username: proxy.username.clone(),
            has_password: proxy
                .password
                .as_ref()
                .is_some_and(|value| !value.is_empty()),
            disabled: proxy.disabled,
            last_tested_at: proxy.last_tested_at.clone(),
            last_test_status: proxy.last_test_status.clone(),
            last_latency_ms: proxy.last_latency_ms,
            last_error: proxy.last_error.clone(),
            quality_checked_at: proxy.quality_checked_at.clone(),
            quality_score: proxy.quality_score,
            quality_grade: proxy.quality_grade.clone(),
            exit_ip: proxy.exit_ip.clone(),
            country: proxy.country.clone(),
            city: proxy.city.clone(),
            quality_error: proxy.quality_error.clone(),
            account_count,
            created_at: proxy.created_at.clone(),
            updated_at: proxy.updated_at.clone(),
        }
    }

    fn proxy_account_ids(&self, id: u64) -> Vec<u64> {
        self.token_manager
            .snapshot()
            .entries
            .into_iter()
            .filter(|entry| entry.proxy_id == Some(id))
            .map(|entry| entry.id)
            .collect()
    }

    fn proxy_config_from_item(proxy: &ProxyItem) -> ProxyConfig {
        let config = ProxyConfig::new(proxy.url());
        match (&proxy.username, &proxy.password) {
            (Some(username), Some(password)) if !username.is_empty() && !password.is_empty() => {
                config.with_auth(username.clone(), password.clone())
            }
            _ => config,
        }
    }

    fn validate_batch_ids(&self, ids: &[u64]) -> Result<(), AdminServiceError> {
        if ids.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "请选择至少一个账号".to_string(),
            ));
        }
        if ids.len() > 500 {
            return Err(AdminServiceError::InvalidCredential(
                "一次最多处理 500 个账号".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_proxy_ids(&self, ids: &[u64]) -> Result<(), AdminServiceError> {
        if ids.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "请选择至少一个代理".to_string(),
            ));
        }
        if ids.len() > 200 {
            return Err(AdminServiceError::InvalidCredential(
                "一次最多处理 200 个代理".to_string(),
            ));
        }
        Ok(())
    }

    fn resolve_proxy_binding(
        &self,
        mode: &str,
        proxy_id: Option<u64>,
    ) -> Result<
        (
            Option<String>,
            Option<u64>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
        AdminServiceError,
    > {
        match mode {
            "inherit" => Ok((Some("inherit".to_string()), None, None, None, None)),
            "direct" => Ok((
                Some("direct".to_string()),
                None,
                Some("direct".to_string()),
                None,
                None,
            )),
            "proxy" => {
                let proxy_id = proxy_id.ok_or_else(|| {
                    AdminServiceError::InvalidCredential("请选择要绑定的代理".to_string())
                })?;
                let data = self
                    .proxy_store
                    .load()
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
                let proxy = data
                    .proxies
                    .into_iter()
                    .find(|item| item.id == proxy_id)
                    .ok_or_else(|| {
                        AdminServiceError::InvalidCredential(format!("代理不存在: {}", proxy_id))
                    })?;
                if proxy.disabled {
                    return Err(AdminServiceError::InvalidCredential(
                        "请选择启用中的代理".to_string(),
                    ));
                }
                Ok((
                    Some("proxy".to_string()),
                    Some(proxy.id),
                    Some(proxy.url()),
                    proxy.username,
                    proxy.password,
                ))
            }
            _ => Err(AdminServiceError::InvalidCredential(
                "代理使用方式必须是 inherit、direct 或 proxy".to_string(),
            )),
        }
    }

    /// 获取系统版本信息
    pub fn get_system_version(&self) -> SystemVersionResponse {
        let latest_job = self.latest_job();
        let mut state = self.version_state.lock();
        state.response.latest_job = latest_job;
        state.response.clone()
    }

    /// 重新检查系统版本信息
    pub async fn check_system_version(&self) -> Result<SystemVersionResponse, AdminServiceError> {
        let latest_job = self.latest_job();
        let response = if self.is_docker_deployment() {
            let latest_docker = self.fetch_latest_docker_version().await?;
            self.build_docker_version_response(latest_docker, latest_job)
        } else {
            let latest_release = self.fetch_latest_release().await?;
            self.build_version_response(latest_release, latest_job)
        };
        self.version_state.lock().response = response.clone();
        Ok(response)
    }

    pub async fn start_system_update(
        self: &Arc<Self>,
        payload: SystemUpdateRequest,
    ) -> Result<SystemOperationJobResponse, AdminServiceError> {
        let config = self.token_manager.config();
        if !config.update.enabled {
            return Err(AdminServiceError::InvalidCredential(
                "当前未启用在线更新".to_string(),
            ));
        }
        if self.is_docker_deployment() {
            if config.update.update_command.trim().is_empty() {
                return Err(AdminServiceError::InvalidCredential(
                    "当前实例为容器部署，请先配置 update.updateCommand".to_string(),
                ));
            }
        } else if config.update.build_type != "release" {
            return Err(AdminServiceError::InvalidCredential(
                "当前实例不支持在线更新，请确认已启用 update.enabled 且 buildType 为 release"
                    .to_string(),
            ));
        }
        let version_info = if self.is_docker_deployment() {
            self.get_system_version()
        } else {
            self.check_system_version().await?
        };
        let target_version = payload
            .version
            .clone()
            .or_else(|| {
                version_info
                    .update_available
                    .then_some(version_info.latest_version.clone())
            })
            .or_else(|| Some(version_info.current_version.clone()));
        let target_version = target_version.filter(|value| !value.trim().is_empty());
        let job = self.create_job(
            "update",
            target_version.clone(),
            format!(
                "准备更新到 {}",
                target_version
                    .clone()
                    .unwrap_or_else(|| "当前版本".to_string())
            ),
        );
        let job_id = job.job_id.clone();
        let service = self.clone();
        tokio::spawn(async move {
            service.run_update_job(job_id, target_version).await;
        });
        Ok(job)
    }

    pub async fn start_system_rollback(
        self: &Arc<Self>,
        payload: SystemRollbackRequest,
    ) -> Result<SystemOperationJobResponse, AdminServiceError> {
        let config = self.token_manager.config();
        if self.is_docker_deployment() {
            return Err(AdminServiceError::InvalidCredential(
                "当前容器部署暂不支持在线回滚".to_string(),
            ));
        }
        if !config.update.enabled || config.update.build_type != "release" {
            return Err(AdminServiceError::InvalidCredential(
                "当前实例不支持在线回滚，请确认 buildType 为 release".to_string(),
            ));
        }
        let backup_name = payload.backup_name.filter(|value| !value.trim().is_empty());
        let job = self.create_job(
            "rollback",
            backup_name.clone(),
            if let Some(name) = &backup_name {
                format!("准备回滚到备份 {}", name)
            } else {
                "准备回滚到最近一次备份".to_string()
            },
        );
        let job_id = job.job_id.clone();
        let service = self.clone();
        tokio::spawn(async move {
            service.run_rollback_job(job_id, backup_name).await;
        });
        Ok(job)
    }

    pub async fn start_system_restart(
        self: &Arc<Self>,
    ) -> Result<SystemOperationJobResponse, AdminServiceError> {
        let config = self.token_manager.config();
        if config.update.restart_command.trim().is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "当前未配置在线重启命令".to_string(),
            ));
        }
        let job = self.create_job("restart", None, "准备重启当前实例".to_string());
        let job_id = job.job_id.clone();
        let service = self.clone();
        tokio::spawn(async move {
            service.run_restart_job(job_id).await;
        });
        Ok(job)
    }

    pub fn get_system_job(
        &self,
        id: &str,
    ) -> Result<SystemOperationJobResponse, AdminServiceError> {
        if let Some(job) = self.system_jobs.lock().get(id).cloned() {
            if let Some(persisted) = self
                .load_persisted_job()
                .filter(|persisted| persisted.job_id == id)
                .filter(|persisted| Self::should_prefer_persisted_job(&job, persisted))
            {
                self.system_jobs
                    .lock()
                    .insert(persisted.job_id.clone(), persisted.clone());
                self.sync_latest_job(Some(persisted.clone()));
                return Ok(persisted);
            }
            return Ok(job);
        }
        let job = self
            .load_persisted_job()
            .filter(|job| job.job_id == id)
            .ok_or_else(|| AdminServiceError::InvalidCredential(format!("任务不存在: {}", id)))?;
        self.system_jobs
            .lock()
            .insert(job.job_id.clone(), job.clone());
        Ok(job)
    }

    /// 设置负载均衡模式
    pub fn set_load_balancing_mode(
        &self,
        req: SetLoadBalancingModeRequest,
    ) -> Result<LoadBalancingModeResponse, AdminServiceError> {
        // 验证模式值
        if req.mode != "priority" && req.mode != "balanced" {
            return Err(AdminServiceError::InvalidCredential(
                "mode 必须是 'priority' 或 'balanced'".to_string(),
            ));
        }

        self.token_manager
            .set_load_balancing_mode(req.mode.clone())
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        Ok(LoadBalancingModeResponse { mode: req.mode })
    }

    fn persist_prompt_cache_url(&self, redis_url: Option<String>) -> anyhow::Result<()> {
        use anyhow::Context;

        let config_path = self
            .token_manager
            .config()
            .config_path()
            .ok_or_else(|| anyhow::anyhow!("配置文件路径未知，无法保存 Redis 设置"))?
            .to_path_buf();

        let mut config = Config::load(&config_path)
            .with_context(|| format!("重新加载配置失败: {}", config_path.display()))?;
        config.redis_url = redis_url;
        config
            .save()
            .with_context(|| format!("保存 Redis 设置失败: {}", config_path.display()))?;
        Ok(())
    }

    /// 强制刷新指定凭据的 Token
    pub async fn force_refresh_token(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .force_refresh_token_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))
    }

    /// 对单个账号发起真实流式测试，实时返回测试事件流。
    pub async fn test_credential(
        &self,
        id: u64,
        payload: CredentialTestRequest,
    ) -> Result<TestEventStream, AdminServiceError> {
        let prompt = if payload.prompt.trim().is_empty() {
            "请回复一句简短的话，确认连接已可用。".to_string()
        } else {
            payload.prompt.trim().to_string()
        };
        let model_id = payload.model_id.clone();

        let request_body = serde_json::to_string(&KiroRequest {
            conversation_state: ConversationState::new(Uuid::new_v4().to_string())
                .with_agent_continuation_id(Uuid::new_v4().to_string())
                .with_agent_task_type("vibe")
                .with_chat_trigger_type("MANUAL")
                .with_current_message(CurrentMessage::new(
                    UserInputMessage::new(prompt.clone(), payload.model_id.clone())
                        .with_origin("AI_EDITOR"),
                )),
            profile_arn: None,
        })
        .map_err(|e| AdminServiceError::InternalError(format!("测试请求序列化失败: {}", e)))?;

        let mut options = AcquireOptions::new(Some(payload.model_id.clone()));
        options.preferred_account_id = Some(id);
        options.strict_preferred_account = true;
        options.runtime_probe = true;

        let provider_response = self
            .provider
            .call_api_stream_for_account_with_metadata(
                &request_body,
                options,
                Some(&payload.model_id),
                None,
            )
            .await
            .map_err(|e| self.classify_test_credential_error(e, id, &model_id))?;

        let token_manager = self.provider.token_manager().clone();
        let dispatch_path = provider_response.dispatch_path.clone();
        let used_soft_fallback = provider_response.used_soft_fallback;
        let account_state_at_start = provider_response.account_state_at_start.clone();
        let mut stream = provider_response.response.bytes_stream();
        let mut lease = Some(provider_response.lease);
        let mut decoder = EventStreamDecoder::new();
        let mut content = String::new();

        let output = async_stream::try_stream! {
            yield json!({
                "type": "test_start",
                "accountId": id,
                "model": model_id,
                "dispatchPath": dispatch_path,
                "usedSoftFallback": used_soft_fallback,
                "accountStateAtStart": account_state_at_start,
            });

            loop {
                let next_chunk = timeout(Duration::from_secs(30), stream.next())
                    .await
                    .map_err(|_| AdminServiceError::UpstreamError("测试请求超时".to_string()))?;

                match next_chunk {
                    Some(Ok(chunk)) => {
                        if let Err(e) = decoder.feed(&chunk) {
                            tracing::warn!("测试流解码缓冲区异常: {}", e);
                        }

                        for frame in decoder.decode_iter().flatten() {
                            if let Ok(event) = Event::from_frame(frame) {
                                match event {
                                    Event::AssistantResponse(resp) if !resp.content.is_empty() => {
                                        content.push_str(&resp.content);
                                        yield json!({
                                            "type": "content",
                                            "text": resp.content,
                                        });
                                    }
                                    Event::ToolUse(tool_use) => {
                                        yield json!({
                                            "type": "tool_use",
                                            "name": tool_use.name,
                                            "input": tool_use.input,
                                            "stop": tool_use.stop,
                                        });
                                    }
                                    Event::ContextUsage(ctx) => {
                                        yield json!({
                                            "type": "context_usage",
                                            "percentage": ctx.context_usage_percentage,
                                        });
                                    }
                                    Event::Error { error_code, error_message } => {
                                        yield json!({
                                            "type": "upstream_error",
                                            "code": error_code,
                                            "message": error_message,
                                        });
                                    }
                                    Event::Exception { exception_type, message } => {
                                        yield json!({
                                            "type": "upstream_exception",
                                            "exceptionType": exception_type,
                                            "message": message,
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        if let Some(mut current_lease) = lease.take() {
                            token_manager.release_slot(&mut current_lease);
                        }
                        Err(AdminServiceError::UpstreamError(format!("读取测试流失败: {}", e)))?;
                    }
                    None => {
                        if let Some(mut current_lease) = lease.take() {
                            token_manager.release_slot(&mut current_lease);
                        }
                        yield json!({
                            "type": "test_complete",
                            "success": true,
                            "summary": content,
                        });
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(output))
    }

    fn build_admin_http_client(config: &Config) -> anyhow::Result<Client> {
        let proxy = config
            .update
            .proxy_url
            .as_ref()
            .map(|url| ProxyConfig::new(url.clone()));
        build_client(proxy.as_ref(), 60, config.tls_backend)
    }

    fn detect_current_commit() -> Option<String> {
        let candidates = [
            std::env::var("GITHUB_SHA").ok(),
            std::env::var("VERGEN_GIT_SHA").ok(),
            option_env!("GIT_COMMIT_HASH").map(|value| value.to_string()),
        ];
        candidates
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())
            .map(|value| value.chars().take(12).collect())
    }

    fn build_default_version_response(
        config: &Config,
        current_version: String,
        current_commit: Option<String>,
        latest_job: Option<SystemOperationJobResponse>,
    ) -> SystemVersionResponse {
        let deployment_mode = if !config.update.deployment_mode.trim().is_empty() {
            config.update.deployment_mode.clone()
        } else if std::path::Path::new("/.dockerenv").exists() {
            "docker".to_string()
        } else {
            "binary".to_string()
        };
        let build_type = if !config.update.build_type.trim().is_empty() {
            config.update.build_type.clone()
        } else {
            "release".to_string()
        };
        let can_update = if deployment_mode == "docker" {
            config.update.enabled && !config.update.update_command.trim().is_empty()
        } else {
            config.update.enabled && build_type == "release"
        };
        let can_rollback = if deployment_mode == "docker" {
            false
        } else {
            can_update
        };
        let can_restart = !config.update.restart_command.trim().is_empty();
        SystemVersionResponse {
            current_version: current_version.clone(),
            latest_version: current_version,
            update_available: false,
            latest_published_at: None,
            release_notes_url: None,
            build_type: build_type.clone(),
            deployment_mode: deployment_mode.clone(),
            can_update,
            can_rollback,
            can_restart,
            update_hint: if deployment_mode == "docker" && can_update {
                "当前实例支持一键更新测试容器，更新后会自动做健康检查。".to_string()
            } else if deployment_mode == "docker" {
                "当前实例为容器部署，请先配置 update.updateCommand 后再使用一键更新。".to_string()
            } else if can_update {
                "当前实例支持在线下载和替换，更新完成后需要手动重启生效。".to_string()
            } else if build_type == "source" {
                "当前实例为源码构建，请通过新的构建产物升级。".to_string()
            } else {
                "当前实例未满足在线更新条件，请检查 update 配置。".to_string()
            },
            checked_at: Utc::now().to_rfc3339(),
            current_commit,
            channel: Some(config.update.channel.clone()),
            latest_job,
        }
    }

    fn build_version_response(
        &self,
        latest_release: Option<GitHubRelease>,
        latest_job: Option<SystemOperationJobResponse>,
    ) -> SystemVersionResponse {
        let config = self.token_manager.config();
        let mut response = Self::build_default_version_response(
            config,
            self.current_version.clone(),
            self.current_commit.clone(),
            latest_job,
        );
        if let Some(release) = latest_release {
            let latest_version = release.tag_name.trim_start_matches('v').to_string();
            response.latest_version = latest_version.clone();
            response.update_available = latest_version != self.current_version;
            response.latest_published_at = release.published_at;
            response.release_notes_url = Some(release.html_url);
            response.update_hint = if response.update_available {
                format!(
                    "发现新版本 {}，可以先更新，再按需手动重启。",
                    latest_version
                )
            } else if response.can_restart || response.can_rollback {
                "当前已经是最新版本，可继续在这里执行重启或回滚。".to_string()
            } else {
                response.update_hint
            };
        }
        response
    }

    fn build_docker_version_response(
        &self,
        latest_docker: Option<DockerVersionInfo>,
        latest_job: Option<SystemOperationJobResponse>,
    ) -> SystemVersionResponse {
        let config = self.token_manager.config();
        let mut response = Self::build_default_version_response(
            config,
            self.current_version.clone(),
            self.current_commit.clone(),
            latest_job,
        );
        if let Some(docker_version) = latest_docker {
            response.latest_version = docker_version.version.clone();
            response.update_available = docker_version.update_available;
            response.latest_published_at = docker_version.published_at;
            response.release_notes_url = docker_version.release_notes_url;
            response.update_hint = if response.update_available {
                format!(
                    "发现新的测试镜像版本 {}，可以直接更新测试实例并自动检查可用性。",
                    docker_version.version
                )
            } else if response.can_restart {
                "当前测试实例已经是最新镜像，可继续在这里重启并复查。".to_string()
            } else {
                response.update_hint
            };
        } else {
            response.update_hint =
                "暂未获取到可用的测试镜像版本，请先确认 GitHub Actions 已成功推送 beta 镜像。"
                    .to_string();
        }
        response
    }

    async fn fetch_latest_release(&self) -> Result<Option<GitHubRelease>, AdminServiceError> {
        let config = self.token_manager.config();
        let repo = config.update.github_repo.trim();
        if repo.is_empty() {
            return Ok(None);
        }
        let url = format!("{}/{}/releases", GITHUB_API_BASE, repo);
        let response = self
            .http_client
            .get(url)
            .header(reqwest::header::USER_AGENT, "kiro-rs-admin")
            .send()
            .await
            .map_err(|e| {
                AdminServiceError::UpstreamError(format!("检查 GitHub Release 失败: {}", e))
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdminServiceError::UpstreamError(format!(
                "GitHub Release 检查失败: {} {}",
                status, body
            )));
        }
        let releases: Vec<GitHubRelease> = response.json().await.map_err(|e| {
            AdminServiceError::UpstreamError(format!("解析 GitHub Release 失败: {}", e))
        })?;
        let selected = releases.into_iter().find(|release| {
            if release.prerelease && !config.update.allow_prerelease {
                return false;
            }
            if config.update.channel == "stable" {
                !release.prerelease
            } else {
                true
            }
        });
        Ok(selected)
    }

    async fn fetch_latest_docker_version(
        &self,
    ) -> Result<Option<DockerVersionInfo>, AdminServiceError> {
        let config = self.token_manager.config();
        let channel = config.update.channel.trim();
        if channel.is_empty() {
            return Ok(None);
        }

        let current_version = self.current_version.trim();
        let update_available = current_version != channel;

        Ok(Some(DockerVersionInfo {
            version: channel.to_string(),
            published_at: None,
            release_notes_url: None,
            update_available,
        }))
    }

    fn create_job(
        &self,
        operation: &str,
        target_version: Option<String>,
        message: String,
    ) -> SystemOperationJobResponse {
        let job = SystemOperationJobResponse {
            job_id: Uuid::new_v4().to_string(),
            operation: operation.to_string(),
            status: "running".to_string(),
            target_version,
            current_version: Some(self.current_version.clone()),
            started_at: Some(Utc::now().to_rfc3339()),
            finished_at: None,
            message,
            can_retry: false,
        };
        self.system_jobs
            .lock()
            .insert(job.job_id.clone(), job.clone());
        self.sync_latest_job(Some(job.clone()));
        self.persist_system_job(Some(&job));
        job
    }

    fn update_job(
        &self,
        job_id: &str,
        status: &str,
        message: String,
        can_retry: bool,
    ) -> Option<SystemOperationJobResponse> {
        let mut jobs = self.system_jobs.lock();
        let job = jobs.get_mut(job_id)?;
        job.status = status.to_string();
        job.message = message;
        job.can_retry = can_retry;
        job.finished_at = Some(Utc::now().to_rfc3339());
        let cloned = job.clone();
        drop(jobs);
        self.sync_latest_job(Some(cloned.clone()));
        self.persist_system_job(Some(&cloned));
        Some(cloned)
    }

    fn touch_job(&self, job_id: &str, message: String) -> Option<SystemOperationJobResponse> {
        let mut jobs = self.system_jobs.lock();
        let job = jobs.get_mut(job_id)?;
        job.message = message;
        let cloned = job.clone();
        drop(jobs);
        self.sync_latest_job(Some(cloned.clone()));
        self.persist_system_job(Some(&cloned));
        Some(cloned)
    }

    fn latest_job(&self) -> Option<SystemOperationJobResponse> {
        let latest = self
            .system_jobs
            .lock()
            .values()
            .cloned()
            .max_by(|left, right| left.started_at.cmp(&right.started_at));
        let persisted = self.load_persisted_job();
        match (latest, persisted) {
            (Some(current), Some(persisted))
                if Self::should_prefer_persisted_job(&current, &persisted) =>
            {
                self.system_jobs
                    .lock()
                    .insert(persisted.job_id.clone(), persisted.clone());
                self.sync_latest_job(Some(persisted.clone()));
                Some(persisted)
            }
            (Some(current), _) => Some(current),
            (None, persisted) => persisted,
        }
    }

    fn sync_latest_job(&self, latest_job: Option<SystemOperationJobResponse>) {
        self.version_state.lock().response.latest_job = latest_job;
    }

    fn system_job_path(&self) -> Option<PathBuf> {
        self.token_manager
            .cache_dir()
            .map(|dir| dir.join("kiro_system_job.json"))
    }

    fn load_persisted_job(&self) -> Option<SystemOperationJobResponse> {
        Self::load_system_job_from(&self.system_job_path())
    }

    fn load_system_job_from(job_path: &Option<PathBuf>) -> Option<SystemOperationJobResponse> {
        let path = job_path.as_ref()?;
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn persist_system_job(&self, job: Option<&SystemOperationJobResponse>) {
        let Some(path) = self.system_job_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!("创建系统任务状态目录失败: {}", error);
                return;
            }
        }
        match job {
            Some(job) => match serde_json::to_vec_pretty(job) {
                Ok(payload) => {
                    if let Err(error) = std::fs::write(&path, payload) {
                        tracing::warn!("写入系统任务状态文件失败: {}", error);
                    }
                }
                Err(error) => {
                    tracing::warn!("序列化系统任务状态失败: {}", error);
                }
            },
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    fn should_prefer_persisted_job(
        current: &SystemOperationJobResponse,
        persisted: &SystemOperationJobResponse,
    ) -> bool {
        if persisted.job_id != current.job_id {
            return persisted.started_at > current.started_at;
        }
        if current.status == "running" && persisted.status != "running" {
            return true;
        }
        if current.finished_at.is_none() && persisted.finished_at.is_some() {
            return true;
        }
        persisted.started_at > current.started_at
    }

    fn build_running_job(
        &self,
        job_id: &str,
        operation: &str,
        target_version: Option<String>,
        message: String,
    ) -> Result<SystemOperationJobResponse, AdminServiceError> {
        let existing = self.get_system_job(job_id)?;
        Ok(SystemOperationJobResponse {
            job_id: existing.job_id,
            operation: operation.to_string(),
            status: "running".to_string(),
            target_version,
            current_version: Some(self.current_version.clone()),
            started_at: existing.started_at,
            finished_at: None,
            message,
            can_retry: false,
        })
    }

    fn build_finished_job(
        &self,
        base: &SystemOperationJobResponse,
        status: &str,
        message: String,
        can_retry: bool,
    ) -> Result<SystemOperationJobResponse, AdminServiceError> {
        let finished_at = Utc::now().to_rfc3339();
        let started_at = base
            .started_at
            .clone()
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let parsed_start = DateTime::parse_from_rfc3339(&started_at)
            .map_err(|e| AdminServiceError::InternalError(format!("任务开始时间格式无效: {}", e)))?
            .with_timezone(&Utc);
        let parsed_finish = DateTime::parse_from_rfc3339(&finished_at)
            .map_err(|e| AdminServiceError::InternalError(format!("任务结束时间格式无效: {}", e)))?
            .with_timezone(&Utc);
        let mut finished = base.clone();
        finished.status = status.to_string();
        finished.message = message;
        finished.can_retry = can_retry;
        finished.started_at = Some(parsed_start.to_rfc3339());
        finished.finished_at = Some(parsed_finish.to_rfc3339());
        Ok(finished)
    }

    async fn run_update_job(self: Arc<Self>, job_id: String, target_version: Option<String>) {
        if self.is_docker_deployment() {
            let target_label = target_version
                .clone()
                .unwrap_or_else(|| self.token_manager.config().update.channel.clone());
            match self
                .dispatch_detached_docker_job(
                    &job_id,
                    "update",
                    Some(target_label.clone()),
                    self.token_manager.config().update.update_command.clone(),
                    "已转交宿主机后台执行更新，等待实例恢复并完成健康检查".to_string(),
                    format!("测试实例已更新到 {}，健康检查通过", target_label),
                    format!("测试实例已更新到 {}，但健康检查未通过", target_label),
                    "更新命令未能在宿主机后台完成".to_string(),
                )
                .await
            {
                Ok(()) => {
                    let _ = self.touch_job(
                        &job_id,
                        "已转交宿主机后台执行更新，等待实例恢复并完成健康检查".to_string(),
                    );
                }
                Err(error) => {
                    let _ = self.update_job(&job_id, "failed", error.to_string(), true);
                }
            }
            return;
        }

        let result = async {
            let release = self.fetch_latest_release().await?.ok_or_else(|| {
                AdminServiceError::UpstreamError("未获取到可用发布版本".to_string())
            })?;
            let expected_version = target_version
                .clone()
                .unwrap_or_else(|| release.tag_name.trim_start_matches('v').to_string());
            let staging_dir = self.prepare_update_dirs().await?;
            let release_file = staging_dir.join(format!(
                "release-{}.json",
                expected_version.replace('/', "_")
            ));
            let release_payload = serde_json::to_vec_pretty(&json!({
                "tagName": release.tag_name,
                "htmlUrl": release.html_url,
                "publishedAt": release.published_at,
                "body": release.body,
                "checkedAt": Utc::now().to_rfc3339(),
            }))
            .map_err(|e| {
                AdminServiceError::InternalError(format!("序列化 release 元信息失败: {}", e))
            })?;
            tokio::fs::write(&release_file, release_payload)
                .await
                .map_err(|e| {
                    AdminServiceError::InternalError(format!("写入 release 元信息失败: {}", e))
                })?;
            let backup_dir = self.create_backup().await?;
            self.apply_release_binary(&release, &expected_version, &staging_dir)
                .await?;
            Ok::<String, AdminServiceError>(format!(
                "已完成更新准备，目标版本 {}，备份目录 {}，请执行重启使新版本生效",
                expected_version,
                backup_dir.display()
            ))
        }
        .await;

        match result {
            Ok(message) => {
                let _ = self.update_job(&job_id, "succeeded", message, false);
            }
            Err(error) => {
                let _ = self.update_job(&job_id, "failed", error.to_string(), true);
            }
        }
    }

    async fn run_rollback_job(self: Arc<Self>, job_id: String, backup_name: Option<String>) {
        let result = async {
            let backup_dir = self.restore_backup(backup_name.as_deref()).await?;
            self.apply_backup_binary(&backup_dir).await?;
            Ok::<String, AdminServiceError>(format!(
                "已回滚到备份 {}，请执行重启使回滚生效",
                backup_dir.display()
            ))
        }
        .await;
        match result {
            Ok(message) => {
                let _ = self.update_job(&job_id, "rolled_back", message, false);
            }
            Err(error) => {
                let _ = self.update_job(&job_id, "failed", error.to_string(), true);
            }
        }
    }

    async fn run_restart_job(self: Arc<Self>, job_id: String) {
        if self.is_docker_deployment() {
            match self
                .dispatch_detached_docker_job(
                    &job_id,
                    "restart",
                    None,
                    self.token_manager.config().update.restart_command.clone(),
                    "已转交宿主机后台执行重启，等待实例恢复并完成健康检查".to_string(),
                    "重启命令已执行，健康检查通过".to_string(),
                    "重启命令已执行，但健康检查未通过".to_string(),
                    "重启命令未能在宿主机后台完成".to_string(),
                )
                .await
            {
                Ok(()) => {
                    let _ = self.touch_job(
                        &job_id,
                        "已转交宿主机后台执行重启，等待实例恢复并完成健康检查".to_string(),
                    );
                }
                Err(error) => {
                    let _ = self.update_job(&job_id, "failed", error.to_string(), true);
                }
            }
            return;
        }

        let result = async {
            self.run_restart_command().await?;
            self.run_healthcheck().await?;
            Ok::<String, AdminServiceError>("重启命令已执行，健康检查通过".to_string())
        }
        .await;
        match result {
            Ok(message) => {
                let _ = self.update_job(&job_id, "succeeded", message, false);
            }
            Err(error) => {
                let _ = self.update_job(&job_id, "failed", error.to_string(), true);
            }
        }
    }

    async fn prepare_update_dirs(&self) -> Result<PathBuf, AdminServiceError> {
        let config = self.token_manager.config();
        let dir = self.resolve_workspace_path(&config.update.download_dir)?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("创建下载目录失败: {}", e)))?;
        Ok(dir)
    }

    async fn create_backup(&self) -> Result<PathBuf, AdminServiceError> {
        let config = self.token_manager.config();
        let backup_root = self.resolve_workspace_path(&config.update.backup_dir)?;
        tokio::fs::create_dir_all(&backup_root)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("创建备份目录失败: {}", e)))?;
        let backup_dir = backup_root.join(Utc::now().format("%Y%m%d%H%M%S").to_string());
        tokio::fs::create_dir_all(&backup_dir)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("创建备份快照失败: {}", e)))?;
        if let Ok(current_exe) = std::env::current_exe() {
            let target = backup_dir.join(
                current_exe
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("kiro-rs")),
            );
            tokio::fs::copy(&current_exe, &target).await.map_err(|e| {
                AdminServiceError::InternalError(format!("备份当前二进制失败: {}", e))
            })?;
        }
        self.cleanup_old_backups(&backup_root, config.update.max_backups)
            .await?;
        Ok(backup_dir)
    }

    async fn restore_backup(
        &self,
        backup_name: Option<&str>,
    ) -> Result<PathBuf, AdminServiceError> {
        let config = self.token_manager.config();
        let backup_root = self.resolve_workspace_path(&config.update.backup_dir)?;
        let mut entries = tokio::fs::read_dir(&backup_root)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("读取备份目录失败: {}", e)))?;
        let mut candidates = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("读取备份条目失败: {}", e)))?
        {
            let path = entry.path();
            if path.is_dir() {
                candidates.push(path);
            }
        }
        candidates.sort();
        let selected = if let Some(name) = backup_name {
            let candidate = backup_root.join(name);
            if candidates.iter().any(|path| path == &candidate) {
                candidate
            } else {
                return Err(AdminServiceError::InvalidCredential(format!(
                    "指定备份不存在: {}",
                    name
                )));
            }
        } else {
            candidates.pop().ok_or_else(|| {
                AdminServiceError::InvalidCredential("当前没有可回滚的备份".to_string())
            })?
        };
        Ok(selected)
    }

    async fn cleanup_old_backups(
        &self,
        backup_root: &Path,
        max_backups: usize,
    ) -> Result<(), AdminServiceError> {
        if max_backups == 0 {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(backup_root)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("读取备份目录失败: {}", e)))?;
        let mut dirs = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("读取备份条目失败: {}", e)))?
        {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
        dirs.sort();
        while dirs.len() > max_backups {
            let path = dirs.remove(0);
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|e| AdminServiceError::InternalError(format!("清理旧备份失败: {}", e)))?;
        }
        Ok(())
    }

    async fn run_restart_command(&self) -> Result<(), AdminServiceError> {
        let config = self.token_manager.config();
        let command_line = config.update.restart_command.trim();
        if command_line.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "未配置可执行的重启命令".to_string(),
            ));
        }
        let status = Command::new("sh")
            .arg("-lc")
            .arg(command_line)
            .status()
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("执行命令失败: {}", e)))?;
        if !status.success() {
            return Err(AdminServiceError::InternalError(format!(
                "命令执行失败，退出码: {:?}",
                status.code()
            )));
        }
        Ok(())
    }

    async fn apply_release_binary(
        &self,
        release: &GitHubRelease,
        version: &str,
        staging_dir: &Path,
    ) -> Result<(), AdminServiceError> {
        let archive_asset = self
            .select_release_archive(release, version)
            .ok_or_else(|| {
                AdminServiceError::UpstreamError("未找到当前平台对应的发布包".to_string())
            })?;
        let checksum_asset = release
            .assets
            .iter()
            .find(|asset| asset.name == "checksums.txt");

        let archive_path = staging_dir.join(&archive_asset.name);
        let archive_bytes = self
            .download_release_asset(&archive_asset.browser_download_url)
            .await?;
        tokio::fs::write(&archive_path, &archive_bytes)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("写入更新包失败: {}", e)))?;

        if let Some(asset) = checksum_asset {
            let checksum_bytes = self
                .download_release_asset(&asset.browser_download_url)
                .await?;
            self.verify_archive_checksum(&archive_asset.name, &archive_bytes, &checksum_bytes)?;
        }

        let extracted_path = staging_dir.join("kiro-rs.new");
        self.extract_binary_from_archive(&archive_bytes, &extracted_path)
            .await?;
        self.replace_current_binary(&extracted_path).await
    }

    async fn apply_backup_binary(&self, backup_dir: &Path) -> Result<(), AdminServiceError> {
        let current_exe = self.current_executable_path()?;
        let backup_binary = backup_dir.join(
            current_exe
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("kiro-rs")),
        );
        if !backup_binary.exists() {
            return Err(AdminServiceError::InvalidCredential(format!(
                "备份中未找到可恢复的二进制文件: {}",
                backup_binary.display()
            )));
        }

        let temp_restore = backup_dir.join("kiro-rs.rollback");
        tokio::fs::copy(&backup_binary, &temp_restore)
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("复制回滚文件失败: {}", e)))?;
        self.replace_current_binary(&temp_restore).await
    }

    fn select_release_archive<'a>(
        &self,
        release: &'a GitHubRelease,
        version: &str,
    ) -> Option<&'a GitHubReleaseAsset> {
        let config = self.token_manager.config();
        let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let expected_name = config
            .update
            .artifact_name_template
            .replace("{version}", version)
            .replace("{target}", &target);
        release
            .assets
            .iter()
            .find(|asset| asset.name == expected_name)
    }

    async fn download_release_asset(&self, url: &str) -> Result<Vec<u8>, AdminServiceError> {
        let response = self
            .http_client
            .get(url)
            .header(reqwest::header::USER_AGENT, "kiro-rs-admin")
            .send()
            .await
            .map_err(|e| AdminServiceError::UpstreamError(format!("下载更新文件失败: {}", e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdminServiceError::UpstreamError(format!(
                "下载更新文件失败: {} {}",
                status, body
            )));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| AdminServiceError::UpstreamError(format!("读取更新文件失败: {}", e)))
    }

    fn verify_archive_checksum(
        &self,
        archive_name: &str,
        archive_bytes: &[u8],
        checksum_bytes: &[u8],
    ) -> Result<(), AdminServiceError> {
        let checksum_text = String::from_utf8(checksum_bytes.to_vec())
            .map_err(|e| AdminServiceError::InternalError(format!("解析校验文件失败: {}", e)))?;
        let expected = checksum_text
            .lines()
            .find_map(|line| {
                let mut parts = line.split_whitespace();
                let checksum = parts.next()?;
                let name = parts.next()?.trim_start_matches('*');
                (name == archive_name).then_some(checksum.to_string())
            })
            .ok_or_else(|| {
                AdminServiceError::UpstreamError("校验文件中未找到对应更新包".to_string())
            })?;
        let actual = hex::encode(Sha256::digest(archive_bytes));
        if actual != expected {
            return Err(AdminServiceError::InternalError(format!(
                "更新包校验失败: 期望 {}, 实际 {}",
                expected, actual
            )));
        }
        Ok(())
    }

    async fn extract_binary_from_archive(
        &self,
        archive_bytes: &[u8],
        output_path: &Path,
    ) -> Result<(), AdminServiceError> {
        let output_path = output_path.to_path_buf();
        let archive_bytes = archive_bytes.to_vec();
        tokio::task::spawn_blocking(move || -> Result<(), AdminServiceError> {
            let cursor = Cursor::new(archive_bytes);
            let decoder = GzDecoder::new(cursor);
            let mut archive = Archive::new(decoder);
            let mut extracted = false;

            for entry in archive
                .entries()
                .map_err(|e| AdminServiceError::InternalError(format!("读取更新包失败: {}", e)))?
            {
                let mut entry = entry.map_err(|e| {
                    AdminServiceError::InternalError(format!("读取更新条目失败: {}", e))
                })?;
                let path = entry.path().map_err(|e| {
                    AdminServiceError::InternalError(format!("解析更新条目路径失败: {}", e))
                })?;
                if path.file_name() == Some(std::ffi::OsStr::new("kiro-rs")) {
                    entry.unpack(&output_path).map_err(|e| {
                        AdminServiceError::InternalError(format!("解包更新二进制失败: {}", e))
                    })?;
                    extracted = true;
                    break;
                }
            }

            if !extracted {
                return Err(AdminServiceError::InternalError(
                    "更新包中未找到 kiro-rs 可执行文件".to_string(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                std::fs::set_permissions(&output_path, PermissionsExt::from_mode(0o755)).map_err(
                    |e| AdminServiceError::InternalError(format!("设置更新文件权限失败: {}", e)),
                )?;
            }
            Ok(())
        })
        .await
        .map_err(|e| AdminServiceError::InternalError(format!("解包任务失败: {}", e)))?
    }

    async fn replace_current_binary(&self, candidate_path: &Path) -> Result<(), AdminServiceError> {
        let current_exe = self.current_executable_path()?;
        let previous_path = current_exe.with_extension("previous");
        if previous_path.exists() {
            let _ = tokio::fs::remove_file(&previous_path).await;
        }

        tokio::fs::rename(&current_exe, &previous_path)
            .await
            .map_err(|e| {
                AdminServiceError::InternalError(format!("备份当前可执行文件失败: {}", e))
            })?;
        if let Err(err) = tokio::fs::rename(candidate_path, &current_exe).await {
            let _ = tokio::fs::rename(&previous_path, &current_exe).await;
            return Err(AdminServiceError::InternalError(format!(
                "替换当前可执行文件失败: {}",
                err
            )));
        }
        Ok(())
    }

    fn current_executable_path(&self) -> Result<PathBuf, AdminServiceError> {
        let current_exe = std::env::current_exe().map_err(|e| {
            AdminServiceError::InternalError(format!("获取当前可执行文件失败: {}", e))
        })?;
        current_exe.canonicalize().map_err(|e| {
            AdminServiceError::InternalError(format!("解析当前可执行文件路径失败: {}", e))
        })
    }

    async fn run_healthcheck(&self) -> Result<(), AdminServiceError> {
        let config = self.token_manager.config();
        let response = timeout(
            Duration::from_secs(config.update.healthcheck_timeout_seconds),
            self.http_client
                .get(&config.update.healthcheck_url)
                .header(reqwest::header::USER_AGENT, "kiro-rs-admin")
                .send(),
        )
        .await
        .map_err(|_| AdminServiceError::UpstreamError("健康检查超时".to_string()))?
        .map_err(|e| AdminServiceError::UpstreamError(format!("健康检查失败: {}", e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdminServiceError::UpstreamError(format!(
                "健康检查未通过: {} {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn dispatch_detached_docker_job(
        &self,
        job_id: &str,
        operation: &str,
        target_version: Option<String>,
        command_line: String,
        running_message: String,
        success_message: String,
        healthcheck_failed_message: String,
        command_failed_message: String,
    ) -> Result<(), AdminServiceError> {
        let command_line = command_line.trim();
        if command_line.is_empty() {
            return Err(AdminServiceError::InvalidCredential(format!(
                "未配置可执行的{}命令",
                if operation == "update" {
                    "更新"
                } else {
                    "重启"
                }
            )));
        }

        let job_file = self.system_job_path().ok_or_else(|| {
            AdminServiceError::InternalError("系统任务状态文件路径未知".to_string())
        })?;
        let helper_name = format!("kiro-rs-admin-job-{}", &job_id[..job_id.len().min(12)]);
        let healthcheck_url = self.token_manager.config().update.healthcheck_url.clone();
        let healthcheck_timeout = self
            .token_manager
            .config()
            .update
            .healthcheck_timeout_seconds
            .max(5);
        let running_job =
            self.build_running_job(job_id, operation, target_version, running_message)?;
        let success_job =
            self.build_finished_job(&running_job, "succeeded", success_message, false)?;
        let health_failed_job =
            self.build_finished_job(&running_job, "failed", healthcheck_failed_message, true)?;
        let command_failed_job =
            self.build_finished_job(&running_job, "failed", command_failed_message, true)?;

        let success_payload = serde_json::to_string_pretty(&success_job).map_err(|e| {
            AdminServiceError::InternalError(format!("序列化系统任务状态失败: {}", e))
        })?;
        let health_failed_payload =
            serde_json::to_string_pretty(&health_failed_job).map_err(|e| {
                AdminServiceError::InternalError(format!("序列化系统任务状态失败: {}", e))
            })?;
        let command_failed_payload =
            serde_json::to_string_pretty(&command_failed_job).map_err(|e| {
                AdminServiceError::InternalError(format!("序列化系统任务状态失败: {}", e))
            })?;

        self.persist_system_job(Some(&running_job));
        self.system_jobs
            .lock()
            .insert(running_job.job_id.clone(), running_job);

        let helper_script = format!(
            r#"set -eu
CURRENT_CONTAINER="$(hostname)"
HELPER_IMAGE="$(docker inspect "$CURRENT_CONTAINER" --format '{{{{.Config.Image}}}}')"
docker rm -f "{helper_name}" >/dev/null 2>&1 || true
docker run -d --rm --name "{helper_name}" --volumes-from "$CURRENT_CONTAINER" --network host "$HELPER_IMAGE" sh -lc '
set -eu
JOB_FILE="{job_file}"
SUCCESS_FILE="$(mktemp)"
HEALTH_FAILED_FILE="$(mktemp)"
COMMAND_FAILED_FILE="$(mktemp)"
cat <<'"'"'EOF_SUCCESS'"'"' > "$SUCCESS_FILE"
{success_payload}
EOF_SUCCESS
cat <<'"'"'EOF_HEALTH_FAILED'"'"' > "$HEALTH_FAILED_FILE"
{health_failed_payload}
EOF_HEALTH_FAILED
cat <<'"'"'EOF_COMMAND_FAILED'"'"' > "$COMMAND_FAILED_FILE"
{command_failed_payload}
EOF_COMMAND_FAILED
if {command_line}; then
  i=0
  while [ "$i" -lt {healthcheck_timeout} ]; do
    if (command -v curl >/dev/null 2>&1 && curl -fsS "{healthcheck_url}" >/dev/null 2>&1) || (command -v wget >/dev/null 2>&1 && wget -qO- "{healthcheck_url}" >/dev/null 2>&1); then
      cp "$SUCCESS_FILE" "$JOB_FILE"
      exit 0
    fi
    i=$((i + 1))
    sleep 1
  done
  cp "$HEALTH_FAILED_FILE" "$JOB_FILE"
  exit 1
fi
cp "$COMMAND_FAILED_FILE" "$JOB_FILE"
exit 1
'"#,
            helper_name = helper_name,
            job_file = job_file.display(),
            success_payload = success_payload,
            health_failed_payload = health_failed_payload,
            command_failed_payload = command_failed_payload,
            command_line = command_line,
            healthcheck_timeout = healthcheck_timeout,
            healthcheck_url = healthcheck_url,
        );

        let status = Command::new("sh")
            .arg("-lc")
            .arg(helper_script)
            .status()
            .await
            .map_err(|e| AdminServiceError::InternalError(format!("提交后台任务失败: {}", e)))?;
        if !status.success() {
            return Err(AdminServiceError::InternalError(format!(
                "提交后台任务失败，退出码: {:?}",
                status.code()
            )));
        }
        Ok(())
    }

    fn resolve_workspace_path(&self, relative: &str) -> Result<PathBuf, AdminServiceError> {
        let config_path = self
            .token_manager
            .config()
            .config_path()
            .ok_or_else(|| AdminServiceError::InternalError("配置文件路径未知".to_string()))?;
        let workspace_root = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let candidate = workspace_root.join(relative);
        let normalized = candidate
            .components()
            .fold(PathBuf::new(), |mut acc, component| {
                match component {
                    std::path::Component::ParentDir => {
                        acc.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => acc.push(other.as_os_str()),
                }
                acc
            });
        if !normalized.starts_with(&workspace_root) {
            return Err(AdminServiceError::InvalidCredential(format!(
                "路径越界，禁止写入工作区外: {}",
                relative
            )));
        }
        Ok(normalized)
    }

    fn is_docker_deployment(&self) -> bool {
        self.token_manager.config().update.deployment_mode.trim() == "docker"
    }

    // ============ 余额缓存持久化 ============

    fn load_balance_cache_from(cache_path: &Option<PathBuf>) -> HashMap<u64, CachedBalance> {
        let path = match cache_path {
            Some(p) => p,
            None => return HashMap::new(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };

        // 文件中使用字符串 key 以兼容 JSON 格式
        let map: HashMap<String, CachedBalance> = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("解析余额缓存失败，将忽略: {}", e);
                return HashMap::new();
            }
        };

        let now = Utc::now().timestamp() as f64;
        map.into_iter()
            .filter_map(|(k, v)| {
                let id = k.parse::<u64>().ok()?;
                // 丢弃超过 TTL 的条目
                if (now - v.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    Some((id, v))
                } else {
                    None
                }
            })
            .collect()
    }

    fn save_balance_cache(&self) {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return,
        };

        // 持有锁期间完成序列化和写入，防止并发损坏
        let cache = self.balance_cache.lock();
        let map: HashMap<String, &CachedBalance> =
            cache.iter().map(|(k, v)| (k.to_string(), v)).collect();

        match serde_json::to_string_pretty(&map) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::warn!("保存余额缓存失败: {}", e);
                }
            }
            Err(e) => tracing::warn!("序列化余额缓存失败: {}", e),
        }
    }

    // ============ 错误分类 ============

    /// 分类简单操作错误（set_disabled, set_priority, reset_and_enable）
    fn classify_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类余额查询错误（可能涉及上游 API 调用）
    fn classify_balance_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();

        // 1. 凭据不存在
        if msg.contains("不存在") {
            return AdminServiceError::NotFound { id };
        }

        // 2. API Key 凭据不支持刷新：客户端请求错误，映射为 400
        if msg.contains("API Key 凭据不支持刷新") {
            return AdminServiceError::InvalidCredential(msg);
        }

        // 3. 上游服务错误特征：HTTP 响应错误或网络错误
        let is_upstream_error =
            // HTTP 响应错误（来自 refresh_*_token 的错误消息）
            msg.contains("凭证已过期或无效") ||
            msg.contains("权限不足") ||
            msg.contains("已被限流") ||
            msg.contains("服务器错误") ||
            msg.contains("Token 刷新失败") ||
            msg.contains("暂时不可用") ||
            // 网络错误（reqwest 错误）
            msg.contains("error trying to connect") ||
            msg.contains("connection") ||
            msg.contains("timeout") ||
            msg.contains("timed out");

        if is_upstream_error {
            AdminServiceError::UpstreamError(msg)
        } else {
            // 4. 默认归类为内部错误（本地验证失败、配置错误等）
            // 包括：缺少 refreshToken、refreshToken 已被截断、无法生成 machineId 等
            AdminServiceError::InternalError(msg)
        }
    }

    fn classify_test_credential_error(
        &self,
        e: anyhow::Error,
        id: u64,
        model_id: &str,
    ) -> AdminServiceError {
        let msg = e.to_string();

        if msg.contains("不存在") {
            return AdminServiceError::NotFound { id };
        }

        if msg.contains("当前没有可直接调度的凭据") || msg.contains("当前没有可继续切换的凭据")
        {
            return AdminServiceError::InternalError(format!(
                "账号 #{} 当前不能直接测试，请检查该账号是否支持模型 {}，或是否处于本地阻塞/刷新异常状态",
                id, model_id
            ));
        }

        if msg.contains("API 请求失败: 429") || msg.contains("流式 API 请求失败: 429") {
            return AdminServiceError::UpstreamError(format!(
                "账号 #{} 测试失败：上游已限频，模型 {} 当前不可立即调用",
                id, model_id
            ));
        }

        if msg.contains("API 请求失败: 401")
            || msg.contains("流式 API 请求失败: 401")
            || msg.contains("API 请求失败: 403")
            || msg.contains("流式 API 请求失败: 403")
        {
            return AdminServiceError::UpstreamError(format!(
                "账号 #{} 测试失败：该账号当前无权限调用模型 {}，或登录状态已失效",
                id, model_id
            ));
        }

        if msg.contains("API 请求失败: 402") || msg.contains("流式 API 请求失败: 402") {
            return AdminServiceError::UpstreamError(format!(
                "账号 #{} 测试失败：该账号当前额度不足，无法调用模型 {}",
                id, model_id
            ));
        }

        self.classify_balance_error(e, id)
    }

    /// 分类添加凭据错误
    fn classify_add_error(&self, e: anyhow::Error) -> AdminServiceError {
        let msg = e.to_string();

        // 凭据验证失败（refreshToken 无效、格式错误等）
        let is_invalid_credential = msg.contains("缺少 refreshToken")
            || msg.contains("refreshToken 为空")
            || msg.contains("refreshToken 已被截断")
            || msg.contains("凭据已存在")
            || msg.contains("refreshToken 重复")
            || msg.contains("kiroApiKey 重复")
            || msg.contains("缺少 kiroApiKey")
            || msg.contains("kiroApiKey 为空")
            || msg.contains("凭证已过期或无效")
            || msg.contains("权限不足")
            || msg.contains("已被限流");

        if is_invalid_credential {
            AdminServiceError::InvalidCredential(msg)
        } else if msg.contains("error trying to connect")
            || msg.contains("connection")
            || msg.contains("timeout")
        {
            AdminServiceError::UpstreamError(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类删除凭据错误
    fn classify_delete_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else {
            AdminServiceError::InternalError(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_build_default_version_response_uses_new_capabilities() {
        let mut config = Config::default();
        config.update.enabled = true;
        config.update.build_type = "release".to_string();
        config.update.deployment_mode = "docker".to_string();
        config.update.update_command = "docker compose pull && docker compose up -d".to_string();
        config.update.restart_command = "docker restart kiro-rs".to_string();

        let response = AdminService::build_default_version_response(
            &config,
            "1.0.0".to_string(),
            Some("abc123".to_string()),
            None,
        );

        assert_eq!(response.build_type, "release");
        assert_eq!(response.deployment_mode, "docker");
        assert!(response.can_update);
        assert!(!response.can_rollback);
        assert!(response.can_restart);
        assert!(response.update_hint.contains("测试容器"));
    }

    #[test]
    fn test_build_default_version_response_for_source_build() {
        let mut config = Config::default();
        config.update.enabled = true;
        config.update.build_type = "source".to_string();

        let response =
            AdminService::build_default_version_response(&config, "1.0.0".to_string(), None, None);

        assert_eq!(response.build_type, "source");
        assert!(!response.can_update);
        assert!(!response.can_rollback);
        assert!(!response.can_restart);
    }
}
