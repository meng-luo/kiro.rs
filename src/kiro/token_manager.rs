//! Token 管理模块
//!
//! 负责 Token 过期检测和刷新，支持 Social 和 IdC 认证方式
//! 支持多凭据 (MultiTokenManager) 管理

use anyhow::bail;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as TokioMutex;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration as StdDuration, Instant};

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::diagnostics::{
    DiagnosticsQuery, DiagnosticsRequestsResponse, DiagnosticsStore, DiagnosticsSummaryResponse,
    RequestDiagnosticEntry, RequestDiagnosticUpdate,
};
use crate::kiro::machine_id;
use crate::kiro::model::available_models::AvailableModelsResponse;
use crate::kiro::model::credentials::KiroCredentials;
pub use crate::kiro::model::credentials::SchedulerPolicy;
use crate::kiro::model::token_refresh::{
    IdcRefreshRequest, IdcRefreshResponse, RefreshRequest, RefreshResponse,
};
use crate::kiro::model::usage_limits::UsageLimitsResponse;
use crate::model::config::Config;
use crate::proxy_pool::ProxyPool;

/// 检查 Token 是否在指定时间内过期
pub(crate) fn is_token_expiring_within(
    credentials: &KiroCredentials,
    minutes: i64,
) -> Option<bool> {
    credentials
        .expires_at
        .as_ref()
        .and_then(|expires_at| DateTime::parse_from_rfc3339(expires_at).ok())
        .map(|expires| expires <= Utc::now() + Duration::minutes(minutes))
}

/// 检查 Token 是否已过期（提前 5 分钟判断）
pub(crate) fn is_token_expired(credentials: &KiroCredentials) -> bool {
    is_token_expiring_within(credentials, 5).unwrap_or(true)
}

/// 检查 Token 是否即将过期（10分钟内）
pub(crate) fn is_token_expiring_soon(credentials: &KiroCredentials) -> bool {
    is_token_expiring_within(credentials, 10).unwrap_or(false)
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

fn normalize_email(value: &str) -> Option<String> {
    let email = value.trim().to_ascii_lowercase();
    if email.is_empty() { None } else { Some(email) }
}

fn credential_email_key(credentials: &KiroCredentials) -> Option<String> {
    credentials.email.as_deref().and_then(normalize_email)
}

/// 生成 API Key 脱敏展示(前 4 + ... + 后 4,长度不足或非 ASCII 回退 ***)
fn mask_api_key(key: &str) -> String {
    if key.is_ascii() && key.len() > 16 {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    } else {
        "***".to_string()
    }
}

const DEFAULT_MAX_CONCURRENT: u32 = 3;
const STICKY_SESSION_TTL_SECS: i64 = 30 * 60;
const KIRO_WEB_PORTAL_API_BASE: &str =
    "https://app.kiro.dev/service/KiroWebPortalService/operation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKind {
    Normal429,
    SuspiciousActivity,
    Refresh429,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchState {
    Ready,
    Saturated,
    Cooldown,
    Blocked,
    Disabled,
}

impl fmt::Display for DispatchState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            DispatchState::Ready => "ready",
            DispatchState::Saturated => "saturated",
            DispatchState::Cooldown => "cooldown",
            DispatchState::Blocked => "blocked",
            DispatchState::Disabled => "disabled",
        };
        write!(f, "{}", value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    Normal,
    Banned,
    RateLimited,
    Disabled,
}

impl fmt::Display for AccountStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            AccountStatus::Normal => "normal",
            AccountStatus::Banned => "banned",
            AccountStatus::RateLimited => "rate_limited",
            AccountStatus::Disabled => "disabled",
        };
        write!(f, "{}", value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchPath {
    Preferred,
    Sticky,
    Balanced,
    SoftFallback,
}

impl fmt::Display for DispatchPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            DispatchPath::Preferred => "preferred",
            DispatchPath::Sticky => "sticky",
            DispatchPath::Balanced => "balanced",
            DispatchPath::SoftFallback => "soft_fallback",
        };
        write!(f, "{}", value)
    }
}

/// 验证 refreshToken 的基本有效性
pub(crate) fn validate_refresh_token(credentials: &KiroCredentials) -> anyhow::Result<()> {
    let refresh_token = credentials
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少 refreshToken"))?;

    if refresh_token.is_empty() {
        bail!("refreshToken 为空");
    }

    if refresh_token.len() < 100 || refresh_token.ends_with("...") || refresh_token.contains("...")
    {
        bail!(
            "refreshToken 已被截断（长度: {} 字符）。\n\
             这通常是 Kiro IDE 为了防止凭证被第三方工具使用而故意截断的。",
            refresh_token.len()
        );
    }

    Ok(())
}

/// Refresh Token 永久失效错误
///
/// 当服务端返回 400 + `invalid_grant` 时，表示 refreshToken 已被撤销或过期，
/// 不应重试，需立即禁用对应凭据。
#[derive(Debug)]
pub(crate) struct RefreshTokenInvalidError {
    pub message: String,
}

impl fmt::Display for RefreshTokenInvalidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RefreshTokenInvalidError {}

/// 刷新 Token
pub(crate) async fn refresh_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    // API Key 凭据不支持 Token 刷新：底层契约级拦截
    // 其他调用点（try_ensure_token / 活跃路径 / add_credential）在调用前已显式分流 API Key；
    // 仅 force_refresh_token_for 未分流，此处 bail 让错误自然传播为 400 BAD_REQUEST。
    if credentials.is_api_key_credential() {
        bail!("API Key 凭据不支持刷新 Token");
    }

    validate_refresh_token(credentials)?;

    // 根据 auth_method 选择刷新方式
    // 如果未指定 auth_method，根据是否有 clientId/clientSecret 自动判断
    let auth_method = credentials.auth_method.as_deref().unwrap_or_else(|| {
        if credentials.client_id.is_some() && credentials.client_secret.is_some() {
            "idc"
        } else {
            "social"
        }
    });

    if auth_method.eq_ignore_ascii_case("idc")
        || auth_method.eq_ignore_ascii_case("builder-id")
        || auth_method.eq_ignore_ascii_case("iam")
    {
        refresh_idc_token(credentials, config, proxy).await
    } else {
        refresh_social_token(credentials, config, proxy).await
    }
}

/// 刷新 Social Token
async fn refresh_social_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 Social Token...");

    let refresh_token = credentials.refresh_token.as_ref().unwrap();
    // 优先级：凭据.auth_region > 凭据.region > config.auth_region > config.region
    let region = credentials.effective_auth_region(config);

    let refresh_url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", region);
    let refresh_domain = format!("prod.{}.auth.desktop.kiro.dev", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config);
    let kiro_version = &config.kiro_version;

    let client = build_client(proxy, 60, config.tls_backend)?;
    let body = RefreshRequest {
        refresh_token: refresh_token.to_string(),
    };

    let response = client
        .post(&refresh_url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            format!("KiroIDE-{}-{}", kiro_version, machine_id),
        )
        .header("Accept-Encoding", "gzip, compress, deflate, br")
        .header("host", &refresh_domain)
        .header("Connection", "close")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();

        // 400 + invalid_grant + Invalid refresh token provided → refreshToken 永久失效
        if status.as_u16() == 400
            && body_text.contains("\"invalid_grant\"")
            && body_text.contains("Invalid refresh token provided")
        {
            return Err(RefreshTokenInvalidError {
                message: format!("Social refreshToken 已失效 (invalid_grant): {}", body_text),
            }
            .into());
        }

        let error_msg = match status.as_u16() {
            401 => "OAuth 凭证已过期或无效，需要重新认证",
            403 => "权限不足，无法刷新 Token",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS OAuth 服务暂时不可用",
            _ => "Token 刷新失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: RefreshResponse = response.json().await?;

    let mut new_credentials = credentials.clone();
    new_credentials.access_token = Some(data.access_token);

    if let Some(new_refresh_token) = data.refresh_token {
        new_credentials.refresh_token = Some(new_refresh_token);
    }

    if let Some(profile_arn) = data.profile_arn {
        new_credentials.profile_arn = Some(profile_arn);
    }

    if let Some(expires_in) = data.expires_in {
        let expires_at = Utc::now() + Duration::seconds(expires_in);
        new_credentials.expires_at = Some(expires_at.to_rfc3339());
    }

    Ok(new_credentials)
}

/// 刷新 IdC Token (AWS SSO OIDC)
async fn refresh_idc_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 IdC Token...");

    let refresh_token = credentials.refresh_token.as_ref().unwrap();
    let client_id = credentials
        .client_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("IdC 刷新需要 clientId"))?;
    let client_secret = credentials
        .client_secret
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("IdC 刷新需要 clientSecret"))?;

    // 优先级：凭据.auth_region > 凭据.region > config.auth_region > config.region
    let region = credentials.effective_auth_region(config);
    let refresh_url = format!("https://oidc.{}.amazonaws.com/token", region);
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    let x_amz_user_agent = "aws-sdk-js/3.980.0 KiroIDE";
    let user_agent = format!(
        "aws-sdk-js/3.980.0 ua/2.1 os/{} lang/js md/nodejs#{} api/sso-oidc#3.980.0 m/E KiroIDE",
        os_name, node_version
    );

    let client = build_client(proxy, 60, config.tls_backend)?;
    let body = IdcRefreshRequest {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
        refresh_token: refresh_token.to_string(),
        grant_type: "refresh_token".to_string(),
    };

    let response = client
        .post(&refresh_url)
        .header("content-type", "application/json")
        .header("x-amz-user-agent", x_amz_user_agent)
        .header("user-agent", &user_agent)
        .header("host", format!("oidc.{}.amazonaws.com", region))
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=4")
        .header("Connection", "close")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();

        // 400 + invalid_grant + Invalid refresh token provided → refreshToken 永久失效
        if status.as_u16() == 400
            && body_text.contains("\"invalid_grant\"")
            && body_text.contains("Invalid refresh token provided")
        {
            return Err(RefreshTokenInvalidError {
                message: format!("IdC refreshToken 已失效 (invalid_grant): {}", body_text),
            }
            .into());
        }

        let error_msg = match status.as_u16() {
            401 => "IdC 凭证已过期或无效，需要重新认证",
            403 => "权限不足，无法刷新 Token",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS OIDC 服务暂时不可用",
            _ => "IdC Token 刷新失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: IdcRefreshResponse = response.json().await?;

    let mut new_credentials = credentials.clone();
    new_credentials.access_token = Some(data.access_token);

    if let Some(new_refresh_token) = data.refresh_token {
        new_credentials.refresh_token = Some(new_refresh_token);
    }

    if let Some(expires_in) = data.expires_in {
        let expires_at = Utc::now() + Duration::seconds(expires_in);
        new_credentials.expires_at = Some(expires_at.to_rfc3339());
    }

    // 同步更新 profile_arn（如果 IdC 响应中包含）
    if let Some(profile_arn) = data.profile_arn {
        new_credentials.profile_arn = Some(profile_arn);
    }

    Ok(new_credentials)
}

/// 获取使用额度信息
pub(crate) async fn get_usage_limits(
    credentials: &KiroCredentials,
    config: &Config,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<UsageLimitsResponse> {
    tracing::debug!("正在获取使用额度信息...");

    // 优先级：凭据.api_region > config.api_region > config.region
    let region = credentials.effective_api_region(config);
    let host = format!("q.{}.amazonaws.com", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config);
    let kiro_version = &config.kiro_version;
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    // 构建 URL
    let mut url = format!(
        "https://{}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST",
        host
    );

    // profileArn 是可选的
    if let Some(profile_arn) = &credentials.profile_arn {
        url.push_str(&format!("&profileArn={}", urlencoding::encode(profile_arn)));
    }

    // 构建 User-Agent headers
    let user_agent = format!(
        "aws-sdk-js/1.0.0 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{}-{}",
        os_name, node_version, kiro_version, machine_id
    );
    let amz_user_agent = format!("aws-sdk-js/1.0.0 KiroIDE-{}-{}", kiro_version, machine_id);

    let client = build_client(proxy, 60, config.tls_backend)?;

    let mut request = client
        .get(&url)
        .header("x-amz-user-agent", &amz_user_agent)
        .header("user-agent", &user_agent)
        .header("host", &host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Connection", "close");

    if credentials.is_api_key_credential() {
        request = request.header("tokentype", "API_KEY");
    }

    let response = request.send().await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let error_msg = match status.as_u16() {
            401 => "认证失败，Token 无效或已过期",
            403 => "权限不足，无法获取使用额度",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS 服务暂时不可用",
            _ => "获取使用额度失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: UsageLimitsResponse = response.json().await?;
    Ok(data)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserInfoResponse {
    #[serde(default)]
    pub email: Option<String>,
}

/// 获取当前账号用户信息
pub(crate) async fn get_user_info(
    credentials: &KiroCredentials,
    config: &Config,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<UserInfoResponse> {
    tracing::debug!("正在获取账号用户信息...");

    let machine_id = machine_id::generate_from_credentials(credentials, config);
    let kiro_version = &config.kiro_version;
    let url = format!("{}/GetUserInfo", KIRO_WEB_PORTAL_API_BASE);
    let client = build_client(proxy, 60, config.tls_backend)?;
    let body = json!({ "origin": "KIRO_IDE" });
    let mut body_bytes = Vec::new();
    ciborium::into_writer(&body, &mut body_bytes)?;

    let idp = "BuilderId";

    let mut request = client
        .post(&url)
        .header("accept", "application/cbor")
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=1")
        .header(
            "x-amz-user-agent",
            format!("aws-sdk-js/1.0.18 KiroIDE {} {}", kiro_version, machine_id),
        )
        .header("Authorization", format!("Bearer {}", access_token))
        .header(
            "Cookie",
            format!("Idp={}; AccessToken={}", idp, access_token),
        )
        .body(body_bytes);

    if credentials.is_api_key_credential() {
        request = request.header("tokentype", "API_KEY");
    }

    let response = request.send().await?;
    let status = response.status();
    let response_bytes = response.bytes().await?;

    if !status.is_success() {
        let mut cursor = std::io::Cursor::new(response_bytes.as_ref());
        let error_detail = ciborium::from_reader::<serde_json::Value, _>(&mut cursor)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| Some(value.to_string()))
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&response_bytes).to_string());
        bail!("获取账号用户信息失败: {} {}", status, error_detail);
    }

    let mut cursor = std::io::Cursor::new(response_bytes.as_ref());
    let data: UserInfoResponse = ciborium::from_reader(&mut cursor)?;
    Ok(data)
}

/// 获取当前账号可用模型列表
pub(crate) async fn get_available_models(
    credentials: &KiroCredentials,
    config: &Config,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<Vec<String>> {
    tracing::debug!("正在获取可用模型列表...");

    let region = credentials.effective_api_region(config);
    let host = format!("q.{}.amazonaws.com", region);
    let machine_id = machine_id::generate_from_credentials(credentials, config);
    let kiro_version = &config.kiro_version;
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    let user_agent = format!(
        "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.34 m/E KiroIDE-{}-{}",
        os_name, node_version, kiro_version, machine_id
    );
    let amz_user_agent = format!("aws-sdk-js/1.0.34 KiroIDE-{}-{}", kiro_version, machine_id);

    let client = build_client(proxy, 60, config.tls_backend)?;
    let mut next_token: Option<String> = None;
    let mut models = Vec::new();

    loop {
        let mut url = format!("https://{}/ListAvailableModels?origin=AI_EDITOR", host);
        if let Some(profile_arn) = &credentials.profile_arn {
            url.push_str(&format!("&profileArn={}", urlencoding::encode(profile_arn)));
        }
        if let Some(page_token) = &next_token {
            url.push_str(&format!("&nextToken={}", urlencoding::encode(page_token)));
        }

        let mut request = client
            .get(&url)
            .header("x-amz-user-agent", &amz_user_agent)
            .header("user-agent", &user_agent)
            .header("host", &host)
            .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=1")
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Connection", "close");

        if credentials.is_api_key_credential() {
            request = request.header("tokentype", "API_KEY");
        }

        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            let error_msg = match status.as_u16() {
                401 => "认证失败，Token 无效或已过期",
                403 => "权限不足，无法获取可用模型列表",
                429 => "请求过于频繁，已被限流",
                500..=599 => "服务器错误，AWS 服务暂时不可用",
                _ => "获取可用模型列表失败",
            };
            bail!("{}: {} {}", error_msg, status, body_text);
        }

        let data: AvailableModelsResponse = response.json().await?;
        for model in data.models {
            if let Some(id) = model.model_identifier() {
                models.push(id.to_string());
            }
        }
        if let Some(model) = data.default_model {
            if let Some(id) = model.model_identifier() {
                models.push(id.to_string());
            }
        }

        next_token = data.next_token;
        if next_token.is_none() {
            break;
        }
    }

    models.sort();
    models.dedup();
    Ok(models)
}

// ============================================================================
// 多凭据 Token 管理器
// ============================================================================

/// 单个凭据条目的状态
struct CredentialEntry {
    /// 凭据唯一 ID
    id: u64,
    /// 凭据信息
    credentials: KiroCredentials,
    /// API 调用连续失败次数
    failure_count: u32,
    /// Token 刷新连续失败次数
    refresh_failure_count: u32,
    /// 是否已禁用
    disabled: bool,
    /// 禁用原因（用于区分手动禁用 vs 自动禁用，便于自愈）
    disabled_reason: Option<DisabledReason>,
    /// API 调用成功次数
    success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    last_used_at: Option<String>,
    /// 当前占用中的并发槽位
    inflight: u32,
    /// 当前凭据的并发上限
    max_concurrent: u32,
    /// 冷却截止时间
    cooldown_until: Option<DateTime<Utc>>,
    /// 最近一次限频类型
    last_rate_limit_kind: Option<RateLimitKind>,
    /// 最近普通 429 次数
    recent_429_count: u32,
    /// 最近 suspicious 次数
    recent_suspicious_count: u32,
    /// 最近一次 suspicious 触发时间
    last_suspicious_at: Option<DateTime<Utc>>,
    /// suspicious 隔离截止时间
    suspicious_isolation_until: Option<DateTime<Utc>>,
    /// 最近一次被绑定的活跃时间
    sticky_detached: bool,
    /// 最近一次选号路径
    last_dispatch_path: Option<DispatchPath>,
    /// 最近一次软回退时间
    last_soft_fallback_at: Option<String>,
}

/// 禁用原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisabledReason {
    /// Admin API 手动禁用
    Manual,
    /// 上游明确返回账号封禁/暂停/停用
    Banned,
    /// 连续失败达到阈值后自动禁用
    TooManyFailures,
    /// Token 刷新连续失败达到阈值后自动禁用
    TooManyRefreshFailures,
    /// 额度已用尽（如 MONTHLY_REQUEST_COUNT）
    QuotaExceeded,
    /// Refresh Token 永久失效（服务端返回 invalid_grant）
    InvalidRefreshToken,
    /// 凭据配置无效（如 authMethod=api_key 但缺少 kiroApiKey）
    InvalidConfig,
}

fn disabled_reason_label(reason: DisabledReason) -> String {
    match reason {
        DisabledReason::Manual => "Manual",
        DisabledReason::Banned => "Banned",
        DisabledReason::TooManyFailures => "TooManyFailures",
        DisabledReason::TooManyRefreshFailures => "TooManyRefreshFailures",
        DisabledReason::QuotaExceeded => "QuotaExceeded",
        DisabledReason::InvalidRefreshToken => "InvalidRefreshToken",
        DisabledReason::InvalidConfig => "InvalidConfig",
    }
    .to_string()
}

fn disabled_reason_from_config(value: Option<&str>) -> Option<DisabledReason> {
    match value {
        Some("Manual") | Some("manual") => Some(DisabledReason::Manual),
        Some("Banned") | Some("banned") => Some(DisabledReason::Banned),
        Some("TooManyFailures") | Some("too_many_failures") => {
            Some(DisabledReason::TooManyFailures)
        }
        Some("TooManyRefreshFailures") | Some("too_many_refresh_failures") => {
            Some(DisabledReason::TooManyRefreshFailures)
        }
        Some("QuotaExceeded") | Some("quota_exceeded") => Some(DisabledReason::QuotaExceeded),
        Some("InvalidRefreshToken") | Some("invalid_refresh_token") => {
            Some(DisabledReason::InvalidRefreshToken)
        }
        Some("InvalidConfig") | Some("invalid_config") => Some(DisabledReason::InvalidConfig),
        _ => None,
    }
}

pub(crate) fn is_account_banned_response(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("account banned")
        || lower.contains("account has been banned")
        || lower.contains("account suspended")
        || lower.contains("account has been suspended")
        || lower.contains("account disabled")
        || lower.contains("account has been disabled")
        || lower.contains("account deactivated")
        || lower.contains("account has been deactivated")
        || lower.contains("account terminated")
        || lower.contains("account has been terminated")
        || lower.contains("user banned")
        || lower.contains("user suspended")
        || (lower.contains("user id") && lower.contains("temporarily is suspended"))
        || lower.contains("locked your account")
        || lower.contains("封号")
        || lower.contains("封禁")
        || lower.contains("账号已被禁用")
        || lower.contains("账户已被禁用")
}

/// 统计数据持久化条目
#[derive(Serialize, Deserialize)]
struct StatsEntry {
    success_count: u64,
    last_used_at: Option<String>,
}

#[derive(Debug, Clone)]
struct StickyBinding {
    account_id: u64,
    expires_at: DateTime<Utc>,
}

// ============================================================================
// Admin API 公开结构
// ============================================================================

/// 凭据条目快照（用于 Admin API 读取）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialEntrySnapshot {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级
    pub priority: u32,
    /// 请求策略。
    pub scheduler_policy: SchedulerPolicy,
    /// 是否被禁用
    pub disabled: bool,
    /// 连续失败次数
    pub failure_count: u32,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// Token 过期时间
    pub expires_at: Option<String>,
    /// refreshToken 的 SHA-256 哈希（仅 OAuth 凭据，用于前端去重）
    pub refresh_token_hash: Option<String>,
    /// kiroApiKey 的 SHA-256 哈希（仅 API Key 凭据，用于前端去重）
    pub api_key_hash: Option<String>,
    /// kiroApiKey 的脱敏展示（仅 API Key 凭据，用于前端显示）
    pub masked_api_key: Option<String>,
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// API 调用成功次数
    pub success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    pub last_used_at: Option<String>,
    /// 是否配置了凭据级代理
    pub has_proxy: bool,
    /// 代理 URL（用于前端展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// 代理使用方式
    pub proxy_mode: Option<String>,
    /// 绑定的代理池 ID
    pub proxy_id: Option<u64>,
    /// Token 刷新连续失败次数
    pub refresh_failure_count: u32,
    /// 禁用原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// 对外展示的账号状态：normal / banned / rate_limited / disabled
    pub account_status: String,
    /// 端点名称（未显式配置时返回 None，由 Admin 层回退到默认值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// 调度状态
    pub dispatch_state: String,
    /// 当前并发
    pub current_concurrent: u32,
    /// 并发上限
    pub max_concurrent: u32,
    /// 冷却剩余时间（毫秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_remaining_ms: Option<u64>,
    /// 最近一次限频类型
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_rate_limit_kind: Option<String>,
    /// 最近普通 429 次数
    pub recent_429_count: u32,
    /// 最近 suspicious 次数
    pub recent_suspicious_count: u32,
    /// 粘性会话数
    pub sticky_session_count: u32,
    /// 是否已解除粘性
    pub sticky_detached: bool,
    /// 最近一次选号路径
    pub dispatch_path: Option<String>,
    /// 当前是否允许软回退
    pub soft_fallback_eligible: bool,
    /// 最近一次软回退时间
    pub last_soft_fallback_at: Option<String>,
    /// 是否处于 suspicious 隔离
    pub suspicious_isolated: bool,
    /// suspicious 隔离剩余时间（毫秒）
    pub isolation_remaining_ms: Option<u64>,
    /// 账号健康分（0-100）
    pub health_score: u32,
    /// 当前调度权重（0.0-1.0）
    pub dispatch_weight: f64,
    /// 权重/健康分说明
    pub weight_reason: String,
    /// 订阅等级
    pub subscription_title: Option<String>,
    /// 当前账号可用模型列表
    pub available_models: Option<Vec<String>>,
}

/// 凭据管理器状态快照
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerSnapshot {
    /// 凭据条目列表
    pub entries: Vec<CredentialEntrySnapshot>,
    /// 当前活跃凭据 ID
    pub current_id: u64,
    /// 总凭据数量
    pub total: usize,
    /// 当前可直接调度的凭据数量
    pub available: usize,
    /// 未禁用的凭据数量
    pub enabled_count: usize,
    /// 当前可直接调度的凭据数量
    pub schedulable_count: usize,
}

#[derive(Debug, Clone)]
pub struct AcquireOptions {
    pub model: Option<String>,
    pub scheduler_policy: Option<SchedulerPolicy>,
    pub session_key: Option<String>,
    pub tried_account_ids: HashSet<u64>,
    pub preferred_account_id: Option<u64>,
    pub strict_preferred_account: bool,
    pub runtime_probe: bool,
}

impl AcquireOptions {
    pub fn new(model: Option<String>) -> Self {
        Self {
            model,
            scheduler_policy: None,
            session_key: None,
            tried_account_ids: HashSet::new(),
            preferred_account_id: None,
            strict_preferred_account: false,
            runtime_probe: false,
        }
    }
}

#[derive(Clone)]
pub struct DispatchLease {
    pub id: u64,
    released: bool,
    entries: Arc<Mutex<Vec<CredentialEntry>>>,
}

impl DispatchLease {
    fn new(id: u64, entries: Arc<Mutex<Vec<CredentialEntry>>>) -> Self {
        Self {
            id,
            released: false,
            entries,
        }
    }
}

impl Drop for DispatchLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.iter_mut().find(|e| e.id == self.id) {
            entry.inflight = entry.inflight.saturating_sub(1);
        }
        self.released = true;
    }
}

/// 多凭据 Token 管理器
///
/// 支持多个凭据的管理，实现固定优先级 + 故障转移策略
/// 故障统计基于 API 调用结果，而非 Token 刷新结果
pub struct MultiTokenManager {
    config: Config,
    proxy_pool: ProxyPool,
    /// 凭据条目列表
    entries: Arc<Mutex<Vec<CredentialEntry>>>,
    /// 当前活动凭据 ID
    current_id: Mutex<u64>,
    /// Token 刷新锁，确保同一时间只有一个刷新操作
    refresh_lock: TokioMutex<()>,
    /// 凭据文件路径（用于回写）
    credentials_path: Option<PathBuf>,
    /// 是否为多凭据格式（数组格式才回写）
    is_multiple_format: bool,
    /// 负载均衡模式（运行时可修改）
    load_balancing_mode: Mutex<String>,
    /// 最近一次统计持久化时间（用于 debounce）
    last_stats_save_at: Mutex<Option<Instant>>,
    /// 统计数据是否有未落盘更新
    stats_dirty: AtomicBool,
    /// 会话粘性绑定
    sticky_bindings: Mutex<HashMap<String, StickyBinding>>,
    /// 同优先级轮询游标
    round_robin_cursor: Mutex<HashMap<u32, usize>>,
    /// 请求诊断存储
    diagnostics: DiagnosticsStore,
}

/// 每个凭据最大 API 调用失败次数
const MAX_FAILURES_PER_CREDENTIAL: u32 = 3;
/// 统计数据持久化防抖间隔
const STATS_SAVE_DEBOUNCE: StdDuration = StdDuration::from_secs(30);

/// API 调用上下文
///
/// 绑定特定凭据的调用上下文，确保 token、credentials 和 id 的一致性
/// 用于解决并发调用时 current_id 竞态问题
#[derive(Clone)]
pub struct CallContext {
    /// 凭据 ID（用于 report_success/report_failure）
    pub id: u64,
    /// 凭据信息（用于构建请求头）
    pub credentials: KiroCredentials,
    /// 访问 Token
    pub token: String,
    /// 当前请求占用的调度槽位
    pub lease: Option<DispatchLease>,
    /// 选号路径
    pub dispatch_path: DispatchPath,
    /// 是否走软回退
    pub used_soft_fallback: bool,
    /// 账号开始请求时的状态
    pub account_state_at_start: DispatchState,
}

#[derive(Clone)]
struct SelectionResult {
    id: u64,
    credentials: KiroCredentials,
    dispatch_path: DispatchPath,
    used_soft_fallback: bool,
    account_state_at_start: DispatchState,
}

impl MultiTokenManager {
    /// 创建多凭据 Token 管理器
    ///
    /// # Arguments
    /// * `config` - 应用配置
    /// * `credentials` - 凭据列表
    /// * `credentials_path` - 凭据文件路径（用于回写）
    /// * `is_multiple_format` - 是否为多凭据格式（数组格式才回写）
    pub fn new(
        config: Config,
        credentials: Vec<KiroCredentials>,
        credentials_path: Option<PathBuf>,
        is_multiple_format: bool,
    ) -> anyhow::Result<Self> {
        // 计算当前最大 ID，为没有 ID 的凭据分配新 ID
        let max_existing_id = credentials.iter().filter_map(|c| c.id).max().unwrap_or(0);
        let mut next_id = max_existing_id + 1;
        let mut has_new_ids = false;
        let mut has_new_machine_ids = false;
        let config_ref = &config;

        let entries: Vec<CredentialEntry> = credentials
            .into_iter()
            .map(|mut cred| {
                cred.canonicalize_auth_method();
                let id = cred.id.unwrap_or_else(|| {
                    let id = next_id;
                    next_id += 1;
                    cred.id = Some(id);
                    has_new_ids = true;
                    id
                });
                if cred.machine_id.is_none() {
                    cred.machine_id =
                        Some(machine_id::generate_from_credentials(&cred, config_ref));
                    has_new_machine_ids = true;
                }
                CredentialEntry {
                    id,
                    credentials: cred.clone(),
                    failure_count: 0,
                    refresh_failure_count: 0,
                    disabled: cred.disabled, // 从配置文件读取 disabled 状态
                    disabled_reason: if cred.disabled {
                        disabled_reason_from_config(cred.disabled_reason.as_deref())
                            .or(Some(DisabledReason::Manual))
                    } else {
                        None
                    },
                    success_count: 0,
                    last_used_at: None,
                    inflight: 0,
                    max_concurrent: cred.max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT).max(1),
                    cooldown_until: None,
                    last_rate_limit_kind: None,
                    recent_429_count: 0,
                    recent_suspicious_count: 0,
                    last_suspicious_at: None,
                    suspicious_isolation_until: None,
                    sticky_detached: false,
                    last_dispatch_path: None,
                    last_soft_fallback_at: None,
                }
            })
            .collect();

        // 校验 API Key 凭据配置完整性：authMethod=api_key 时必须提供 kiroApiKey
        let mut entries = entries;
        for entry in &mut entries {
            if entry.credentials.kiro_api_key.is_none()
                && entry
                    .credentials
                    .auth_method
                    .as_deref()
                    .map(|m| m.eq_ignore_ascii_case("api_key") || m.eq_ignore_ascii_case("apikey"))
                    .unwrap_or(false)
            {
                tracing::warn!(
                    "凭据 #{} 配置了 authMethod=api_key 但缺少 kiroApiKey 字段，已自动禁用",
                    entry.id
                );
                entry.disabled = true;
                entry.disabled_reason = Some(DisabledReason::InvalidConfig);
            }
        }

        // 检测重复 ID
        let mut seen_ids = std::collections::HashSet::new();
        let mut duplicate_ids = Vec::new();
        for entry in &entries {
            if !seen_ids.insert(entry.id) {
                duplicate_ids.push(entry.id);
            }
        }
        if !duplicate_ids.is_empty() {
            anyhow::bail!("检测到重复的凭据 ID: {:?}", duplicate_ids);
        }

        // 选择初始凭据：优先级最高（priority 最小）的可用凭据，无可用凭据时为 0
        let initial_id = entries
            .iter()
            .filter(|e| !e.disabled)
            .min_by_key(|e| e.credentials.priority)
            .map(|e| e.id)
            .unwrap_or(0);

        let load_balancing_mode = config.load_balancing_mode.clone();
        let diagnostics_config = config.diagnostics.clone();
        let diagnostics_path = credentials_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.join("kiro_request_diagnostics.jsonl")));
        let proxy_pool = ProxyPool::new(ProxyPool::path_for_cache_dir(
            credentials_path.as_ref().and_then(|p| p.parent()),
        ));
        let manager = Self {
            config,
            proxy_pool,
            entries: Arc::new(Mutex::new(entries)),
            current_id: Mutex::new(initial_id),
            refresh_lock: TokioMutex::new(()),
            credentials_path,
            is_multiple_format,
            load_balancing_mode: Mutex::new(load_balancing_mode),
            last_stats_save_at: Mutex::new(None),
            stats_dirty: AtomicBool::new(false),
            sticky_bindings: Mutex::new(HashMap::new()),
            round_robin_cursor: Mutex::new(HashMap::new()),
            diagnostics: DiagnosticsStore::new(diagnostics_config, diagnostics_path),
        };

        // 如果有新分配的 ID 或新生成的 machineId，立即持久化到配置文件
        if has_new_ids || has_new_machine_ids {
            if let Err(e) = manager.persist_credentials() {
                tracing::warn!("补全凭据 ID/machineId 后持久化失败: {}", e);
            } else {
                tracing::info!("已补全凭据 ID/machineId 并写回配置文件");
            }
        }

        // 加载持久化的统计数据（success_count, last_used_at）
        manager.load_stats();

        Ok(manager)
    }

    /// 获取配置的引用
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn start_email_backfill_task(self: &Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            manager.backfill_missing_emails().await;
        });
    }

    async fn backfill_missing_emails(&self) {
        let mut pending_ids = {
            let entries = self.entries.lock();
            entries
                .iter()
                .filter(|entry| !entry.disabled)
                .filter(|entry| credential_email_key(&entry.credentials).is_none())
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
        };

        if pending_ids.is_empty() {
            self.deduplicate_existing_credentials_by_email();
            return;
        }

        tracing::info!("启动时发现 {} 个账号缺少邮箱，开始补全", pending_ids.len());
        let failed_ids = self
            .backfill_email_batch(pending_ids.drain(..).collect())
            .await;

        if !failed_ids.is_empty() {
            tracing::info!("{} 个账号邮箱补全失败，开始重试一次", failed_ids.len());
            let retry_failed_ids = self.backfill_email_batch(failed_ids).await;
            for id in retry_failed_ids {
                tracing::warn!("凭据 #{} 重试后仍未获取到邮箱", id);
            }
        }

        self.deduplicate_existing_credentials_by_email();
    }

    async fn backfill_email_batch(&self, ids: Vec<u64>) -> Vec<u64> {
        let mut failed_ids = Vec::new();
        for id in ids {
            match self.refresh_email_for(id).await {
                Ok(Some(email)) => tracing::info!("凭据 #{} 邮箱已补全: {}", id, email),
                Ok(None) => {
                    tracing::warn!("凭据 #{} 未能从账号信息 API 获取邮箱", id);
                    failed_ids.push(id);
                }
                Err(err) => {
                    tracing::warn!("凭据 #{} 邮箱补全失败: {}", id, err);
                    failed_ids.push(id);
                }
            }
            tokio::time::sleep(StdDuration::from_millis(500)).await;
        }
        failed_ids
    }

    fn deduplicate_existing_credentials_by_email(&self) {
        let current_id = *self.current_id.lock();
        let duplicate_ids = {
            let entries = self.entries.lock();
            let mut ordered = entries
                .iter()
                .filter_map(|entry| {
                    credential_email_key(&entry.credentials).map(|email| {
                        (
                            email,
                            entry.id,
                            entry.credentials.priority,
                            entry.id == current_id,
                        )
                    })
                })
                .collect::<Vec<_>>();

            ordered.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| b.3.cmp(&a.3))
                    .then_with(|| a.2.cmp(&b.2))
                    .then_with(|| a.1.cmp(&b.1))
            });

            let mut seen_emails = HashSet::new();
            ordered
                .into_iter()
                .filter_map(|(email, id, _, _)| {
                    if seen_emails.insert(email) {
                        None
                    } else {
                        Some(id)
                    }
                })
                .collect::<HashSet<_>>()
        };

        if duplicate_ids.is_empty() {
            tracing::info!("邮箱补全完成，未发现重复账号");
            return;
        }

        let removed = {
            let mut entries = self.entries.lock();
            let before = entries.len();
            entries.retain(|entry| !duplicate_ids.contains(&entry.id));
            before - entries.len()
        };

        let current_still_available = {
            let entries = self.entries.lock();
            entries
                .iter()
                .any(|entry| entry.id == current_id && !entry.disabled)
        };
        if !current_still_available {
            self.select_highest_priority();
        }

        if self.entries.lock().is_empty() {
            let mut current_id = self.current_id.lock();
            *current_id = 0;
        }

        if let Err(err) = self.persist_credentials() {
            tracing::warn!("邮箱去重后持久化失败: {}", err);
        }
        self.save_stats();

        let mut ids = duplicate_ids.into_iter().collect::<Vec<_>>();
        ids.sort_unstable();
        tracing::info!("邮箱补全完成，已删除 {} 个重复账号: {:?}", removed, ids);
    }

    fn random_proxy_from_pool(&self) -> Option<ProxyConfig> {
        self.proxy_pool.random_enabled_proxy()
    }

    /// 获取凭据总数
    pub fn total_count(&self) -> usize {
        self.entries.lock().len()
    }

    /// 获取可用凭据数量
    pub fn available_count(&self) -> usize {
        self.entries.lock().iter().filter(|e| !e.disabled).count()
    }

    /// 获取当前可直接调度的凭据数量
    pub fn schedulable_count(&self) -> usize {
        let entries = self.entries.lock();
        let empty_tried = HashSet::new();
        entries
            .iter()
            .filter(|e| self.entry_schedulable(e, None, &empty_tried))
            .count()
    }

    pub fn schedulable_capacity_for_model(
        &self,
        model: Option<&str>,
        scheduler_policy: Option<SchedulerPolicy>,
    ) -> u32 {
        let empty_tried = HashSet::new();
        self.schedulable_capacity_for_filter(model, scheduler_policy, &empty_tried)
    }

    pub fn schedulable_capacity_for_options(&self, options: &AcquireOptions) -> u32 {
        self.schedulable_capacity_for_filter(
            options.model.as_deref(),
            options.scheduler_policy,
            &options.tried_account_ids,
        )
    }

    fn schedulable_capacity_for_filter(
        &self,
        model: Option<&str>,
        scheduler_policy: Option<SchedulerPolicy>,
        tried_account_ids: &HashSet<u64>,
    ) -> u32 {
        let entries = self.entries.lock();
        entries
            .iter()
            .filter(|e| self.entry_policy_matches(e, scheduler_policy))
            .filter(|e| self.entry_schedulable(e, model, tried_account_ids))
            .map(|e| e.max_concurrent.saturating_sub(e.inflight))
            .sum()
    }

    pub fn scheduler_policy_for_account(&self, id: u64) -> Option<SchedulerPolicy> {
        self.entries
            .lock()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.credentials.scheduler_policy)
    }

    /// 根据负载均衡模式选择下一个凭据
    ///
    /// - priority 模式：选择优先级最高（priority 最小）的可用凭据
    /// - balanced 模式：均衡选择可用凭据
    ///
    /// # 参数
    /// - `model`: 可选的模型名称，用于过滤支持该模型的凭据（如 opus 模型需要付费订阅）
    fn select_next_credential(&self, options: &AcquireOptions) -> Option<SelectionResult> {
        self.gc_sticky_bindings();
        let entries = self.entries.lock();

        let model = options.model.as_deref();

        if let Some(preferred_account_id) = options.preferred_account_id {
            if options.runtime_probe {
                if let Some(entry) = entries.iter().find(|e| {
                    e.id == preferred_account_id
                        && self.entry_policy_matches(e, options.scheduler_policy)
                        && !options.tried_account_ids.contains(&e.id)
                        && self.entry_supports_model(e, model)
                }) {
                    return Some(SelectionResult {
                        id: entry.id,
                        credentials: entry.credentials.clone(),
                        dispatch_path: DispatchPath::Preferred,
                        used_soft_fallback: false,
                        account_state_at_start: self.dispatch_state_of(entry, Utc::now()),
                    });
                }
                if options.strict_preferred_account {
                    return None;
                }
            }
        }

        let available: Vec<_> = entries
            .iter()
            .filter(|e| {
                if !self.entry_schedulable(e, model, &options.tried_account_ids) {
                    return false;
                }
                self.entry_policy_matches(e, options.scheduler_policy)
            })
            .collect();

        if let Some(preferred_account_id) = options.preferred_account_id {
            if let Some(entry) = available.iter().find(|e| e.id == preferred_account_id) {
                return Some(SelectionResult {
                    id: entry.id,
                    credentials: entry.credentials.clone(),
                    dispatch_path: DispatchPath::Preferred,
                    used_soft_fallback: false,
                    account_state_at_start: self.dispatch_state_of(entry, Utc::now()),
                });
            }
            if options.strict_preferred_account {
                return None;
            }
        }

        if let Some(session_key) = options.session_key.as_deref() {
            if let Some(bound_id) = self
                .sticky_bindings
                .lock()
                .get(session_key)
                .map(|b| b.account_id)
            {
                if let Some(entry) = available.iter().find(|e| e.id == bound_id) {
                    return Some(SelectionResult {
                        id: entry.id,
                        credentials: entry.credentials.clone(),
                        dispatch_path: DispatchPath::Sticky,
                        used_soft_fallback: false,
                        account_state_at_start: self.dispatch_state_of(entry, Utc::now()),
                    });
                }
            }
        }

        let mode = self.load_balancing_mode.lock().clone();
        let mode = mode.as_str();

        if !available.is_empty() {
            let selected = match mode {
                "balanced" if self.config.scheduler.health_weighted_scheduling_enabled => {
                    self.pick_health_weighted_entry(&available)
                }
                "balanced" => self.pick_round_robin_entry(&available),
                _ => self.pick_priority_entry(&available),
            };

            if let Some((id, credentials, _, _)) = selected {
                let entry = available.iter().find(|entry| entry.id == id)?;
                return Some(SelectionResult {
                    id,
                    credentials,
                    dispatch_path: DispatchPath::Balanced,
                    used_soft_fallback: false,
                    account_state_at_start: self.dispatch_state_of(entry, Utc::now()),
                });
            }
        }

        self.pick_soft_fallback_entry(
            &entries,
            model,
            &options.tried_account_ids,
            options.scheduler_policy,
        )
    }

    fn entry_policy_matches(
        &self,
        entry: &CredentialEntry,
        scheduler_policy: Option<SchedulerPolicy>,
    ) -> bool {
        scheduler_policy
            .map(|policy| entry.credentials.scheduler_policy == policy)
            .unwrap_or(true)
    }

    fn entry_schedulable(
        &self,
        entry: &CredentialEntry,
        model: Option<&str>,
        tried_account_ids: &HashSet<u64>,
    ) -> bool {
        if entry.disabled || tried_account_ids.contains(&entry.id) {
            return false;
        }
        if self.entry_suspicious_isolated(entry, Utc::now()) {
            return false;
        }
        if entry.refresh_failure_count >= MAX_FAILURES_PER_CREDENTIAL {
            return false;
        }
        if !self.entry_supports_model(entry, model) {
            return false;
        }
        if entry
            .cooldown_until
            .is_some_and(|deadline| deadline > Utc::now())
        {
            return false;
        }
        if entry.inflight >= entry.max_concurrent {
            return false;
        }
        true
    }

    fn pick_priority_entry(
        &self,
        available: &[&CredentialEntry],
    ) -> Option<(u64, KiroCredentials, u32, String)> {
        let min_priority = available.iter().map(|e| e.credentials.priority).min()?;
        let group: Vec<_> = available
            .iter()
            .copied()
            .filter(|e| e.credentials.priority == min_priority)
            .collect();
        self.pick_round_robin_from_group(min_priority, &group)
    }

    fn pick_round_robin_entry(
        &self,
        available: &[&CredentialEntry],
    ) -> Option<(u64, KiroCredentials, u32, String)> {
        self.pick_round_robin_from_group(u32::MAX, available)
    }

    fn pick_round_robin_from_group(
        &self,
        priority: u32,
        group: &[&CredentialEntry],
    ) -> Option<(u64, KiroCredentials, u32, String)> {
        if group.is_empty() {
            return None;
        }
        let mut sorted = group.to_vec();
        sorted.sort_by_key(|e| e.id);
        let mut cursor_map = self.round_robin_cursor.lock();
        let cursor = cursor_map.entry(priority).or_insert(0);
        let index = *cursor % sorted.len();
        *cursor = (*cursor + 1) % sorted.len();
        sorted.get(index).map(|entry| {
            let (score, reason) = self.health_score(entry, Utc::now());
            (entry.id, entry.credentials.clone(), score, reason)
        })
    }

    fn pick_health_weighted_entry(
        &self,
        available: &[&CredentialEntry],
    ) -> Option<(u64, KiroCredentials, u32, String)> {
        let now = Utc::now();
        available
            .iter()
            .copied()
            .map(|entry| {
                let (score, reason) = self.health_score(entry, now);
                (entry, score, reason)
            })
            .max_by(|(left, left_score, _), (right, right_score, _)| {
                left_score
                    .cmp(right_score)
                    .then_with(|| right.credentials.priority.cmp(&left.credentials.priority))
                    .then_with(|| right.id.cmp(&left.id))
            })
            .map(|(entry, score, reason)| (entry.id, entry.credentials.clone(), score, reason))
    }

    fn pick_soft_fallback_entry(
        &self,
        entries: &[CredentialEntry],
        model: Option<&str>,
        tried_account_ids: &HashSet<u64>,
        scheduler_policy: Option<SchedulerPolicy>,
    ) -> Option<SelectionResult> {
        if !self.config.scheduler.soft_fallback_enabled {
            return None;
        }

        let now = Utc::now();
        let normal_group: Vec<_> = entries
            .iter()
            .filter(|entry| self.entry_policy_matches(entry, scheduler_policy))
            .filter(|entry| self.soft_fallback_eligible(entry, model, tried_account_ids, now))
            .collect();

        if let Some((id, credentials, _, _)) = self.pick_round_robin_entry(&normal_group) {
            let entry = normal_group.iter().find(|entry| entry.id == id)?;
            return Some(SelectionResult {
                id,
                credentials,
                dispatch_path: DispatchPath::SoftFallback,
                used_soft_fallback: true,
                account_state_at_start: self.dispatch_state_of(entry, now),
            });
        }

        None
    }

    fn soft_fallback_eligible(
        &self,
        entry: &CredentialEntry,
        model: Option<&str>,
        tried_account_ids: &HashSet<u64>,
        now: DateTime<Utc>,
    ) -> bool {
        if !self.config.scheduler.soft_fallback_enabled {
            return false;
        }
        if entry.disabled || tried_account_ids.contains(&entry.id) {
            return false;
        }
        if self.entry_suspicious_isolated(entry, now) {
            return false;
        }
        if entry.refresh_failure_count >= MAX_FAILURES_PER_CREDENTIAL {
            return false;
        }
        if !self.entry_supports_model(entry, model) {
            return false;
        }
        if entry.inflight >= entry.max_concurrent {
            return false;
        }
        if !entry.cooldown_until.is_some_and(|deadline| deadline > now) {
            return false;
        }
        match entry.last_rate_limit_kind {
            Some(RateLimitKind::Normal429) => true,
            _ => false,
        }
    }

    fn entry_suspicious_isolated(&self, entry: &CredentialEntry, now: DateTime<Utc>) -> bool {
        self.config.scheduler.suspicious_isolation_enabled
            && entry
                .suspicious_isolation_until
                .is_some_and(|deadline| deadline > now)
    }

    fn health_score(&self, entry: &CredentialEntry, now: DateTime<Utc>) -> (u32, String) {
        if entry.disabled {
            return (0, "disabled".to_string());
        }
        if self.entry_suspicious_isolated(entry, now) {
            return (0, "suspicious isolated".to_string());
        }
        if entry.cooldown_until.is_some_and(|deadline| deadline > now) {
            return (0, "cooldown".to_string());
        }
        if entry.inflight >= entry.max_concurrent {
            return (0, "saturated".to_string());
        }

        let available_slots = entry.max_concurrent.saturating_sub(entry.inflight);
        let mut score = 100_i32;
        if entry.max_concurrent > 0 {
            let slot_ratio = available_slots as f64 / entry.max_concurrent as f64;
            score = score.min((slot_ratio * 100.0).round() as i32 + 20);
        }
        score -= (entry.recent_429_count.min(5) as i32) * 8;
        score -= (entry.recent_suspicious_count.min(5) as i32) * 20;
        let sticky_count = self.sticky_count_for(entry.id);
        score -= (sticky_count.min(5) as i32) * 2;
        if entry.success_count > 0 {
            score += 5;
        }
        let score = score.clamp(1, 100) as u32;
        let reason = format!(
            "slots {}/{}, 429 {}, suspicious {}, sticky {}",
            available_slots,
            entry.max_concurrent,
            entry.recent_429_count,
            entry.recent_suspicious_count,
            sticky_count
        );
        (score, reason)
    }

    fn entry_supports_model(&self, entry: &CredentialEntry, model: Option<&str>) -> bool {
        model
            .map(|model| entry.credentials.supports_model(model))
            .unwrap_or(true)
    }

    fn gc_sticky_bindings(&self) {
        let now = Utc::now();
        self.sticky_bindings
            .lock()
            .retain(|_, binding| binding.expires_at > now);
    }

    fn bind_session(&self, session_key: &str, account_id: u64) {
        let now = Utc::now();
        self.sticky_bindings.lock().insert(
            session_key.to_string(),
            StickyBinding {
                account_id,
                expires_at: now + Duration::seconds(STICKY_SESSION_TTL_SECS),
            },
        );
    }

    fn detach_session_binding_for_account(&self, account_id: u64) {
        self.sticky_bindings
            .lock()
            .retain(|_, binding| binding.account_id != account_id);
    }

    fn sticky_count_for(&self, account_id: u64) -> u32 {
        self.gc_sticky_bindings();
        self.sticky_bindings
            .lock()
            .values()
            .filter(|binding| binding.account_id == account_id)
            .count() as u32
    }

    fn mark_dispatch_selected(&self, id: u64, path: DispatchPath, used_soft_fallback: bool) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.last_dispatch_path = Some(path);
            if used_soft_fallback {
                entry.last_soft_fallback_at = Some(Utc::now().to_rfc3339());
            }
        }
    }

    /// 获取 API 调用上下文
    ///
    /// 返回绑定了 id、credentials 和 token 的调用上下文
    /// 确保整个 API 调用过程中使用一致的凭据信息
    ///
    /// 如果 Token 过期或即将过期，会自动刷新
    /// Token 刷新失败会累计到当前凭据，达到阈值后禁用并切换
    ///
    /// # 参数
    /// - `model`: 可选的模型名称，用于过滤支持该模型的凭据（如 opus 模型需要付费订阅）
    pub async fn acquire_context(&self, model: Option<&str>) -> anyhow::Result<CallContext> {
        self.acquire_context_with_options(AcquireOptions::new(model.map(|m| m.to_string())))
            .await
    }

    pub async fn acquire_context_with_options(
        &self,
        options: AcquireOptions,
    ) -> anyhow::Result<CallContext> {
        let total = self.total_count();
        let max_attempts = (total * MAX_FAILURES_PER_CREDENTIAL as usize).max(1);
        let mut attempt_count = 0;
        let mut options = options;

        loop {
            if attempt_count >= max_attempts {
                anyhow::bail!(
                    "所有凭据均无法获取有效 Token（启用: {}/{}, 可调度: {}）",
                    self.available_count(),
                    total,
                    self.schedulable_count()
                );
            }

            let selection = {
                let is_balanced = self.load_balancing_mode.lock().as_str() == "balanced";

                // balanced 模式：每次请求都重新均衡选择，不固定 current_id
                // priority 模式：优先使用 current_id 指向的凭据
                let current_hit = if is_balanced || options.strict_preferred_account {
                    None
                } else {
                    let entries = self.entries.lock();
                    let current_id = *self.current_id.lock();
                    entries
                        .iter()
                        .find(|e| {
                            e.id == current_id
                                && self.entry_policy_matches(e, options.scheduler_policy)
                                && self.entry_schedulable(
                                    e,
                                    options.model.as_deref(),
                                    &options.tried_account_ids,
                                )
                        })
                        .map(|e| {
                            let now = Utc::now();
                            SelectionResult {
                                id: e.id,
                                credentials: e.credentials.clone(),
                                dispatch_path: DispatchPath::Balanced,
                                used_soft_fallback: false,
                                account_state_at_start: self.dispatch_state_of(e, now),
                            }
                        })
                };

                if let Some(hit) = current_hit {
                    hit
                } else {
                    // 当前凭据不可用或 balanced 模式，根据负载均衡策略选择
                    let mut best = self.select_next_credential(&options);

                    // 没有可用凭据：如果是"自动禁用导致全灭"，做一次类似重启的自愈
                    if best.is_none() {
                        let mut entries = self.entries.lock();
                        if entries.iter().any(|e| {
                            e.disabled && e.disabled_reason == Some(DisabledReason::TooManyFailures)
                        }) {
                            tracing::warn!(
                                "所有凭据均已被自动禁用，执行自愈：重置失败计数并重新启用（等价于重启）"
                            );
                            for e in entries.iter_mut() {
                                if e.disabled_reason == Some(DisabledReason::TooManyFailures) {
                                    e.disabled = false;
                                    e.disabled_reason = None;
                                    e.failure_count = 0;
                                }
                            }
                            drop(entries);
                            best = self.select_next_credential(&options);
                        }
                    }

                    if let Some(best) = best {
                        // 更新 current_id
                        let mut current_id = self.current_id.lock();
                        *current_id = best.id;
                        best
                    } else {
                        let entries = self.entries.lock();
                        // 注意：必须在 bail! 之前计算 available_count，
                        // 因为 available_count() 会尝试获取 entries 锁，
                        // 而此时我们已经持有该锁，会导致死锁
                        let empty_tried = HashSet::new();
                        let enabled_count = entries.iter().filter(|e| !e.disabled).count();
                        let schedulable_count = entries
                            .iter()
                            .filter(|e| self.entry_policy_matches(e, options.scheduler_policy))
                            .filter(|e| self.entry_schedulable(e, None, &empty_tried))
                            .count();
                        anyhow::bail!(
                            "当前没有可直接调度的凭据（启用: {}/{}, 可调度: {}，可能全部处于冷却、阻塞、并发饱和、模型不兼容或本次请求已试过）",
                            enabled_count,
                            total,
                            schedulable_count
                        );
                    }
                }
            };
            let id = selection.id;
            let credentials = selection.credentials.clone();

            // 尝试获取/刷新 Token
            match self.try_ensure_token(id, &credentials).await {
                Ok(mut ctx) => {
                    self.acquire_slot(id, options.runtime_probe, selection.used_soft_fallback)?;
                    if let Some(session_key) = options.session_key.as_deref() {
                        self.bind_session(session_key, id);
                    }
                    ctx.lease = Some(DispatchLease::new(id, Arc::clone(&self.entries)));
                    ctx.dispatch_path = selection.dispatch_path;
                    ctx.used_soft_fallback = selection.used_soft_fallback;
                    ctx.account_state_at_start = selection.account_state_at_start;
                    self.mark_dispatch_selected(
                        id,
                        selection.dispatch_path,
                        selection.used_soft_fallback,
                    );
                    return Ok(ctx);
                }
                Err(e) => {
                    // refreshToken 永久失效 → 立即禁用，不累计重试
                    let has_available = if e.downcast_ref::<RefreshTokenInvalidError>().is_some() {
                        tracing::warn!("凭据 #{} refreshToken 永久失效: {}", id, e);
                        self.report_refresh_token_invalid(id)
                    } else {
                        tracing::warn!("凭据 #{} Token 刷新失败: {}", id, e);
                        self.report_refresh_failure(id)
                    };
                    options.tried_account_ids.insert(id);
                    attempt_count += 1;
                    if options.strict_preferred_account {
                        return Err(e);
                    }
                    if !has_available {
                        anyhow::bail!("当前没有可继续切换的凭据（0/{})", total);
                    }
                }
            }
        }
    }

    /// 选择优先级最高的未禁用凭据作为当前凭据（内部方法）
    ///
    /// 纯粹按优先级选择，不排除当前凭据，用于优先级变更后立即生效
    fn select_highest_priority(&self) {
        let entries = self.entries.lock();
        let mut current_id = self.current_id.lock();

        // 选择优先级最高的未禁用凭据（不排除当前凭据）
        if let Some(best) = entries
            .iter()
            .filter(|e| !e.disabled)
            .min_by_key(|e| e.credentials.priority)
        {
            if best.id != *current_id {
                tracing::info!(
                    "优先级变更后切换凭据: #{} -> #{}（优先级 {}）",
                    *current_id,
                    best.id,
                    best.credentials.priority
                );
                *current_id = best.id;
            }
        }
    }

    /// 尝试使用指定凭据获取有效 Token
    ///
    /// 使用双重检查锁定模式，确保同一时间只有一个刷新操作
    ///
    /// # Arguments
    /// * `id` - 凭据 ID，用于更新正确的条目
    /// * `credentials` - 凭据信息
    async fn try_ensure_token(
        &self,
        id: u64,
        credentials: &KiroCredentials,
    ) -> anyhow::Result<CallContext> {
        // API Key 凭据直接使用 kiro_api_key 作为 Bearer Token，无需刷新
        if credentials.is_api_key_credential() {
            let token = credentials
                .kiro_api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"))?;
            return Ok(CallContext {
                id,
                credentials: credentials.clone(),
                token,
                lease: None,
                dispatch_path: DispatchPath::Preferred,
                used_soft_fallback: false,
                account_state_at_start: DispatchState::Ready,
            });
        }

        // 第一次检查（无锁）：快速判断是否需要刷新
        let needs_refresh = is_token_expired(credentials) || is_token_expiring_soon(credentials);

        let creds = if needs_refresh {
            // 获取刷新锁，确保同一时间只有一个刷新操作
            let _guard = self.refresh_lock.lock().await;

            // 第二次检查：获取锁后重新读取凭据，因为其他请求可能已经完成刷新
            let current_creds = {
                let entries = self.entries.lock();
                entries
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.credentials.clone())
                    .ok_or_else(|| anyhow::anyhow!("凭据 #{} 不存在", id))?
            };

            if is_token_expired(&current_creds) || is_token_expiring_soon(&current_creds) {
                // 确实需要刷新
                let effective_proxy = self.random_proxy_from_pool();
                let new_creds =
                    refresh_token(&current_creds, &self.config, effective_proxy.as_ref()).await?;

                if is_token_expired(&new_creds) {
                    anyhow::bail!("刷新后的 Token 仍然无效或已过期");
                }

                // 更新凭据
                {
                    let mut entries = self.entries.lock();
                    if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                        entry.credentials = new_creds.clone();
                    }
                }

                // 回写凭据到文件（仅多凭据格式），失败只记录警告
                if let Err(e) = self.persist_credentials() {
                    tracing::warn!("Token 刷新后持久化失败（不影响本次请求）: {}", e);
                }

                new_creds
            } else {
                // 其他请求已经完成刷新，直接使用新凭据
                tracing::debug!("Token 已被其他请求刷新，跳过刷新");
                current_creds
            }
        } else {
            credentials.clone()
        };

        let token = creds
            .access_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("没有可用的 accessToken"))?;

        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.refresh_failure_count = 0;
            }
        }

        Ok(CallContext {
            id,
            credentials: creds,
            token,
            lease: None,
            dispatch_path: DispatchPath::Balanced,
            used_soft_fallback: false,
            account_state_at_start: DispatchState::Ready,
        })
    }

    /// 将凭据列表回写到源文件
    ///
    /// 仅在以下条件满足时回写：
    /// - 源文件是多凭据格式（数组）
    /// - credentials_path 已设置
    ///
    /// # Returns
    /// - `Ok(true)` - 成功写入文件
    /// - `Ok(false)` - 跳过写入（非多凭据格式或无路径配置）
    /// - `Err(_)` - 写入失败
    fn persist_credentials(&self) -> anyhow::Result<bool> {
        use anyhow::Context;

        // 仅多凭据格式才回写
        if !self.is_multiple_format {
            return Ok(false);
        }

        let path = match &self.credentials_path {
            Some(p) => p,
            None => return Ok(false),
        };

        // 收集所有凭据
        let credentials: Vec<KiroCredentials> = {
            let entries = self.entries.lock();
            entries
                .iter()
                .map(|e| {
                    let mut cred = e.credentials.clone();
                    cred.canonicalize_auth_method();
                    // 同步 disabled 状态到凭据对象
                    cred.disabled = e.disabled;
                    cred.disabled_reason = e.disabled_reason.map(disabled_reason_label);
                    cred.scheduler_policy = e.credentials.scheduler_policy;
                    cred
                })
                .collect()
        };

        // 序列化为 pretty JSON
        let json = serde_json::to_string_pretty(&credentials).context("序列化凭据失败")?;

        // 写入文件（在 Tokio runtime 内使用 block_in_place 避免阻塞 worker）
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| std::fs::write(path, &json))
                .with_context(|| format!("回写凭据文件失败: {:?}", path))?;
        } else {
            std::fs::write(path, &json).with_context(|| format!("回写凭据文件失败: {:?}", path))?;
        }

        tracing::debug!("已回写凭据到文件: {:?}", path);
        Ok(true)
    }

    /// 获取缓存目录（凭据文件所在目录）
    pub fn cache_dir(&self) -> Option<PathBuf> {
        self.credentials_path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    pub fn record_diagnostic(&self, update: RequestDiagnosticUpdate) {
        self.diagnostics.record(update);
    }

    pub fn update_diagnostic_tokens(
        &self,
        request_id: &str,
        input_tokens: Option<i32>,
        output_tokens: Option<i32>,
        cache_creation_input_tokens: Option<i32>,
        cache_read_input_tokens: Option<i32>,
        uncached_input_tokens: Option<i32>,
    ) {
        self.diagnostics.update_tokens(
            request_id,
            input_tokens,
            output_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
            uncached_input_tokens,
        );
    }

    pub fn query_diagnostics(&self, query: &DiagnosticsQuery) -> DiagnosticsRequestsResponse {
        self.diagnostics.query(query)
    }

    pub fn get_diagnostic(&self, request_id: &str) -> Option<RequestDiagnosticEntry> {
        self.diagnostics.get(request_id)
    }

    pub fn diagnostics_summary(&self, query: &DiagnosticsQuery) -> DiagnosticsSummaryResponse {
        self.diagnostics.summary(query)
    }

    /// 统计数据文件路径
    fn stats_path(&self) -> Option<PathBuf> {
        self.cache_dir().map(|d| d.join("kiro_stats.json"))
    }

    /// 从磁盘加载统计数据并应用到当前条目
    fn load_stats(&self) {
        let path = match self.stats_path() {
            Some(p) => p,
            None => return,
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return, // 首次运行时文件不存在
        };

        let stats: HashMap<String, StatsEntry> = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("解析统计缓存失败，将忽略: {}", e);
                return;
            }
        };

        let mut entries = self.entries.lock();
        for entry in entries.iter_mut() {
            if let Some(s) = stats.get(&entry.id.to_string()) {
                entry.success_count = s.success_count;
                entry.last_used_at = s.last_used_at.clone();
            }
        }
        *self.last_stats_save_at.lock() = Some(Instant::now());
        self.stats_dirty.store(false, Ordering::Relaxed);
        tracing::info!("已从缓存加载 {} 条统计数据", stats.len());
    }

    /// 将当前统计数据持久化到磁盘
    fn save_stats(&self) {
        let path = match self.stats_path() {
            Some(p) => p,
            None => return,
        };

        let stats: HashMap<String, StatsEntry> = {
            let entries = self.entries.lock();
            entries
                .iter()
                .map(|e| {
                    (
                        e.id.to_string(),
                        StatsEntry {
                            success_count: e.success_count,
                            last_used_at: e.last_used_at.clone(),
                        },
                    )
                })
                .collect()
        };

        match serde_json::to_string_pretty(&stats) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::warn!("保存统计缓存失败: {}", e);
                } else {
                    *self.last_stats_save_at.lock() = Some(Instant::now());
                    self.stats_dirty.store(false, Ordering::Relaxed);
                }
            }
            Err(e) => tracing::warn!("序列化统计数据失败: {}", e),
        }
    }

    /// 标记统计数据已更新，并按 debounce 策略决定是否立即落盘
    fn save_stats_debounced(&self) {
        self.stats_dirty.store(true, Ordering::Relaxed);

        let should_flush = {
            let last = *self.last_stats_save_at.lock();
            match last {
                Some(last_saved_at) => last_saved_at.elapsed() >= STATS_SAVE_DEBOUNCE,
                None => true,
            }
        };

        if should_flush {
            self.save_stats();
        }
    }

    fn acquire_slot(
        &self,
        id: u64,
        runtime_probe: bool,
        allow_cooldown_bypass: bool,
    ) -> anyhow::Result<()> {
        let mut entries = self.entries.lock();
        let entry = entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;

        if !runtime_probe
            && !allow_cooldown_bypass
            && entry
                .cooldown_until
                .is_some_and(|deadline| deadline > Utc::now())
        {
            anyhow::bail!("凭据 #{} 仍在冷却中", id);
        }
        if entry.inflight >= entry.max_concurrent {
            anyhow::bail!("凭据 #{} 并发已满", id);
        }
        entry.inflight += 1;
        Ok(())
    }

    pub fn release_slot(&self, lease: &mut DispatchLease) {
        if lease.released {
            return;
        }
        self.release_slot_by_id(lease.id);
        lease.released = true;
    }

    fn release_slot_by_id(&self, id: u64) {
        let mut entries = self.entries.lock();
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.inflight = entry.inflight.saturating_sub(1);
        }
    }

    fn apply_cooldown(&self, id: u64, kind: RateLimitKind) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return false,
            };

            let now = Utc::now();
            let cooldown_config = &self.config.rate_limit_cooldown;
            let cooldown_secs = match kind {
                RateLimitKind::Normal429 => cooldown_config.normal_429_seconds,
                RateLimitKind::SuspiciousActivity => {
                    let repeated = entry.last_suspicious_at.is_some_and(|last| {
                        now - last
                            <= Duration::seconds(
                                cooldown_config.suspicious_repeat_window_seconds.max(1),
                            )
                    });
                    if repeated {
                        cooldown_config.suspicious_repeated_seconds
                    } else {
                        cooldown_config.suspicious_first_seconds
                    }
                }
                RateLimitKind::Refresh429 => cooldown_config.refresh_429_seconds,
            };
            let next_deadline = now + Duration::seconds(cooldown_secs.max(1));
            entry.cooldown_until = Some(
                entry
                    .cooldown_until
                    .map(|current| current.max(next_deadline))
                    .unwrap_or(next_deadline),
            );
            entry.last_rate_limit_kind = Some(kind);
            entry.last_used_at = Some(now.to_rfc3339());
            match kind {
                RateLimitKind::Normal429 => {
                    entry.recent_429_count += 1;
                    entry.sticky_detached = false;
                }
                RateLimitKind::SuspiciousActivity => {
                    entry.recent_suspicious_count += 1;
                    entry.sticky_detached = true;
                    entry.last_suspicious_at = Some(now);
                    if self.config.scheduler.suspicious_isolation_enabled {
                        let isolation_deadline = now
                            + Duration::seconds(
                                self.config
                                    .scheduler
                                    .suspicious_isolation_seconds
                                    .max(cooldown_secs)
                                    .max(1),
                            );
                        entry.suspicious_isolation_until = Some(
                            entry
                                .suspicious_isolation_until
                                .map(|current| current.max(isolation_deadline))
                                .unwrap_or(isolation_deadline),
                        );
                    }
                    self.detach_session_binding_for_account(id);
                }
                RateLimitKind::Refresh429 => {
                    entry.sticky_detached = false;
                }
            }
            entries.iter().any(|e| !e.disabled)
        };
        self.save_stats_debounced();
        result
    }

    fn clear_runtime_block(&self, entry: &mut CredentialEntry) {
        entry.cooldown_until = None;
        entry.last_rate_limit_kind = None;
        entry.recent_429_count = 0;
        entry.sticky_detached = false;
        if entry
            .suspicious_isolation_until
            .is_some_and(|deadline| deadline <= Utc::now())
        {
            entry.suspicious_isolation_until = None;
            entry.recent_suspicious_count = 0;
            entry.last_suspicious_at = None;
        }
    }

    fn dispatch_state_of(&self, entry: &CredentialEntry, now: DateTime<Utc>) -> DispatchState {
        if entry.disabled {
            DispatchState::Disabled
        } else if entry.refresh_failure_count >= MAX_FAILURES_PER_CREDENTIAL {
            DispatchState::Blocked
        } else if entry.cooldown_until.is_some_and(|deadline| deadline > now) {
            DispatchState::Cooldown
        } else if entry.inflight >= entry.max_concurrent {
            DispatchState::Saturated
        } else {
            DispatchState::Ready
        }
    }

    fn account_status_of(&self, entry: &CredentialEntry, now: DateTime<Utc>) -> AccountStatus {
        if entry.disabled_reason == Some(DisabledReason::Banned) {
            AccountStatus::Banned
        } else if entry.disabled {
            AccountStatus::Disabled
        } else if entry.cooldown_until.is_some_and(|deadline| deadline > now) {
            AccountStatus::RateLimited
        } else {
            AccountStatus::Normal
        }
    }

    /// 报告指定凭据 API 调用成功
    ///
    /// 重置该凭据的失败计数
    ///
    /// # Arguments
    /// * `id` - 凭据 ID（来自 CallContext）
    pub fn report_success(&self, id: u64) {
        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.failure_count = 0;
                entry.refresh_failure_count = 0;
                self.clear_runtime_block(entry);
                entry.success_count += 1;
                entry.last_used_at = Some(Utc::now().to_rfc3339());
                tracing::debug!(
                    "凭据 #{} API 调用成功（累计 {} 次）",
                    id,
                    entry.success_count
                );
            }
        }
        self.save_stats_debounced();
    }

    /// 报告指定凭据 API 调用失败
    ///
    /// 增加失败计数，达到阈值时禁用凭据并切换到优先级最高的可用凭据
    /// 返回是否还有可用凭据可以重试
    ///
    /// # Arguments
    /// * `id` - 凭据 ID（来自 CallContext）
    pub fn report_failure(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.failure_count += 1;
            entry.last_used_at = Some(Utc::now().to_rfc3339());
            let failure_count = entry.failure_count;

            tracing::warn!(
                "凭据 #{} API 调用失败（{}/{}）",
                id,
                failure_count,
                MAX_FAILURES_PER_CREDENTIAL
            );

            if failure_count >= MAX_FAILURES_PER_CREDENTIAL {
                entry.disabled = true;
                entry.disabled_reason = Some(DisabledReason::TooManyFailures);
                tracing::error!("凭据 #{} 已连续失败 {} 次，已被禁用", id, failure_count);

                // 切换到优先级最高的可用凭据
                if let Some(next) = entries
                    .iter()
                    .filter(|e| !e.disabled)
                    .min_by_key(|e| e.credentials.priority)
                {
                    *current_id = next.id;
                    tracing::info!(
                        "已切换到凭据 #{}（优先级 {}）",
                        next.id,
                        next.credentials.priority
                    );
                } else {
                    tracing::error!("所有凭据均已禁用！");
                }
            }

            entries.iter().any(|e| !e.disabled)
        };
        self.save_stats_debounced();
        result
    }

    /// 报告上游明确返回账号封禁/暂停/停用。
    ///
    /// 封号是硬禁用状态，不等待连续失败阈值。
    pub fn report_banned(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled && entry.disabled_reason == Some(DisabledReason::Banned) {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.last_used_at = Some(Utc::now().to_rfc3339());
            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::Banned);
            entry.failure_count = MAX_FAILURES_PER_CREDENTIAL;
            self.clear_runtime_block(entry);

            tracing::error!("凭据 #{} 检测到账号封禁，已自动标记为封号并禁用", id);

            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        if let Err(e) = self.persist_credentials() {
            tracing::warn!("封号状态持久化失败（内存状态已生效）: {}", e);
        }
        result
    }

    pub fn report_rate_limited(&self, id: u64, kind: RateLimitKind) -> bool {
        self.apply_cooldown(id, kind)
    }

    pub fn report_normal_429_short_cooldown(&self, id: u64, cooldown_ms: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return false,
            };

            let now = Utc::now();
            let cooldown = chrono::Duration::milliseconds(cooldown_ms.max(1) as i64);
            let next_deadline = now + cooldown;
            entry.cooldown_until = Some(
                entry
                    .cooldown_until
                    .map(|current| current.max(next_deadline))
                    .unwrap_or(next_deadline),
            );
            entry.last_rate_limit_kind = Some(RateLimitKind::Normal429);
            entry.last_used_at = Some(now.to_rfc3339());
            entry.recent_429_count += 1;
            entry.sticky_detached = false;
            entries.iter().any(|e| !e.disabled)
        };
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据额度已用尽
    ///
    /// 用于处理 402 Payment Required 且 reason 为 `MONTHLY_REQUEST_COUNT` 的场景：
    /// - 立即禁用该凭据（不等待连续失败阈值）
    /// - 切换到下一个可用凭据继续重试
    /// - 返回是否还有可用凭据
    pub fn report_quota_exhausted(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::QuotaExceeded);
            entry.last_used_at = Some(Utc::now().to_rfc3339());
            // 设为阈值，便于在管理面板中直观看到该凭据已不可用
            entry.failure_count = MAX_FAILURES_PER_CREDENTIAL;

            tracing::error!("凭据 #{} 额度已用尽（MONTHLY_REQUEST_COUNT），已被禁用", id);

            // 切换到优先级最高的可用凭据
            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        if let Err(e) = self.persist_credentials() {
            tracing::warn!("额度耗尽状态持久化失败（内存状态已生效）: {}", e);
        }
        result
    }

    /// 报告指定凭据刷新 Token 失败。
    ///
    /// 连续刷新失败达到阈值后禁用凭据并切换，阈值内保持当前凭据不切换，
    /// 与 API 401/403 的累计失败策略保持一致。
    pub fn report_refresh_failure(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.last_used_at = Some(Utc::now().to_rfc3339());
            entry.refresh_failure_count += 1;
            let refresh_failure_count = entry.refresh_failure_count;
            if entry.refresh_failure_count == 1 {
                entry.last_rate_limit_kind = Some(RateLimitKind::Refresh429);
            }

            tracing::warn!(
                "凭据 #{} Token 刷新失败（{}/{}）",
                id,
                refresh_failure_count,
                MAX_FAILURES_PER_CREDENTIAL
            );

            if refresh_failure_count < MAX_FAILURES_PER_CREDENTIAL {
                entries.iter().any(|e| !e.disabled)
            } else {
                entry.disabled = true;
                entry.disabled_reason = Some(DisabledReason::TooManyRefreshFailures);

                tracing::error!(
                    "凭据 #{} Token 已连续刷新失败 {} 次，已被禁用",
                    id,
                    refresh_failure_count
                );

                if let Some(next) = entries
                    .iter()
                    .filter(|e| !e.disabled)
                    .min_by_key(|e| e.credentials.priority)
                {
                    *current_id = next.id;
                    tracing::info!(
                        "已切换到凭据 #{}（优先级 {}）",
                        next.id,
                        next.credentials.priority
                    );
                    true
                } else {
                    tracing::error!("所有凭据均已禁用！");
                    false
                }
            }
        };
        self.apply_cooldown(id, RateLimitKind::Refresh429);
        self.save_stats_debounced();
        result
    }

    /// 报告指定凭据的 refreshToken 永久失效（invalid_grant）。
    ///
    /// 立即禁用凭据，不累计、不重试。
    /// 返回是否还有可用凭据。
    pub fn report_refresh_token_invalid(&self, id: u64) -> bool {
        let result = {
            let mut entries = self.entries.lock();
            let mut current_id = self.current_id.lock();

            let entry = match entries.iter_mut().find(|e| e.id == id) {
                Some(e) => e,
                None => return entries.iter().any(|e| !e.disabled),
            };

            if entry.disabled {
                return entries.iter().any(|e| !e.disabled);
            }

            entry.last_used_at = Some(Utc::now().to_rfc3339());
            entry.disabled = true;
            entry.disabled_reason = Some(DisabledReason::InvalidRefreshToken);

            tracing::error!(
                "凭据 #{} refreshToken 已失效 (invalid_grant)，已立即禁用",
                id
            );

            if let Some(next) = entries
                .iter()
                .filter(|e| !e.disabled)
                .min_by_key(|e| e.credentials.priority)
            {
                *current_id = next.id;
                tracing::info!(
                    "已切换到凭据 #{}（优先级 {}）",
                    next.id,
                    next.credentials.priority
                );
                true
            } else {
                tracing::error!("所有凭据均已禁用！");
                false
            }
        };
        self.save_stats_debounced();
        result
    }

    /// 切换到优先级最高的可用凭据
    ///
    /// 返回是否成功切换
    pub fn switch_to_next(&self) -> bool {
        let entries = self.entries.lock();
        let mut current_id = self.current_id.lock();

        // 选择优先级最高的未禁用凭据（排除当前凭据）
        if let Some(next) = entries
            .iter()
            .filter(|e| !e.disabled && e.id != *current_id)
            .min_by_key(|e| e.credentials.priority)
        {
            *current_id = next.id;
            tracing::info!(
                "已切换到凭据 #{}（优先级 {}）",
                next.id,
                next.credentials.priority
            );
            true
        } else {
            // 没有其他可用凭据，检查当前凭据是否可用
            entries.iter().any(|e| e.id == *current_id && !e.disabled)
        }
    }

    // ========================================================================
    // Admin API 方法
    // ========================================================================

    /// 获取管理器状态快照（用于 Admin API）
    pub fn snapshot(&self) -> ManagerSnapshot {
        let entries = self.entries.lock();
        let current_id = *self.current_id.lock();
        let now = Utc::now();
        let empty_tried = HashSet::new();
        let enabled_count = entries.iter().filter(|e| !e.disabled).count();
        let schedulable_count = entries
            .iter()
            .filter(|e| self.entry_schedulable(e, None, &empty_tried))
            .count();

        ManagerSnapshot {
            entries: entries
                .iter()
                .map(|e| {
                    let suspicious_isolated = self.entry_suspicious_isolated(e, now);
                    let isolation_remaining_ms =
                        e.suspicious_isolation_until.and_then(|deadline| {
                            if deadline > now {
                                Some((deadline - now).num_milliseconds().max(0) as u64)
                            } else {
                                None
                            }
                        });
                    let (health_score, weight_reason) = self.health_score(e, now);
                    let dispatch_weight = health_score as f64 / 100.0;
                    CredentialEntrySnapshot {
                        id: e.id,
                        priority: e.credentials.priority,
                        scheduler_policy: e.credentials.scheduler_policy,
                        disabled: e.disabled,
                        failure_count: e.failure_count,
                        auth_method: if e.credentials.is_api_key_credential() {
                            Some("api_key".to_string())
                        } else {
                            e.credentials.auth_method.as_deref().map(|m| {
                                if m.eq_ignore_ascii_case("builder-id")
                                    || m.eq_ignore_ascii_case("iam")
                                {
                                    "idc".to_string()
                                } else {
                                    m.to_string()
                                }
                            })
                        },
                        has_profile_arn: e.credentials.profile_arn.is_some(),
                        expires_at: if e.credentials.is_api_key_credential() {
                            None // API Key 凭据本地不维护过期时间（服务端策略未知）
                        } else {
                            e.credentials.expires_at.clone()
                        },
                        refresh_token_hash: if e.credentials.is_api_key_credential() {
                            None
                        } else {
                            e.credentials.refresh_token.as_deref().map(sha256_hex)
                        },
                        api_key_hash: if e.credentials.is_api_key_credential() {
                            e.credentials.kiro_api_key.as_deref().map(sha256_hex)
                        } else {
                            None
                        },
                        masked_api_key: if e.credentials.is_api_key_credential() {
                            e.credentials.kiro_api_key.as_deref().map(mask_api_key)
                        } else {
                            None
                        },
                        email: e.credentials.email.clone(),
                        success_count: e.success_count,
                        last_used_at: e.last_used_at.clone(),
                        has_proxy: false,
                        proxy_url: None,
                        proxy_mode: Some("pool".to_string()),
                        proxy_id: None,
                        refresh_failure_count: e.refresh_failure_count,
                        disabled_reason: e.disabled_reason.map(disabled_reason_label),
                        account_status: self.account_status_of(e, now).to_string(),
                        endpoint: e.credentials.endpoint.clone(),
                        dispatch_state: self.dispatch_state_of(e, now).to_string(),
                        current_concurrent: e.inflight,
                        max_concurrent: e.max_concurrent,
                        cooldown_remaining_ms: e.cooldown_until.and_then(|deadline| {
                            if deadline > now {
                                Some((deadline - now).num_milliseconds().max(0) as u64)
                            } else {
                                None
                            }
                        }),
                        last_rate_limit_kind: e.last_rate_limit_kind.map(|kind| {
                            match kind {
                                RateLimitKind::Normal429 => "normal_429",
                                RateLimitKind::SuspiciousActivity => "suspicious_activity",
                                RateLimitKind::Refresh429 => "refresh_429",
                            }
                            .to_string()
                        }),
                        recent_429_count: e.recent_429_count,
                        recent_suspicious_count: e.recent_suspicious_count,
                        sticky_session_count: self.sticky_count_for(e.id),
                        sticky_detached: e.sticky_detached,
                        dispatch_path: e.last_dispatch_path.map(|path| path.to_string()),
                        soft_fallback_eligible: self.soft_fallback_eligible(
                            e,
                            None,
                            &HashSet::new(),
                            now,
                        ),
                        last_soft_fallback_at: e.last_soft_fallback_at.clone(),
                        suspicious_isolated,
                        isolation_remaining_ms,
                        health_score,
                        dispatch_weight,
                        weight_reason,
                        subscription_title: e.credentials.subscription_title.clone(),
                        available_models: e.credentials.available_models.clone(),
                    }
                })
                .collect(),
            current_id,
            total: entries.len(),
            available: schedulable_count,
            enabled_count,
            schedulable_count,
        }
    }

    /// 设置凭据禁用状态（Admin API）
    pub fn set_disabled(&self, id: u64, disabled: bool) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            entry.disabled = disabled;
            if !disabled {
                // 启用时重置失败计数
                entry.failure_count = 0;
                entry.refresh_failure_count = 0;
                entry.disabled_reason = None;
            } else {
                entry.disabled_reason = Some(DisabledReason::Manual);
            }
        }
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 设置凭据优先级（Admin API）
    ///
    /// 修改优先级后会立即按新优先级重新选择当前凭据。
    /// 即使持久化失败，内存中的优先级和当前凭据选择也会生效。
    pub fn set_priority(&self, id: u64, priority: u32) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            entry.credentials.priority = priority;
        }
        // 立即按新优先级重新选择当前凭据（无论持久化是否成功）
        self.select_highest_priority();
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 重置凭据失败计数并重新启用（Admin API）
    pub fn reset_and_enable(&self, id: u64) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            if entry.disabled_reason == Some(DisabledReason::InvalidConfig) {
                anyhow::bail!("凭据 #{} 因配置无效被禁用，请修正配置后重启服务", id);
            }
            entry.failure_count = 0;
            entry.refresh_failure_count = 0;
            entry.disabled = false;
            entry.disabled_reason = None;
        }
        // 持久化更改
        self.persist_credentials()?;
        Ok(())
    }

    /// 手动恢复本地运行态阻塞，不表示上游账号已恢复。
    ///
    /// 仅清理本地失败计数、冷却态和刷新阻塞，不会覆盖配置无效等硬禁用原因。
    pub fn recover_runtime_state(&self, id: u64) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;

            if entry.disabled_reason == Some(DisabledReason::InvalidConfig) {
                anyhow::bail!("凭据 #{} 因配置无效被禁用，请修正配置后重启服务", id);
            }

            if matches!(
                entry.disabled_reason,
                Some(DisabledReason::QuotaExceeded | DisabledReason::InvalidRefreshToken)
            ) {
                anyhow::bail!("凭据 #{} 当前状态不支持手动恢复", id);
            }

            entry.failure_count = 0;
            entry.refresh_failure_count = 0;
            entry.disabled = false;
            entry.disabled_reason = None;
            self.clear_runtime_block(entry);
        }

        self.persist_credentials()?;
        Ok(())
    }

    /// 设置账号并发上限并立即生效。
    pub fn set_max_concurrent(&self, id: u64, max_concurrent: u32) -> anyhow::Result<()> {
        let sanitized = max_concurrent.max(1);
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            entry.max_concurrent = sanitized;
            entry.credentials.max_concurrent = Some(sanitized);
        }
        self.persist_credentials()?;
        Ok(())
    }

    pub fn set_scheduler_policy(
        &self,
        id: u64,
        scheduler_policy: SchedulerPolicy,
    ) -> anyhow::Result<()> {
        {
            let mut entries = self.entries.lock();
            let entry = entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;
            entry.credentials.scheduler_policy = scheduler_policy;
        }
        self.persist_credentials()?;
        Ok(())
    }

    /// 获取指定凭据的使用额度（Admin API）
    pub async fn get_usage_limits_for(&self, id: u64) -> anyhow::Result<UsageLimitsResponse> {
        let token = self.ensure_token_for(id).await?;

        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        let effective_proxy = self.random_proxy_from_pool();
        let usage_limits =
            get_usage_limits(&credentials, &self.config, &token, effective_proxy.as_ref()).await?;
        if let Some(email) = usage_limits.email() {
            self.update_account_email_if_missing_or_changed(id, email);
        }

        let available_models = match get_available_models(
            &credentials,
            &self.config,
            &token,
            effective_proxy.as_ref(),
        )
        .await
        {
            Ok(models) => Some(models),
            Err(err) => {
                if is_account_banned_response(&err.to_string()) {
                    self.report_banned(id);
                }
                tracing::warn!("获取凭据 #{} 可用模型列表失败: {}", id, err);
                None
            }
        };

        self.update_account_model_capabilities(
            id,
            usage_limits.subscription_title(),
            available_models,
        );

        Ok(usage_limits)
    }

    async fn ensure_token_for(&self, id: u64) -> anyhow::Result<String> {
        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        if credentials.is_api_key_credential() {
            return credentials
                .kiro_api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"));
        }

        let needs_refresh = is_token_expired(&credentials) || is_token_expiring_soon(&credentials);
        if !needs_refresh {
            return credentials
                .access_token
                .ok_or_else(|| anyhow::anyhow!("凭据无 access_token"));
        }

        let _guard = self.refresh_lock.lock().await;
        let current_creds = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        if !(is_token_expired(&current_creds) || is_token_expiring_soon(&current_creds)) {
            return current_creds
                .access_token
                .ok_or_else(|| anyhow::anyhow!("凭据无 access_token"));
        }

        let effective_proxy = self.random_proxy_from_pool();
        let new_creds =
            refresh_token(&current_creds, &self.config, effective_proxy.as_ref()).await?;
        let token = new_creds
            .access_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("刷新后无 access_token"))?;
        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.credentials = new_creds;
                entry.refresh_failure_count = 0;
            }
        }
        if let Err(e) = self.persist_credentials() {
            tracing::warn!("Token 刷新后持久化失败（不影响本次请求）: {}", e);
        }

        Ok(token)
    }

    async fn refresh_email_for(&self, id: u64) -> anyhow::Result<Option<String>> {
        let token = self.ensure_token_for(id).await?;
        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };
        let effective_proxy = self.random_proxy_from_pool();
        let user_info =
            get_user_info(&credentials, &self.config, &token, effective_proxy.as_ref()).await?;
        if let Some(email) = user_info.email.as_deref().and_then(normalize_email) {
            self.update_account_email_if_missing_or_changed(id, &email);
            Ok(Some(email))
        } else {
            Ok(None)
        }
    }

    /// 刷新指定凭据的可用模型列表（Admin API）
    pub async fn refresh_available_models_for(&self, id: u64) -> anyhow::Result<Vec<String>> {
        let token = self.ensure_token_for(id).await?;

        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        let effective_proxy = self.random_proxy_from_pool();
        let models = match get_available_models(
            &credentials,
            &self.config,
            &token,
            effective_proxy.as_ref(),
        )
        .await
        {
            Ok(models) => models,
            Err(err) => {
                if is_account_banned_response(&err.to_string()) {
                    self.report_banned(id);
                }
                return Err(err);
            }
        };

        self.update_account_model_capabilities(
            id,
            credentials.subscription_title.as_deref(),
            Some(models.clone()),
        );

        Ok(models)
    }

    fn update_account_model_capabilities(
        &self,
        id: u64,
        subscription_title: Option<&str>,
        available_models: Option<Vec<String>>,
    ) {
        let changed = {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                let new_title = subscription_title.map(str::to_string);
                let old_title = entry.credentials.subscription_title.clone();
                let title_changed = old_title != new_title;
                let models_changed = available_models.as_ref().is_some_and(|models| {
                    entry.credentials.available_models.as_ref() != Some(models)
                });

                if title_changed || models_changed {
                    entry.credentials.subscription_title = new_title.clone();
                    if let Some(models) = available_models.clone() {
                        entry.credentials.available_models = Some(models);
                    }
                    tracing::info!(
                        "凭据 #{} 模型能力已更新: subscription={:?}, models={:?}",
                        id,
                        new_title,
                        entry.credentials.available_models
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if changed {
            if let Err(e) = self.persist_credentials() {
                tracing::warn!("账号模型能力更新后持久化失败（不影响本次请求）: {}", e);
            }
        }
    }

    fn update_account_email_if_missing_or_changed(&self, id: u64, email: &str) {
        let Some(email) = normalize_email(email) else {
            return;
        };

        let changed = {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                if credential_email_key(&entry.credentials).as_deref() != Some(email.as_str()) {
                    entry.credentials.email = Some(email.clone());
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if changed {
            tracing::info!("凭据 #{} 邮箱已更新: {}", id, email);
            if let Err(e) = self.persist_credentials() {
                tracing::warn!("账号邮箱更新后持久化失败（不影响本次请求）: {}", e);
            }
        }
    }

    fn duplicate_email_exists(&self, email: &str, exclude_id: Option<u64>) -> bool {
        let Some(email) = normalize_email(email) else {
            return false;
        };
        let entries = self.entries.lock();
        entries.iter().any(|entry| {
            exclude_id.is_none_or(|id| entry.id != id)
                && credential_email_key(&entry.credentials).as_deref() == Some(email.as_str())
        })
    }

    /// 添加新凭据（Admin API）
    ///
    /// # 流程
    /// 1. 验证凭据基本字段（API Key: kiroApiKey 不为空; OAuth: refreshToken 不为空）
    /// 2. OAuth: 尝试刷新 Token 验证凭据有效性; API Key: 跳过刷新
    /// 3. 调用账号信息 API 获取邮箱，按邮箱检测重复
    /// 4. 分配新 ID（当前最大 ID + 1）
    /// 5. 添加到 entries 列表
    /// 6. 持久化到配置文件
    ///
    /// # 返回
    /// - `Ok(u64)` - 新凭据 ID
    /// - `Err(_)` - 验证失败或添加失败
    pub async fn add_credential(&self, new_cred: KiroCredentials) -> anyhow::Result<u64> {
        // 1. 基本验证
        if new_cred.is_api_key_credential() {
            let api_key = new_cred
                .kiro_api_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"))?;
            if api_key.is_empty() {
                anyhow::bail!("kiroApiKey 为空");
            }
        } else {
            validate_refresh_token(&new_cred)?;
        }

        if let Some(email) = credential_email_key(&new_cred) {
            if self.duplicate_email_exists(&email, None) {
                anyhow::bail!("账号已存在（邮箱重复: {}）", email);
            }
        }

        // 2. 验证凭据有效性（API Key 无需网络刷新）
        let mut validated_cred = if new_cred.is_api_key_credential() {
            new_cred.clone()
        } else {
            let effective_proxy = self.random_proxy_from_pool();
            refresh_token(&new_cred, &self.config, effective_proxy.as_ref()).await?
        };

        let token = if validated_cred.is_api_key_credential() {
            validated_cred
                .kiro_api_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("API Key 凭据缺少 kiroApiKey"))?
        } else {
            validated_cred
                .access_token
                .clone()
                .ok_or_else(|| anyhow::anyhow!("刷新后无 access_token"))?
        };

        // 3. 添加账号时主动请求账号邮箱，并按邮箱去重
        let effective_proxy = self.random_proxy_from_pool();
        let fetched_email = match get_user_info(
            &validated_cred,
            &self.config,
            &token,
            effective_proxy.as_ref(),
        )
        .await
        {
            Ok(info) => info.email.and_then(|email| normalize_email(&email)),
            Err(err) => {
                tracing::warn!("添加凭据时获取邮箱失败，保留输入邮箱: {}", err);
                None
            }
        };
        validated_cred.email = fetched_email
            .clone()
            .or_else(|| credential_email_key(&new_cred));

        if let Some(email) = validated_cred.email.as_deref() {
            if self.duplicate_email_exists(email, None) {
                anyhow::bail!("账号已存在（邮箱重复: {}）", email);
            }
        } else {
            tracing::warn!("添加凭据时未获取到邮箱，无法执行邮箱去重");
        }

        // 4. 分配新 ID
        let new_id = {
            let entries = self.entries.lock();
            entries.iter().map(|e| e.id).max().unwrap_or(0) + 1
        };

        // 5. 设置 ID 并保留用户输入的元数据
        validated_cred.id = Some(new_id);
        validated_cred.priority = new_cred.priority;
        validated_cred.auth_method = new_cred.auth_method.map(|m| {
            if m.eq_ignore_ascii_case("builder-id") || m.eq_ignore_ascii_case("iam") {
                "idc".to_string()
            } else {
                m
            }
        });
        validated_cred.client_id = new_cred.client_id;
        validated_cred.client_secret = new_cred.client_secret;
        validated_cred.region = new_cred.region;
        validated_cred.auth_region = new_cred.auth_region;
        validated_cred.api_region = new_cred.api_region;
        validated_cred.machine_id = new_cred.machine_id;
        validated_cred.proxy_url = None;
        validated_cred.proxy_username = None;
        validated_cred.proxy_password = None;
        validated_cred.proxy_mode = None;
        validated_cred.proxy_id = None;
        validated_cred.kiro_api_key = new_cred.kiro_api_key;
        validated_cred.max_concurrent = new_cred.max_concurrent;
        let max_concurrent = validated_cred
            .max_concurrent
            .unwrap_or(DEFAULT_MAX_CONCURRENT)
            .max(1);

        {
            let mut entries = self.entries.lock();
            entries.push(CredentialEntry {
                id: new_id,
                credentials: validated_cred,
                failure_count: 0,
                refresh_failure_count: 0,
                disabled: false,
                disabled_reason: None,
                success_count: 0,
                last_used_at: None,
                inflight: 0,
                max_concurrent,
                cooldown_until: None,
                last_rate_limit_kind: None,
                recent_429_count: 0,
                recent_suspicious_count: 0,
                last_suspicious_at: None,
                suspicious_isolation_until: None,
                sticky_detached: false,
                last_dispatch_path: None,
                last_soft_fallback_at: None,
            });
        }

        // 6. 持久化
        self.persist_credentials()?;

        tracing::info!("成功添加凭据 #{}", new_id);
        Ok(new_id)
    }

    /// 删除凭据（Admin API）
    ///
    /// # 行为
    /// 1. 验证凭据存在
    /// 2. 从 entries 移除
    /// 3. 如果删除的是当前凭据，切换到优先级最高的可用凭据
    /// 4. 如果删除后没有凭据，将 current_id 重置为 0
    /// 5. 持久化到文件
    ///
    /// # 返回
    /// - `Ok(())` - 删除成功
    /// - `Err(_)` - 凭据不存在或持久化失败
    pub fn delete_credential(&self, id: u64) -> anyhow::Result<()> {
        let was_current = {
            let mut entries = self.entries.lock();

            // 查找凭据
            let _entry = entries
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?;

            // 记录是否是当前凭据
            let current_id = *self.current_id.lock();
            let was_current = current_id == id;

            // 删除凭据
            entries.retain(|e| e.id != id);

            was_current
        };

        // 如果删除的是当前凭据，切换到优先级最高的可用凭据
        if was_current {
            self.select_highest_priority();
        }

        // 如果删除后没有任何凭据，将 current_id 重置为 0（与初始化行为保持一致）
        {
            let entries = self.entries.lock();
            if entries.is_empty() {
                let mut current_id = self.current_id.lock();
                *current_id = 0;
                tracing::info!("所有凭据已删除，current_id 已重置为 0");
            }
        }

        // 持久化更改
        self.persist_credentials()?;

        // 立即回写统计数据，清除已删除凭据的残留条目
        self.save_stats();

        tracing::info!("已删除凭据 #{}", id);
        Ok(())
    }

    /// 强制刷新指定凭据的 Token（Admin API）
    ///
    /// 无条件调用上游 API 重新获取 access token，不检查是否过期。
    /// 适用于排查问题、Token 异常但未过期、主动更新凭据状态等场景。
    pub async fn force_refresh_token_for(&self, id: u64) -> anyhow::Result<()> {
        let credentials = {
            let entries = self.entries.lock();
            entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.credentials.clone())
                .ok_or_else(|| anyhow::anyhow!("凭据不存在: {}", id))?
        };

        // 获取刷新锁防止并发刷新
        let _guard = self.refresh_lock.lock().await;

        // 无条件调用 refresh_token
        let effective_proxy = self.random_proxy_from_pool();
        let new_creds = refresh_token(&credentials, &self.config, effective_proxy.as_ref()).await?;

        // 更新 entries 中对应凭据
        {
            let mut entries = self.entries.lock();
            if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
                entry.credentials = new_creds;
                entry.refresh_failure_count = 0;
            }
        }

        // 持久化
        if let Err(e) = self.persist_credentials() {
            tracing::warn!("强制刷新 Token 后持久化失败: {}", e);
        }

        tracing::info!("凭据 #{} Token 已强制刷新", id);
        Ok(())
    }

    /// 获取负载均衡模式（Admin API）
    pub fn get_load_balancing_mode(&self) -> String {
        self.load_balancing_mode.lock().clone()
    }

    fn persist_load_balancing_mode(&self, mode: &str) -> anyhow::Result<()> {
        use anyhow::Context;

        let config_path = match self.config.config_path() {
            Some(path) => path.to_path_buf(),
            None => {
                tracing::warn!("配置文件路径未知，负载均衡模式仅在当前进程生效: {}", mode);
                return Ok(());
            }
        };

        let mut config = Config::load(&config_path)
            .with_context(|| format!("重新加载配置失败: {}", config_path.display()))?;
        config.load_balancing_mode = mode.to_string();
        config
            .save()
            .with_context(|| format!("持久化负载均衡模式失败: {}", config_path.display()))?;

        Ok(())
    }

    /// 设置负载均衡模式（Admin API）
    pub fn set_load_balancing_mode(&self, mode: String) -> anyhow::Result<()> {
        // 验证模式值
        if mode != "priority" && mode != "balanced" {
            anyhow::bail!("无效的负载均衡模式: {}", mode);
        }

        let previous_mode = self.get_load_balancing_mode();
        if previous_mode == mode {
            return Ok(());
        }

        *self.load_balancing_mode.lock() = mode.clone();

        if let Err(err) = self.persist_load_balancing_mode(&mode) {
            *self.load_balancing_mode.lock() = previous_mode;
            return Err(err);
        }

        tracing::info!("负载均衡模式已设置为: {}", mode);
        Ok(())
    }
}

impl Drop for MultiTokenManager {
    fn drop(&mut self) {
        if self.stats_dirty.load(Ordering::Relaxed) {
            self.save_stats();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_token_expired_with_expired_token() {
        let mut credentials = KiroCredentials::default();
        credentials.expires_at = Some("2020-01-01T00:00:00Z".to_string());
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_with_valid_token() {
        let mut credentials = KiroCredentials::default();
        let future = Utc::now() + Duration::hours(1);
        credentials.expires_at = Some(future.to_rfc3339());
        assert!(!is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_within_5_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(3);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_no_expires_at() {
        let credentials = KiroCredentials::default();
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expiring_soon_within_10_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(8);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(is_token_expiring_soon(&credentials));
    }

    #[test]
    fn test_is_token_expiring_soon_beyond_10_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(15);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(!is_token_expiring_soon(&credentials));
    }

    #[test]
    fn test_validate_refresh_token_missing() {
        let credentials = KiroCredentials::default();
        let result = validate_refresh_token(&credentials);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_refresh_token_valid() {
        let mut credentials = KiroCredentials::default();
        credentials.refresh_token = Some("a".repeat(150));
        let result = validate_refresh_token(&credentials);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sha256_hex() {
        let result = sha256_hex("test");
        assert_eq!(
            result,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[test]
    fn test_duplicate_email_exists_normalizes_case_and_space() {
        let config = Config::default();
        let mut existing = KiroCredentials::default();
        existing.email = Some(" User@Example.COM ".to_string());

        let manager = MultiTokenManager::new(config, vec![existing], None, false).unwrap();

        assert!(manager.duplicate_email_exists("user@example.com", None));
        assert!(!manager.duplicate_email_exists("other@example.com", None));
    }

    #[test]
    fn test_deduplicate_existing_credentials_by_email_removes_later_duplicates() {
        let config = Config::default();
        let mut first = KiroCredentials::default();
        first.id = Some(1);
        first.email = Some("user@example.com".to_string());
        first.priority = 1;
        let mut duplicate = KiroCredentials::default();
        duplicate.id = Some(2);
        duplicate.email = Some(" USER@example.com ".to_string());
        duplicate.priority = 2;
        let mut unique = KiroCredentials::default();
        unique.id = Some(3);
        unique.email = Some("other@example.com".to_string());

        let manager =
            MultiTokenManager::new(config, vec![first, duplicate, unique], None, false).unwrap();

        manager.deduplicate_existing_credentials_by_email();

        let ids = manager
            .snapshot()
            .entries
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn test_deduplicate_existing_credentials_by_email_prefers_current_account() {
        let config = Config::default();
        let mut first = KiroCredentials::default();
        first.id = Some(1);
        first.email = Some("user@example.com".to_string());
        first.priority = 0;
        let mut current = KiroCredentials::default();
        current.id = Some(2);
        current.email = Some("user@example.com".to_string());
        current.priority = 1;

        let manager = MultiTokenManager::new(config, vec![first, current], None, false).unwrap();
        *manager.current_id.lock() = 2;

        manager.deduplicate_existing_credentials_by_email();

        let snapshot = manager.snapshot();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].id, 2);
        assert_eq!(snapshot.current_id, 2);
    }

    #[tokio::test]
    async fn test_refresh_token_rejects_api_key_credential() {
        let config = Config::default();
        let mut credentials = KiroCredentials::default();
        credentials.kiro_api_key = Some("ksk_test_key_123".to_string());
        credentials.auth_method = Some("api_key".to_string());

        let result = refresh_token(&credentials, &config, None).await;

        assert!(result.is_err(), "API Key 凭据应被 refresh_token 拒绝");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("API Key 凭据不支持刷新"),
            "期望错误消息包含 'API Key 凭据不支持刷新'，实际: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_add_credential_reject_duplicate_email() {
        let config = Config::default();

        let mut existing = KiroCredentials::default();
        existing.refresh_token = Some("a".repeat(150));
        existing.email = Some("user@example.com".to_string());

        let manager = MultiTokenManager::new(config, vec![existing], None, false).unwrap();

        let mut duplicate = KiroCredentials::default();
        duplicate.refresh_token = Some("b".repeat(150));
        duplicate.email = Some(" USER@example.com ".to_string());

        let result = manager.add_credential(duplicate).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("邮箱重复"));
    }

    #[tokio::test]
    async fn test_add_credential_api_key_success() {
        let config = Config::default();
        let manager = MultiTokenManager::new(config, vec![], None, false).unwrap();

        let mut api_key_cred = KiroCredentials::default();
        api_key_cred.kiro_api_key = Some("ksk_test_key_123".to_string());
        api_key_cred.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(api_key_cred).await;
        assert!(result.is_ok());
        let id = result.unwrap();
        assert!(id > 0);
        assert_eq!(manager.total_count(), 1);
        assert_eq!(manager.available_count(), 1);
    }

    #[tokio::test]
    async fn test_add_credential_reject_duplicate_api_key_email() {
        let config = Config::default();

        let mut existing = KiroCredentials::default();
        existing.kiro_api_key = Some("ksk_existing_key".to_string());
        existing.auth_method = Some("api_key".to_string());
        existing.email = Some("user@example.com".to_string());

        let manager = MultiTokenManager::new(config, vec![existing], None, false).unwrap();

        let mut duplicate = KiroCredentials::default();
        duplicate.kiro_api_key = Some("ksk_different_key".to_string());
        duplicate.auth_method = Some("api_key".to_string());
        duplicate.email = Some("user@example.com".to_string());

        let result = manager.add_credential(duplicate).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("邮箱重复"));
    }

    #[tokio::test]
    async fn test_add_credential_api_key_empty_rejected() {
        let config = Config::default();
        let manager = MultiTokenManager::new(config, vec![], None, false).unwrap();

        let mut cred = KiroCredentials::default();
        cred.kiro_api_key = Some(String::new());
        cred.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(cred).await;
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("kiroApiKey 为空")
        );
    }

    #[tokio::test]
    async fn test_add_credential_api_key_missing_key_rejected() {
        let config = Config::default();
        let manager = MultiTokenManager::new(config, vec![], None, false).unwrap();

        let mut cred = KiroCredentials::default();
        cred.auth_method = Some("api_key".to_string());
        // kiro_api_key is None

        let result = manager.add_credential(cred).await;
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("缺少 kiroApiKey")
        );
    }

    #[tokio::test]
    async fn test_add_credential_api_key_and_oauth_coexist() {
        let config = Config::default();

        let mut oauth_cred = KiroCredentials::default();
        oauth_cred.refresh_token = Some("a".repeat(150));

        let manager = MultiTokenManager::new(config, vec![oauth_cred], None, false).unwrap();

        let mut api_key_cred = KiroCredentials::default();
        api_key_cred.kiro_api_key = Some("ksk_new_key".to_string());
        api_key_cred.auth_method = Some("api_key".to_string());

        let result = manager.add_credential(api_key_cred).await;
        assert!(result.is_ok());
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 2);
    }

    // MultiTokenManager 测试

    #[test]
    fn test_multi_token_manager_new() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.priority = 0;
        let mut cred2 = KiroCredentials::default();
        cred2.priority = 1;

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 2);
    }

    #[test]
    fn test_multi_token_manager_empty_credentials() {
        let config = Config::default();
        let result = MultiTokenManager::new(config, vec![], None, false);
        // 支持 0 个凭据启动（可通过管理面板添加）
        assert!(result.is_ok());
        let manager = result.unwrap();
        assert_eq!(manager.total_count(), 0);
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_duplicate_ids() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.id = Some(1);
        let mut cred2 = KiroCredentials::default();
        cred2.id = Some(1); // 重复 ID

        let result = MultiTokenManager::new(config, vec![cred1, cred2], None, false);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("重复的凭据 ID"),
            "错误消息应包含 '重复的凭据 ID'，实际: {}",
            err_msg
        );
    }

    #[test]
    fn test_multi_token_manager_api_key_missing_kiro_api_key_auto_disabled() {
        let config = Config::default();

        // auth_method=api_key 但缺少 kiro_api_key → 应被自动禁用
        let mut bad_cred = KiroCredentials::default();
        bad_cred.auth_method = Some("api_key".to_string());
        // kiro_api_key 保持 None

        let mut good_cred = KiroCredentials::default();
        good_cred.refresh_token = Some("valid_token".to_string());

        let manager =
            MultiTokenManager::new(config, vec![bad_cred, good_cred], None, false).unwrap();
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 1); // bad_cred 被禁用，只剩 1 个可用
    }

    #[test]
    fn test_multi_token_manager_api_key_with_kiro_api_key_not_disabled() {
        let config = Config::default();

        // auth_method=api_key 且有 kiro_api_key → 不应被禁用
        let mut cred = KiroCredentials::default();
        cred.auth_method = Some("api_key".to_string());
        cred.kiro_api_key = Some("ksk_test123".to_string());

        let manager = MultiTokenManager::new(config, vec![cred], None, false).unwrap();
        assert_eq!(manager.total_count(), 1);
        assert_eq!(manager.available_count(), 1);
    }

    #[test]
    fn test_multi_token_manager_report_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        // 前两次失败不会禁用（使用 ID 1）
        assert!(manager.report_failure(1));
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 2);

        // 第三次失败会禁用第一个凭据
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 1);

        // 继续失败第二个凭据（使用 ID 2）
        assert!(manager.report_failure(2));
        assert!(manager.report_failure(2));
        assert!(!manager.report_failure(2)); // 所有凭据都禁用了
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_report_success() {
        let config = Config::default();
        let cred = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred], None, false).unwrap();

        // 失败两次（使用 ID 1）
        manager.report_failure(1);
        manager.report_failure(1);

        // 成功后重置计数（使用 ID 1）
        manager.report_success(1);

        // 再失败两次不会禁用
        manager.report_failure(1);
        manager.report_failure(1);
        assert_eq!(manager.available_count(), 1);
    }

    #[test]
    fn test_multi_token_manager_switch_to_next() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.refresh_token = Some("token1".to_string());
        let mut cred2 = KiroCredentials::default();
        cred2.refresh_token = Some("token2".to_string());

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        let initial_id = manager.snapshot().current_id;

        // 切换到下一个
        assert!(manager.switch_to_next());
        assert_ne!(manager.snapshot().current_id, initial_id);
    }

    #[test]
    fn test_delete_enabled_credential_directly() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.refresh_token = Some("token1".to_string());
        let mut cred2 = KiroCredentials::default();
        cred2.refresh_token = Some("token2".to_string());

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        assert!(manager.delete_credential(1).is_ok());
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.entries.iter().all(|entry| entry.id != 1));
        assert_eq!(snapshot.current_id, 2);
    }

    #[test]
    fn test_set_load_balancing_mode_persists_to_config_file() {
        let config_path =
            std::env::temp_dir().join(format!("kiro-load-balancing-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&config_path, r#"{"loadBalancingMode":"priority"}"#).unwrap();

        let config = Config::load(&config_path).unwrap();
        let manager =
            MultiTokenManager::new(config, vec![KiroCredentials::default()], None, false).unwrap();

        manager
            .set_load_balancing_mode("balanced".to_string())
            .unwrap();

        let persisted = Config::load(&config_path).unwrap();
        assert_eq!(persisted.load_balancing_mode, "balanced");
        assert_eq!(manager.get_load_balancing_mode(), "balanced");

        std::fs::remove_file(&config_path).unwrap();
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_auto_recovers_all_disabled() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(1);
        }
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(2);
        }

        assert_eq!(manager.available_count(), 0);

        // 应触发自愈：重置失败计数并重新启用，避免必须重启进程
        let ctx = manager.acquire_context(None).await.unwrap();
        assert!(ctx.token == "t1" || ctx.token == "t2");
        assert_eq!(manager.available_count(), 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_balanced_retries_until_bad_credential_disabled()
     {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut bad_cred = KiroCredentials::default();
        bad_cred.priority = 0;
        bad_cred.refresh_token = Some("bad".to_string());

        let mut good_cred = KiroCredentials::default();
        good_cred.priority = 1;
        good_cred.access_token = Some("good-token".to_string());
        good_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![bad_cred, good_cred], None, false).unwrap();

        let ctx = manager.acquire_context(None).await.unwrap();
        assert_eq!(ctx.id, 2);
        assert_eq!(ctx.token, "good-token");
    }

    #[tokio::test]
    async fn test_balanced_mode_rotates_across_all_ready_credentials() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut credentials = Vec::new();
        for i in 0..4 {
            let mut cred = KiroCredentials::default();
            cred.priority = if i < 2 { 0 } else { 1 };
            cred.access_token = Some(format!("t{}", i + 1));
            cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
            credentials.push(cred);
        }

        let manager = MultiTokenManager::new(config, credentials, None, false).unwrap();
        let mut selected = Vec::new();
        for _ in 0..4 {
            let ctx = manager.acquire_context(None).await.unwrap();
            selected.push(ctx.id);
            drop(ctx);
        }

        assert_eq!(selected, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn test_acquire_context_skips_suspicious_and_uses_ready_credential() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred2.available_models = Some(vec!["claude-opus-4.7".to_string()]);

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        assert!(manager.report_rate_limited(1, RateLimitKind::SuspiciousActivity));

        let ctx = manager
            .acquire_context_with_options(AcquireOptions::new(Some("claude-opus-4.7".to_string())))
            .await
            .unwrap();
        assert_eq!(ctx.id, 2);
        assert_eq!(ctx.token, "t2");
        assert!(!ctx.used_soft_fallback);

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        let second = snapshot.entries.iter().find(|e| e.id == 2).unwrap();
        assert_eq!(first.dispatch_state, DispatchState::Cooldown.to_string());
        assert_eq!(
            first.last_rate_limit_kind.as_deref(),
            Some("suspicious_activity")
        );
        assert!(first.suspicious_isolated);
        assert!(first.isolation_remaining_ms.is_some());
        assert_eq!(first.dispatch_weight, 0.0);
        assert_eq!(second.dispatch_state, DispatchState::Ready.to_string());
        assert_eq!(snapshot.enabled_count, 2);
        assert_eq!(snapshot.schedulable_count, 1);
        assert_eq!(snapshot.available, 1);
    }

    #[tokio::test]
    async fn test_acquire_context_uses_soft_fallback_for_normal_429_cooldown() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred1], None, false).unwrap();

        assert!(manager.report_rate_limited(1, RateLimitKind::Normal429));

        let ctx = manager
            .acquire_context_with_options(AcquireOptions::new(Some(
                "claude-sonnet-4.6".to_string(),
            )))
            .await
            .unwrap();
        assert_eq!(ctx.id, 1);
        assert!(ctx.used_soft_fallback);
        assert_eq!(ctx.dispatch_path.to_string(), "soft_fallback");
    }

    #[tokio::test]
    async fn test_soft_fallback_can_be_disabled() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();
        config.scheduler.soft_fallback_enabled = false;

        let mut cred = KiroCredentials::default();
        cred.access_token = Some("t1".to_string());
        cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred], None, false).unwrap();
        assert!(manager.report_rate_limited(1, RateLimitKind::Normal429));

        let result = manager.acquire_context(None).await;
        assert!(result.is_err());
        let snapshot = manager.snapshot();
        assert!(!snapshot.entries[0].soft_fallback_eligible);
    }

    #[tokio::test]
    async fn test_soft_fallback_does_not_exceed_max_concurrent() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut cred = KiroCredentials::default();
        cred.access_token = Some("t1".to_string());
        cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred.max_concurrent = Some(1);

        let manager = MultiTokenManager::new(config, vec![cred], None, false).unwrap();
        let first = manager.acquire_context(None).await.unwrap();
        assert!(manager.report_rate_limited(1, RateLimitKind::Normal429));

        let result = manager.acquire_context(None).await;
        assert!(result.is_err());
        drop(first);
    }

    #[tokio::test]
    async fn test_health_weighted_balanced_prefers_healthier_credential() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();
        config.scheduler.health_weighted_scheduling_enabled = true;

        let mut risky = KiroCredentials::default();
        risky.access_token = Some("risky".to_string());
        risky.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut healthy = KiroCredentials::default();
        healthy.access_token = Some("healthy".to_string());
        healthy.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![risky, healthy], None, false).unwrap();
        assert!(manager.report_normal_429_short_cooldown(1, 1));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|entry| entry.id == 1).unwrap();
        let second = snapshot.entries.iter().find(|entry| entry.id == 2).unwrap();
        assert!(second.health_score > first.health_score);
        assert_eq!(first.dispatch_weight, first.health_score as f64 / 100.0);

        let ctx = manager.acquire_context(None).await.unwrap();
        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_normal_429_short_cooldown_uses_milliseconds_budget() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred1], None, false).unwrap();

        assert!(manager.report_normal_429_short_cooldown(1, 250));

        let snapshot = manager.snapshot();
        let entry = snapshot.entries.iter().find(|entry| entry.id == 1).unwrap();
        assert_eq!(entry.dispatch_state, DispatchState::Cooldown.to_string());
        assert_eq!(entry.account_status, AccountStatus::RateLimited.to_string());
        assert_eq!(entry.last_rate_limit_kind.as_deref(), Some("normal_429"));
        let remaining_ms = entry.cooldown_remaining_ms.unwrap();
        assert!(
            remaining_ms <= 1_000,
            "普通 429 短冷却应按毫秒生效，实际剩余 {}ms",
            remaining_ms
        );
    }

    #[test]
    fn test_report_banned_disables_account_immediately() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        assert!(manager.report_banned(1));

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|entry| entry.id == 1).unwrap();
        assert!(first.disabled);
        assert_eq!(first.account_status, AccountStatus::Banned.to_string());
        assert_eq!(first.disabled_reason.as_deref(), Some("Banned"));
        assert_eq!(snapshot.enabled_count, 1);
    }

    #[test]
    fn test_detects_temporarily_suspended_user_id_as_banned() {
        let body = r#"{"message":"Your User ID (34f844e8-10a1-70f9-36c0-61b22c9eb657) temporarily is suspended. We've locked your account as a security precaution. To restore access, please contact our support team to verify your identity: https://app.kiro.dev/account/usage?support_form","reason":null}"#;

        assert!(is_account_banned_response(body));
    }

    #[tokio::test]
    async fn test_acquire_context_never_soft_fallbacks_suspicious_cooldown() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        assert!(manager.report_rate_limited(1, RateLimitKind::SuspiciousActivity));
        assert!(manager.report_rate_limited(2, RateLimitKind::SuspiciousActivity));

        let err = manager
            .acquire_context_with_options(AcquireOptions::new(Some(
                "claude-sonnet-4.6".to_string(),
            )))
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("当前没有可直接调度的凭据"),
            "错误应提示当前没有可调度凭据，实际: {}",
            err
        );

        let snapshot = manager.snapshot();
        assert_eq!(snapshot.enabled_count, 2);
        assert_eq!(snapshot.schedulable_count, 0);
        assert_eq!(snapshot.available, 0);
        assert!(
            snapshot
                .entries
                .iter()
                .all(|entry| !entry.soft_fallback_eligible)
        );
    }

    #[tokio::test]
    async fn test_strict_preferred_account_does_not_fallback_to_ready_credential() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        assert!(manager.report_rate_limited(1, RateLimitKind::SuspiciousActivity));

        let mut options = AcquireOptions::new(Some("claude-sonnet-4.6".to_string()));
        options.preferred_account_id = Some(1);
        options.strict_preferred_account = true;

        let err = manager
            .acquire_context_with_options(options)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("当前没有可直接调度的凭据"),
            "严格指定账号失败时不应回退到其他账号，实际: {}",
            err
        );
        assert_eq!(manager.snapshot().schedulable_count, 1);
    }

    #[tokio::test]
    async fn test_scheduler_policy_filters_capacity_and_selection() {
        let mut config = Config::default();
        config.load_balancing_mode = "balanced".to_string();

        let mut stable = KiroCredentials::default();
        stable.access_token = Some("stable-token".to_string());
        stable.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        stable.max_concurrent = Some(2);

        let mut canary = KiroCredentials::default();
        canary.access_token = Some("canary-token".to_string());
        canary.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        canary.max_concurrent = Some(1);
        canary.scheduler_policy = SchedulerPolicy::Canary;

        let manager = MultiTokenManager::new(config, vec![stable, canary], None, false).unwrap();

        assert_eq!(
            manager.schedulable_capacity_for_model(
                Some("claude-sonnet-4.6"),
                Some(SchedulerPolicy::Stable),
            ),
            2
        );
        assert_eq!(
            manager.schedulable_capacity_for_model(
                Some("claude-sonnet-4.6"),
                Some(SchedulerPolicy::Canary),
            ),
            1
        );

        let mut canary_options = AcquireOptions::new(Some("claude-sonnet-4.6".to_string()));
        canary_options.scheduler_policy = Some(SchedulerPolicy::Canary);
        let canary_ctx = manager
            .acquire_context_with_options(canary_options)
            .await
            .unwrap();
        assert_eq!(canary_ctx.id, 2);
        drop(canary_ctx);

        let mut stable_options = AcquireOptions::new(Some("claude-sonnet-4.6".to_string()));
        stable_options.scheduler_policy = Some(SchedulerPolicy::Stable);
        let stable_ctx = manager
            .acquire_context_with_options(stable_options)
            .await
            .unwrap();
        assert_eq!(stable_ctx.id, 1);
    }

    #[test]
    fn test_multi_token_manager_report_refresh_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        assert_eq!(manager.available_count(), 2);
        for _ in 0..(MAX_FAILURES_PER_CREDENTIAL - 1) {
            assert!(manager.report_refresh_failure(1));
        }
        assert_eq!(manager.available_count(), 2);

        assert!(manager.report_refresh_failure(1));
        assert_eq!(manager.available_count(), 1);

        let snapshot = manager.snapshot();
        let first = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        assert!(first.disabled);
        assert_eq!(first.refresh_failure_count, MAX_FAILURES_PER_CREDENTIAL);
        assert_eq!(snapshot.current_id, 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_refresh_failure_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_refresh_failure(1);
            manager.report_refresh_failure(2);
        }
        assert_eq!(manager.available_count(), 0);

        let err = manager
            .acquire_context(None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("当前没有可直接调度的凭据"),
            "错误应提示当前没有可调度凭据，实际: {}",
            err
        );
    }

    #[test]
    fn test_multi_token_manager_report_quota_exhausted() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        assert_eq!(manager.available_count(), 2);
        assert!(manager.report_quota_exhausted(1));
        assert_eq!(manager.available_count(), 1);

        // 再禁用第二个后，无可用凭据
        assert!(!manager.report_quota_exhausted(2));
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_report_quota_exhausted_persists_disabled_state() {
        let credentials_path =
            std::env::temp_dir().join(format!("kiro-credentials-{}.json", uuid::Uuid::new_v4()));
        let credentials = vec![KiroCredentials::default(), KiroCredentials::default()];
        std::fs::write(
            &credentials_path,
            serde_json::to_string_pretty(&credentials).unwrap(),
        )
        .unwrap();

        let config = Config::default();
        let manager =
            MultiTokenManager::new(config, credentials, Some(credentials_path.clone()), true)
                .unwrap();

        assert!(manager.report_quota_exhausted(1));

        let persisted = std::fs::read_to_string(&credentials_path).unwrap();
        let persisted: Vec<KiroCredentials> = serde_json::from_str(&persisted).unwrap();
        assert!(persisted[0].disabled);
        assert_eq!(
            persisted[0].disabled_reason.as_deref(),
            Some("QuotaExceeded")
        );
        assert!(!persisted[1].disabled);

        std::fs::remove_file(&credentials_path).unwrap();
    }

    #[tokio::test]
    async fn test_multi_token_manager_quota_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred1, cred2], None, false).unwrap();

        manager.report_quota_exhausted(1);
        manager.report_quota_exhausted(2);
        assert_eq!(manager.available_count(), 0);

        let err = manager
            .acquire_context(None)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("当前没有可直接调度的凭据"),
            "错误应提示当前没有可调度凭据，实际: {}",
            err
        );
        assert_eq!(manager.available_count(), 0);
    }

    // ============ 凭据级 Region 优先级测试 ============

    #[test]
    fn test_credential_region_priority_uses_credential_auth_region() {
        // 凭据配置了 auth_region 时，应使用凭据的 auth_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("eu-west-1".to_string());

        let region = credentials.effective_auth_region(&config);
        assert_eq!(region, "eu-west-1");
    }

    #[test]
    fn test_credential_region_priority_fallback_to_credential_region() {
        // 凭据未配置 auth_region 但配置了 region 时，应回退到凭据.region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-central-1".to_string());

        let region = credentials.effective_auth_region(&config);
        assert_eq!(region, "eu-central-1");
    }

    #[test]
    fn test_credential_region_priority_fallback_to_config() {
        // 凭据未配置 auth_region 和 region 时，应回退到 config
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let credentials = KiroCredentials::default();
        assert!(credentials.auth_region.is_none());
        assert!(credentials.region.is_none());

        let region = credentials.effective_auth_region(&config);
        assert_eq!(region, "us-west-2");
    }

    #[test]
    fn test_multiple_credentials_use_respective_regions() {
        // 多凭据场景下，不同凭据使用各自的 auth_region
        let mut config = Config::default();
        config.region = "ap-northeast-1".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.auth_region = Some("us-east-1".to_string());

        let mut cred2 = KiroCredentials::default();
        cred2.region = Some("eu-west-1".to_string());

        let cred3 = KiroCredentials::default(); // 无 region，使用 config

        assert_eq!(cred1.effective_auth_region(&config), "us-east-1");
        assert_eq!(cred2.effective_auth_region(&config), "eu-west-1");
        assert_eq!(cred3.effective_auth_region(&config), "ap-northeast-1");
    }

    #[test]
    fn test_idc_oidc_endpoint_uses_credential_auth_region() {
        // 验证 IdC OIDC endpoint URL 使用凭据 auth_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("eu-central-1".to_string());

        let region = credentials.effective_auth_region(&config);
        let refresh_url = format!("https://oidc.{}.amazonaws.com/token", region);

        assert_eq!(refresh_url, "https://oidc.eu-central-1.amazonaws.com/token");
    }

    #[test]
    fn test_social_refresh_endpoint_uses_credential_auth_region() {
        // 验证 Social refresh endpoint URL 使用凭据 auth_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("ap-southeast-1".to_string());

        let region = credentials.effective_auth_region(&config);
        let refresh_url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", region);

        assert_eq!(
            refresh_url,
            "https://prod.ap-southeast-1.auth.desktop.kiro.dev/refreshToken"
        );
    }

    #[test]
    fn test_api_call_uses_effective_api_region() {
        // 验证 API 调用使用 effective_api_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-west-1".to_string());

        // 凭据.region 不参与 api_region 回退链
        let api_region = credentials.effective_api_region(&config);
        let api_host = format!("q.{}.amazonaws.com", api_region);

        assert_eq!(api_host, "q.us-west-2.amazonaws.com");
    }

    #[test]
    fn test_api_call_uses_credential_api_region() {
        // 凭据配置了 api_region 时，API 调用应使用凭据的 api_region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.api_region = Some("eu-central-1".to_string());

        let api_region = credentials.effective_api_region(&config);
        let api_host = format!("q.{}.amazonaws.com", api_region);

        assert_eq!(api_host, "q.eu-central-1.amazonaws.com");
    }

    #[test]
    fn test_credential_region_empty_string_treated_as_set() {
        // 空字符串 auth_region 被视为已设置（虽然不推荐，但行为应一致）
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("".to_string());

        let region = credentials.effective_auth_region(&config);
        // 空字符串被视为已设置，不会回退到 config
        assert_eq!(region, "");
    }

    #[test]
    fn test_auth_and_api_region_independent() {
        // auth_region 和 api_region 互不影响
        let mut config = Config::default();
        config.region = "default".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_region = Some("auth-only".to_string());
        credentials.api_region = Some("api-only".to_string());

        assert_eq!(credentials.effective_auth_region(&config), "auth-only");
        assert_eq!(credentials.effective_api_region(&config), "api-only");
    }
}
