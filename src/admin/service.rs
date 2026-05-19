//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::model::events::Event;
use crate::kiro::model::requests::conversation::{ConversationState, CurrentMessage, UserInputMessage};
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::provider::KiroProvider;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::{AcquireOptions, MultiTokenManager};
use crate::model::config::Config;

use super::error::AdminServiceError;
use super::types::{
    AddCredentialRequest, AddCredentialResponse, BalanceResponse, CredentialStatusItem, CredentialTestRequest,
    CredentialsStatusResponse, LoadBalancingModeResponse, SetLoadBalancingModeRequest,
    SetMaxConcurrentRequest, SystemOperationJobResponse, SystemRollbackRequest, SystemUpdateRequest,
    SystemVersionResponse,
};

pub type TestEventStream =
    std::pin::Pin<Box<dyn Stream<Item = Result<serde_json::Value, AdminServiceError>> + Send>>;

/// 余额缓存过期时间（秒），5 分钟
const BALANCE_CACHE_TTL_SECS: i64 = 300;
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
}

impl AdminService {
    pub fn new(
        token_manager: Arc<MultiTokenManager>,
        provider: Arc<KiroProvider>,
        known_endpoints: impl IntoIterator<Item = String>,
    ) -> Self {
        let cache_path = token_manager
            .cache_dir()
            .map(|d| d.join("kiro_balance_cache.json"));

        let balance_cache = Self::load_balance_cache_from(&cache_path);
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        let current_commit = Self::detect_current_commit();
        let initial_version_state = CachedVersionState {
            response: Self::build_default_version_response(
                &token_manager.config(),
                current_version.clone(),
                current_commit.clone(),
                None,
            ),
        };
        let http_client = Self::build_admin_http_client(token_manager.config()).unwrap_or_else(|e| {
            tracing::warn!("构建版本治理 HTTP 客户端失败，回退到默认客户端: {}", e);
            Client::builder()
                .build()
                .expect("创建默认 HTTP 客户端失败")
        });

        Self {
            token_manager,
            provider,
            balance_cache: Mutex::new(balance_cache),
            cache_path,
            known_endpoints: known_endpoints.into_iter().collect(),
            current_version,
            current_commit,
            version_state: Mutex::new(initial_version_state),
            system_jobs: Mutex::new(HashMap::new()),
            http_client,
        }
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();
        let default_endpoint = self.token_manager.config().default_endpoint.clone();

        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| CredentialStatusItem {
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
                success_count: entry.success_count,
                last_used_at: entry.last_used_at.clone(),
                has_proxy: entry.has_proxy,
                proxy_url: entry.proxy_url,
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
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            current_id: snapshot.current_id,
            credentials,
        }
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

    /// 获取系统版本信息
    pub fn get_system_version(&self) -> SystemVersionResponse {
        self.version_state.lock().response.clone()
    }

    /// 重新检查系统版本信息
    pub async fn check_system_version(&self) -> Result<SystemVersionResponse, AdminServiceError> {
        let latest_job = self.latest_job();
        let latest_release = self.fetch_latest_release().await?;
        let response = self.build_version_response(latest_release, latest_job);
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
                "当前实例未启用在线更新，请先在配置中开启 update.enabled".to_string(),
            ));
        }
        let version_info = self.check_system_version().await?;
        let target_version = payload
            .version
            .clone()
            .or_else(|| version_info.update_available.then_some(version_info.latest_version.clone()))
            .or_else(|| Some(version_info.current_version.clone()));
        let target_version = target_version.filter(|value| !value.trim().is_empty());
        let job = self.create_job(
            "update",
            target_version.clone(),
            format!(
                "准备更新到 {}",
                target_version.clone().unwrap_or_else(|| "当前版本".to_string())
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
        let job = self.create_job("restart", None, "准备重启当前实例".to_string());
        let job_id = job.job_id.clone();
        let service = self.clone();
        tokio::spawn(async move {
            service.run_restart_job(job_id).await;
        });
        Ok(job)
    }

    pub fn get_system_job(&self, id: &str) -> Result<SystemOperationJobResponse, AdminServiceError> {
        self.system_jobs
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AdminServiceError::InvalidCredential(format!("任务不存在: {}", id)))
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
            .call_api_stream_for_account(&request_body, options)
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
        let proxy = config.update.proxy_url.as_ref().map(|url| ProxyConfig::new(url.clone()));
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
        let deployment_mode = if !config.update.current_deployment_mode.trim().is_empty() {
            config.update.current_deployment_mode.clone()
        } else if std::path::Path::new("/.dockerenv").exists() {
            "docker".to_string()
        } else {
            "binary".to_string()
        };
        let can_self_update = config.update.enabled
            && config.update.restart_mode == "command"
            && !config.update.restart_command.trim().is_empty()
            && deployment_mode == "binary";
        SystemVersionResponse {
            current_version: current_version.clone(),
            latest_version: current_version,
            update_available: false,
            latest_published_at: None,
            release_notes_url: None,
            deployment_mode,
            can_self_update,
            update_hint: if can_self_update {
                "当前实例允许从管理台发起下载、替换和重启。".to_string()
            } else if std::path::Path::new("/.dockerenv").exists() {
                "当前实例运行在容器内，请通过镜像或容器编排更新。".to_string()
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
                    "发现新版本 {}，可在这里发起更新并查看最近任务状态。",
                    latest_version
                )
            } else if response.can_self_update {
                "当前已经是最新版本，可继续在这里执行重启或回滚。".to_string()
            } else {
                response.update_hint
            };
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
            .map_err(|e| AdminServiceError::UpstreamError(format!("检查 GitHub Release 失败: {}", e)))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdminServiceError::UpstreamError(format!(
                "GitHub Release 检查失败: {} {}",
                status, body
            )));
        }
        let releases: Vec<GitHubRelease> = response
            .json()
            .await
            .map_err(|e| AdminServiceError::UpstreamError(format!("解析 GitHub Release 失败: {}", e)))?;
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
        Some(cloned)
    }

    fn latest_job(&self) -> Option<SystemOperationJobResponse> {
        self.system_jobs
            .lock()
            .values()
            .cloned()
            .max_by(|left, right| left.started_at.cmp(&right.started_at))
    }

    fn sync_latest_job(&self, latest_job: Option<SystemOperationJobResponse>) {
        self.version_state.lock().response.latest_job = latest_job;
    }

    async fn run_update_job(self: Arc<Self>, job_id: String, target_version: Option<String>) {
        let result = async {
            let release = self
                .fetch_latest_release()
                .await?
                .ok_or_else(|| AdminServiceError::UpstreamError("未获取到可用发布版本".to_string()))?;
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
            .map_err(|e| AdminServiceError::InternalError(format!("序列化 release 元信息失败: {}", e)))?;
            tokio::fs::write(&release_file, release_payload)
                .await
                .map_err(|e| AdminServiceError::InternalError(format!("写入 release 元信息失败: {}", e)))?;
            let backup_dir = self.create_backup().await?;
            self.run_command_from_config("update").await?;
            self.run_healthcheck().await?;
            Ok::<String, AdminServiceError>(format!(
                "更新流程已执行，目标版本 {}，备份目录 {}",
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
            self.run_command_from_config("rollback").await?;
            self.run_healthcheck().await?;
            Ok::<String, AdminServiceError>(format!("已回滚到备份 {}", backup_dir.display()))
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
        let result = async {
            self.run_command_from_config("restart").await?;
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
            tokio::fs::copy(&current_exe, &target)
                .await
                .map_err(|e| AdminServiceError::InternalError(format!("备份当前二进制失败: {}", e)))?;
        }
        self.cleanup_old_backups(&backup_root, config.update.max_backups).await?;
        Ok(backup_dir)
    }

    async fn restore_backup(&self, backup_name: Option<&str>) -> Result<PathBuf, AdminServiceError> {
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
            candidates
                .pop()
                .ok_or_else(|| AdminServiceError::InvalidCredential("当前没有可回滚的备份".to_string()))?
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

    async fn run_command_from_config(&self, operation: &str) -> Result<(), AdminServiceError> {
        let config = self.token_manager.config();
        if config.update.restart_mode != "command" {
            return Err(AdminServiceError::InvalidCredential(format!(
                "当前仅支持 command 重启模式，实际为 {}",
                config.update.restart_mode
            )));
        }
        let command_line = match operation {
            "rollback" if !config.update.rollback_restart_command.trim().is_empty() => {
                config.update.rollback_restart_command.trim()
            }
            _ => config.update.restart_command.trim(),
        };
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
        let normalized = candidate.components().fold(PathBuf::new(), |mut acc, component| {
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

        if msg.contains("当前没有可直接调度的凭据")
            || msg.contains("当前没有可继续切换的凭据")
        {
            return AdminServiceError::InternalError(format!(
                "账号 #{} 当前不能直接测试，请检查该账号是否支持模型 {}，或是否处于本地阻塞/刷新异常状态",
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
        } else if msg.contains("只能删除已禁用的凭据") || msg.contains("请先禁用凭据") {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }
}
