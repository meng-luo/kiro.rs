//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试
//! 支持按凭据级 endpoint 切换不同 Kiro API 端点

use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::diagnostics::RequestDiagnosticUpdate;
use crate::kiro::endpoint::{KiroEndpoint, RequestContext};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::{
    AcquireOptions, CallContext, DispatchLease, MultiTokenManager, RateLimitKind,
};
use crate::model::config::TlsBackend;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

/// 每个凭据的最大重试次数
const MAX_RETRIES_PER_CREDENTIAL: usize = 3;

/// 总重试次数硬上限（避免无限重试）
const MAX_TOTAL_RETRIES: usize = 9;

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
/// 按凭据 `endpoint` 字段选择 [`KiroEndpoint`] 实现
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    /// 全局代理配置（用于凭据无自定义代理时的回退）
    global_proxy: Option<ProxyConfig>,
    /// Client 缓存：key = effective proxy config, value = reqwest::Client
    /// 不同代理配置的凭据使用不同的 Client，共享相同代理的凭据复用 Client
    client_cache: Mutex<HashMap<Option<ProxyConfig>, Client>>,
    /// TLS 后端配置
    tls_backend: TlsBackend,
    /// 端点实现注册表（key: endpoint 名称）
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    /// 默认端点名称（凭据未指定 endpoint 时使用）
    default_endpoint: String,
}

pub struct ProviderResponse {
    pub response: reqwest::Response,
    pub lease: DispatchLease,
    pub request_id: String,
    pub dispatch_path: String,
    pub used_soft_fallback: bool,
    pub account_state_at_start: String,
}

impl KiroProvider {
    pub fn token_manager(&self) -> &Arc<MultiTokenManager> {
        &self.token_manager
    }

    /// 创建带代理配置和端点注册表的 KiroProvider 实例
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器
    /// * `proxy` - 全局代理配置
    /// * `endpoints` - 端点名 → 实现的注册表（至少包含 `default_endpoint` 对应条目）
    /// * `default_endpoint` - 凭据未显式指定 endpoint 时使用的名称
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        assert!(
            endpoints.contains_key(&default_endpoint),
            "默认端点 {} 未在 endpoints 注册表中",
            default_endpoint
        );
        let tls_backend = token_manager.config().tls_backend;
        // 预热：构建全局代理对应的 Client
        let initial_client =
            build_client(proxy.as_ref(), 720, tls_backend).expect("创建 HTTP 客户端失败");
        let mut cache = HashMap::new();
        cache.insert(proxy.clone(), initial_client);

        Self {
            token_manager,
            global_proxy: proxy,
            client_cache: Mutex::new(cache),
            tls_backend,
            endpoints,
            default_endpoint,
        }
    }

    /// 根据凭据的代理配置获取（或创建并缓存）对应的 reqwest::Client
    fn client_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Client> {
        let effective = credentials.effective_proxy(self.global_proxy.as_ref());
        let mut cache = self.client_cache.lock();
        if let Some(client) = cache.get(&effective) {
            return Ok(client.clone());
        }
        let client = build_client(effective.as_ref(), 720, self.tls_backend)?;
        cache.insert(effective, client.clone());
        Ok(client)
    }

    /// 根据凭据选择 endpoint 实现
    fn endpoint_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
        let name = credentials
            .endpoint
            .as_deref()
            .unwrap_or(&self.default_endpoint);
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移（见 [`Self::call_api_with_retry`]）
    #[allow(dead_code)]
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry(request_body, false, None, None)
            .await
    }

    pub async fn call_api_with_metadata(
        &self,
        request_body: &str,
        original_model: Option<&str>,
        input_tokens: Option<i32>,
    ) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry(request_body, false, original_model, input_tokens)
            .await
    }

    /// 发送流式 API 请求
    #[allow(dead_code)]
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry(request_body, true, None, None)
            .await
    }

    pub async fn call_api_stream_with_metadata(
        &self,
        request_body: &str,
        original_model: Option<&str>,
        input_tokens: Option<i32>,
    ) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry(request_body, true, original_model, input_tokens)
            .await
    }

    #[allow(dead_code)]
    pub async fn call_api_stream_for_account(
        &self,
        request_body: &str,
        options: AcquireOptions,
    ) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry_and_options(request_body, true, Some(options), None, None)
            .await
    }

    pub async fn call_api_stream_for_account_with_metadata(
        &self,
        request_body: &str,
        options: AcquireOptions,
        original_model: Option<&str>,
        input_tokens: Option<i32>,
    ) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry_and_options(
            request_body,
            true,
            Some(options),
            original_model,
            input_tokens,
        )
        .await
    }

    /// 发送 MCP API 请求（WebSearch 等工具调用）
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_mcp_with_retry(request_body).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
            let ctx = match self.token_manager.acquire_context(None).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    // endpoint 解析失败：记为失败，换下一张凭据
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.mcp_url(&rctx);
            let body = endpoint.transform_mcp_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_mcp(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "MCP 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 额度用尽
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 400 Bad Request
            if status.as_u16() == 400 {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 瞬态错误
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                tracing::warn!(
                    "MCP 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx
            if status.is_client_error() {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 兜底
            last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("MCP 请求失败：已达到最大重试次数（{}次）", max_retries)
        }))
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - 每个凭据最多重试 MAX_RETRIES_PER_CREDENTIAL 次
    /// - 总重试次数 = min(凭据数量 × 每凭据重试次数, MAX_TOTAL_RETRIES)
    /// - 硬上限 9 次，避免无限重试
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
        original_model: Option<&str>,
        input_tokens: Option<i32>,
    ) -> anyhow::Result<ProviderResponse> {
        self.call_api_with_retry_and_options(
            request_body,
            is_stream,
            None,
            original_model,
            input_tokens,
        )
        .await
    }

    async fn call_api_with_retry_and_options(
        &self,
        request_body: &str,
        is_stream: bool,
        base_options: Option<AcquireOptions>,
        original_model: Option<&str>,
        input_tokens: Option<i32>,
    ) -> anyhow::Result<ProviderResponse> {
        let request_id = Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now();
        let started_instant = Instant::now();
        let total_credentials = self.token_manager.total_count();
        let max_retries = (total_credentials * MAX_RETRIES_PER_CREDENTIAL).min(MAX_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();
        let api_type = if is_stream { "流式" } else { "非流式" };

        // 尝试从请求体中提取模型信息
        let model = Self::extract_model_from_request(request_body);
        let original_model = original_model
            .map(|m| m.to_string())
            .or_else(|| model.clone());
        let session_key = Self::extract_session_key(request_body);
        let session_hash = session_key.as_deref().map(Self::hash_short);
        let mut tried_account_ids: HashSet<u64> = HashSet::new();
        let mut base_options = base_options.unwrap_or_else(|| AcquireOptions::new(model.clone()));
        if base_options.model.is_none() {
            base_options.model = model.clone();
        }
        if base_options.session_key.is_none() {
            base_options.session_key = session_key.clone();
        }
        let strict_preferred_account = base_options.strict_preferred_account;

        for attempt in 0..max_retries {
            // 获取调用上下文（绑定 index、credentials、token）
            let mut acquire_options = base_options.clone();
            acquire_options.tried_account_ids = tried_account_ids.clone();
            let mut ctx = match self
                .token_manager
                .acquire_context_with_options(acquire_options)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    self.record_api_diagnostic(RequestDiagnosticUpdate {
                        request_id: request_id.clone(),
                        started_at,
                        finished_at: chrono::Utc::now(),
                        duration_ms: started_instant.elapsed().as_millis() as u64,
                        original_model: original_model.clone(),
                        mapped_model: model.clone(),
                        success: false,
                        upstream_error_code: Some("acquire_context_failed".to_string()),
                        upstream_message_short: Some(Self::short_message(&e.to_string())),
                        input_tokens,
                        ..Default::default()
                    });
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    if let Some(mut lease) = ctx.lease.take() {
                        self.token_manager.release_slot(&mut lease);
                    }
                    let message = e.to_string();
                    last_error = Some(anyhow::anyhow!(message.clone()));
                    self.token_manager.report_failure(ctx.id);
                    self.record_api_diagnostic(RequestDiagnosticUpdate {
                        request_id: request_id.clone(),
                        started_at,
                        finished_at: chrono::Utc::now(),
                        duration_ms: started_instant.elapsed().as_millis() as u64,
                        original_model: original_model.clone(),
                        mapped_model: model.clone(),
                        credential_id: Some(ctx.id),
                        dispatch_path: Some(ctx.dispatch_path.to_string()),
                        sticky_hit: ctx.dispatch_path.to_string() == "sticky",
                        sticky_detached: false,
                        session_hash: session_hash.clone(),
                        success: false,
                        upstream_error_code: Some("endpoint_config_error".to_string()),
                        upstream_message_short: Some(Self::short_message(&message)),
                        input_tokens,
                        ..Default::default()
                    });
                    tried_account_ids.insert(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.api_url(&rctx);
            let body = endpoint.transform_api_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_api(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    if let Some(mut lease) = ctx.lease.take() {
                        self.token_manager.release_slot(&mut lease);
                    }
                    tracing::warn!(
                        "API 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    // 网络错误通常是上游/链路瞬态问题，不应导致"禁用凭据"或"切换凭据"
                    // （否则一段时间网络抖动会把所有凭据都误禁用，需要重启才能恢复）
                    let message = e.to_string();
                    self.record_api_diagnostic(RequestDiagnosticUpdate {
                        request_id: request_id.clone(),
                        started_at,
                        finished_at: chrono::Utc::now(),
                        duration_ms: started_instant.elapsed().as_millis() as u64,
                        original_model: original_model.clone(),
                        mapped_model: model.clone(),
                        credential_id: Some(ctx.id),
                        dispatch_path: Some(ctx.dispatch_path.to_string()),
                        sticky_hit: ctx.dispatch_path.to_string() == "sticky",
                        sticky_detached: false,
                        session_hash: session_hash.clone(),
                        success: false,
                        upstream_error_code: Some("send_failed".to_string()),
                        upstream_message_short: Some(Self::short_message(&message)),
                        input_tokens,
                        ..Default::default()
                    });
                    last_error = Some(anyhow::anyhow!(message));
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                let lease = ctx
                    .lease
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("缺少调度租约"))?;
                self.record_api_diagnostic(RequestDiagnosticUpdate {
                    request_id: request_id.clone(),
                    started_at,
                    finished_at: chrono::Utc::now(),
                    duration_ms: started_instant.elapsed().as_millis() as u64,
                    original_model: original_model.clone(),
                    mapped_model: model.clone(),
                    credential_id: Some(ctx.id),
                    dispatch_path: Some(ctx.dispatch_path.to_string()),
                    sticky_hit: ctx.dispatch_path.to_string() == "sticky",
                    sticky_detached: false,
                    session_hash: session_hash.clone(),
                    success: true,
                    upstream_status: Some(status.as_u16()),
                    input_tokens,
                    ..Default::default()
                });
                return Ok(ProviderResponse {
                    response,
                    lease,
                    request_id: request_id.clone(),
                    dispatch_path: ctx.dispatch_path.to_string(),
                    used_soft_fallback: ctx.used_soft_fallback,
                    account_state_at_start: ctx.account_state_at_start.to_string(),
                });
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();
            if let Some(mut lease) = ctx.lease.take() {
                self.token_manager.release_slot(&mut lease);
            }

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                tracing::warn!(
                    "API 请求失败（额度已用尽，禁用凭据并切换，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                self.record_api_diagnostic(Self::failure_diagnostic(
                    &request_id,
                    started_at,
                    started_instant,
                    original_model.clone(),
                    model.clone(),
                    input_tokens,
                    &session_hash,
                    &ctx,
                    status.as_u16(),
                    "quota_exhausted",
                    &body,
                    None,
                ));
                let err = anyhow::anyhow!("{} API 请求失败: {} {}", api_type, status, body);
                if strict_preferred_account {
                    return Err(err);
                }
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }
                last_error = Some(err);
                tried_account_ids.insert(ctx.id);
                continue;
            }

            // 400 Bad Request - 请求问题，重试/切换凭据无意义
            if status.as_u16() == 400 {
                self.record_api_diagnostic(Self::failure_diagnostic(
                    &request_id,
                    started_at,
                    started_instant,
                    original_model.clone(),
                    model.clone(),
                    input_tokens,
                    &session_hash,
                    &ctx,
                    status.as_u16(),
                    "bad_request",
                    &body,
                    None,
                ));
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 401/403 - 更可能是凭据/权限问题：计入失败并允许故障转移
            if matches!(status.as_u16(), 401 | 403) {
                tracing::warn!(
                    "API 请求失败（可能为凭据错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self
                        .token_manager
                        .force_refresh_token_for(ctx.id)
                        .await
                        .is_ok()
                    {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                self.record_api_diagnostic(Self::failure_diagnostic(
                    &request_id,
                    started_at,
                    started_instant,
                    original_model.clone(),
                    model.clone(),
                    input_tokens,
                    &session_hash,
                    &ctx,
                    status.as_u16(),
                    "credential_error",
                    &body,
                    None,
                ));
                let err = anyhow::anyhow!("{} API 请求失败: {} {}", api_type, status, body);
                if strict_preferred_account {
                    return Err(err);
                }
                if !has_available {
                    anyhow::bail!(
                        "{} API 请求失败（所有凭据已用尽）: {} {}",
                        api_type,
                        status,
                        body
                    );
                }

                last_error = Some(err);
                tried_account_ids.insert(ctx.id);
                continue;
            }

            if status.as_u16() == 429 {
                let kind = if Self::is_suspicious_activity(&body) {
                    RateLimitKind::SuspiciousActivity
                } else {
                    RateLimitKind::Normal429
                };
                self.token_manager.report_rate_limited(ctx.id, kind);
                let kind_text = Self::rate_limit_kind_label(kind);
                self.record_api_diagnostic(Self::failure_diagnostic(
                    &request_id,
                    started_at,
                    started_instant,
                    original_model.clone(),
                    model.clone(),
                    input_tokens,
                    &session_hash,
                    &ctx,
                    status.as_u16(),
                    kind_text,
                    &body,
                    Some(kind_text.to_string()),
                ));
                let err = anyhow::anyhow!("{} API 请求失败: {} {}", api_type, status, body);
                if strict_preferred_account {
                    return Err(err);
                }
                tried_account_ids.insert(ctx.id);
                last_error = Some(err);
                continue;
            }

            // 408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            if status.as_u16() == 408 || status.is_server_error() {
                tracing::warn!(
                    "API 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                self.record_api_diagnostic(Self::failure_diagnostic(
                    &request_id,
                    started_at,
                    started_instant,
                    original_model.clone(),
                    model.clone(),
                    input_tokens,
                    &session_hash,
                    &ctx,
                    status.as_u16(),
                    "upstream_transient",
                    &body,
                    None,
                ));
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                self.record_api_diagnostic(Self::failure_diagnostic(
                    &request_id,
                    started_at,
                    started_instant,
                    original_model.clone(),
                    model.clone(),
                    input_tokens,
                    &session_hash,
                    &ctx,
                    status.as_u16(),
                    "client_error",
                    &body,
                    None,
                ));
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 兜底：当作可重试的瞬态错误处理（不切换凭据）
            tracing::warn!(
                "API 请求失败（未知错误，尝试 {}/{}）: {} {}",
                attempt + 1,
                max_retries,
                status,
                body
            );
            self.record_api_diagnostic(Self::failure_diagnostic(
                &request_id,
                started_at,
                started_instant,
                original_model.clone(),
                model.clone(),
                input_tokens,
                &session_hash,
                &ctx,
                status.as_u16(),
                "unknown_error",
                &body,
                None,
            ));
            last_error = Some(anyhow::anyhow!(
                "{} API 请求失败: {} {}",
                api_type,
                status,
                body
            ));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        // 所有重试都失败
        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{} API 请求失败：已达到最大重试次数（{}次）",
                api_type,
                max_retries
            )
        }))
    }

    /// 从请求体中提取模型信息
    ///
    /// 尝试解析 JSON 请求体，提取 conversationState.currentMessage.userInputMessage.modelId
    fn extract_model_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        json.get("conversationState")?
            .get("currentMessage")?
            .get("userInputMessage")?
            .get("modelId")?
            .as_str()
            .map(|s| s.to_string())
    }

    fn extract_session_key(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;
        json.get("conversationState")?
            .get("conversationId")?
            .as_str()
            .map(|s| s.to_string())
    }

    fn is_suspicious_activity(body: &str) -> bool {
        let lower = body.to_lowercase();
        lower.contains("suspicious activity")
            || lower.contains("due to suspicious activity")
            || lower.contains("suspicious_activity")
    }

    fn record_api_diagnostic(&self, update: RequestDiagnosticUpdate) {
        tracing::info!(
            request_id = %update.request_id,
            credential_id = update.credential_id,
            original_model = update.original_model.as_deref().unwrap_or("-"),
            mapped_model = update.mapped_model.as_deref().unwrap_or("-"),
            dispatch_path = update.dispatch_path.as_deref().unwrap_or("-"),
            success = update.success,
            upstream_status = update.upstream_status,
            rate_limit_kind = update.rate_limit_kind.as_deref().unwrap_or("-"),
            duration_ms = update.duration_ms,
            "API 请求诊断事件"
        );
        self.token_manager.record_diagnostic(update);
    }

    fn failure_diagnostic(
        request_id: &str,
        started_at: chrono::DateTime<chrono::Utc>,
        started_instant: Instant,
        original_model: Option<String>,
        mapped_model: Option<String>,
        input_tokens: Option<i32>,
        session_hash: &Option<String>,
        ctx: &CallContext,
        status: u16,
        error_code: &str,
        body: &str,
        rate_limit_kind: Option<String>,
    ) -> RequestDiagnosticUpdate {
        RequestDiagnosticUpdate {
            request_id: request_id.to_string(),
            started_at,
            finished_at: chrono::Utc::now(),
            duration_ms: started_instant.elapsed().as_millis() as u64,
            original_model,
            mapped_model,
            credential_id: Some(ctx.id),
            dispatch_path: Some(ctx.dispatch_path.to_string()),
            sticky_hit: ctx.dispatch_path.to_string() == "sticky",
            sticky_detached: rate_limit_kind.as_deref() == Some("suspicious_activity"),
            session_hash: session_hash.clone(),
            success: false,
            upstream_status: Some(status),
            upstream_error_code: Some(error_code.to_string()),
            upstream_message_short: Some(Self::short_message(body)),
            rate_limit_kind,
            input_tokens,
            ..Default::default()
        }
    }

    fn short_message(body: &str) -> String {
        let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
        collapsed.chars().take(300).collect()
    }

    fn hash_short(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        hex::encode(hasher.finalize())[..16].to_string()
    }

    fn rate_limit_kind_label(kind: RateLimitKind) -> &'static str {
        match kind {
            RateLimitKind::Normal429 => "normal_429",
            RateLimitKind::SuspiciousActivity => "suspicious_activity",
            RateLimitKind::Refresh429 => "refresh_429",
        }
    }

    fn retry_delay(attempt: usize) -> Duration {
        // 指数退避 + 少量抖动，避免上游抖动时放大故障
        const BASE_MS: u64 = 200;
        const MAX_MS: u64 = 2_000;
        let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(MAX_MS);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }
}
