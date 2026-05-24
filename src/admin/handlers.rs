//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, Query, State},
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use futures::{StreamExt, stream};
use tokio::time::{Duration, interval};

use super::{
    middleware::AdminState,
    types::{
        AddCredentialRequest, AdminSettingsRequest, BatchCredentialUpdateRequest,
        BatchDisabledRequest, BatchIdsRequest, CredentialTestRequest, DiagnosticsQueryRequest,
        PromptCacheConfigRequest, ProxyUpsertRequest, SchedulerConfigRequest, SetDisabledRequest,
        SetLoadBalancingModeRequest, SetMaxConcurrentRequest, SetPriorityRequest, SuccessResponse,
        SystemRollbackRequest, SystemUpdateRequest,
    },
};

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// GET /api/admin/credentials/stream
/// 持续推送账号列表快照
pub async fn stream_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let stream = stream::unfold(
        (state, interval(Duration::from_secs(2)), String::new(), true),
        |(state, mut ticker, mut last_payload, first)| async move {
            if !first {
                ticker.tick().await;
            }

            let payload = match serde_json::to_string(&state.service.get_all_credentials()) {
                Ok(payload) => payload,
                Err(error) => serde_json::json!({
                    "error": {
                        "type": "serialization_error",
                        "message": error.to_string(),
                    }
                })
                .to_string(),
            };

            let should_send = first || payload != last_payload;
            if should_send {
                last_payload = payload.clone();
                Some((
                    Ok::<Event, std::convert::Infallible>(
                        Event::default().event("credentials").data(payload),
                    ),
                    (state, ticker, last_payload, false),
                ))
            } else {
                Some((
                    Ok(Event::default().event("ping").data("{}")),
                    (state, ticker, last_payload, false),
                ))
            }
        },
    );

    Sse::new(stream).into_response()
}

/// GET /api/admin/diagnostics/summary
/// 获取请求诊断聚合数据
pub async fn get_diagnostics_summary(
    State(state): State<AdminState>,
    Query(query): Query<DiagnosticsQueryRequest>,
) -> impl IntoResponse {
    Json(state.service.diagnostics_summary(query))
}

/// GET /api/admin/diagnostics/requests
/// 获取请求诊断明细
pub async fn get_diagnostics_requests(
    State(state): State<AdminState>,
    Query(query): Query<DiagnosticsQueryRequest>,
) -> impl IntoResponse {
    Json(state.service.diagnostics_requests(query))
}

/// GET /api/admin/diagnostics/requests/:request_id
/// 获取单条请求记录详情
pub async fn get_diagnostics_request(
    State(state): State<AdminState>,
    Path(request_id): Path<String>,
) -> impl IntoResponse {
    match state.service.diagnostic_request(&request_id) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/diagnostics/cli
/// 根据当前筛选条件生成可复制的 CLI 命令
pub async fn get_diagnostics_cli(
    State(state): State<AdminState>,
    Query(query): Query<DiagnosticsQueryRequest>,
) -> impl IntoResponse {
    Json(state.service.diagnostics_cli(query))
}

/// GET /api/admin/settings
pub async fn get_admin_settings(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_admin_settings())
}

/// PUT /api/admin/settings
pub async fn set_admin_settings(
    State(state): State<AdminState>,
    Json(payload): Json<AdminSettingsRequest>,
) -> impl IntoResponse {
    match state.service.set_admin_settings(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/scheduler
pub async fn get_scheduler_config(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_scheduler_config())
}

/// PUT /api/admin/config/scheduler
pub async fn set_scheduler_config(
    State(state): State<AdminState>,
    Json(payload): Json<SchedulerConfigRequest>,
) -> impl IntoResponse {
    match state.service.set_scheduler_config(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/disabled
/// 设置凭据禁用状态
pub async fn set_credential_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    match state.service.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("凭据 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/priority
/// 设置凭据优先级
pub async fn set_credential_priority(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetPriorityRequest>,
) -> impl IntoResponse {
    match state.service.set_priority(id, payload.priority) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 优先级已设置为 {}",
            id, payload.priority
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/reset
/// 重置失败计数并重新启用
pub async fn reset_failure_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_and_enable(id) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 失败计数已重置并重新启用",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/recover
/// 手动清理本地运行态阻塞
pub async fn recover_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.recover_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 本地阻塞已清理", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/max-concurrent
/// 设置账号并发上限
pub async fn set_credential_max_concurrent(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetMaxConcurrentRequest>,
) -> impl IntoResponse {
    let max_concurrent = payload.max_concurrent;
    match state.service.set_max_concurrent(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 并发上限已设置为 {}",
            id, max_concurrent
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/balance
/// 获取指定凭据的余额
pub async fn get_credential_balance(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_balance(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials
/// 添加新凭据
pub async fn add_credential(
    State(state): State<AdminState>,
    Json(payload): Json<AddCredentialRequest>,
) -> impl IntoResponse {
    match state.service.add_credential(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除凭据
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/batch/disabled
pub async fn batch_set_credential_disabled(
    State(state): State<AdminState>,
    Json(payload): Json<BatchDisabledRequest>,
) -> impl IntoResponse {
    match state.service.batch_set_disabled(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/batch/reset
pub async fn batch_reset_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_reset(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/batch/refresh
pub async fn batch_refresh_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_refresh(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/batch/balance
pub async fn batch_refresh_balances(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_balance(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/batch/delete
pub async fn batch_delete_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_delete(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PATCH /api/admin/credentials/batch
pub async fn batch_update_credentials(
    State(state): State<AdminState>,
    Json(payload): Json<BatchCredentialUpdateRequest>,
) -> impl IntoResponse {
    match state.service.batch_update_credentials(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/refresh
/// 强制刷新凭据 Token
pub async fn force_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.force_refresh_token(id).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} Token 已强制刷新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/models/refresh
/// 刷新指定凭据的可用模型列表
pub async fn refresh_credential_models(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.refresh_available_models(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/email/refresh
/// 刷新指定凭据邮箱
pub async fn refresh_credential_email(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.refresh_credential_email(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/test
/// 对单个账号发起真实流式测试
pub async fn test_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<CredentialTestRequest>,
) -> impl IntoResponse {
    match state.service.test_credential(id, payload).await {
        Ok(events) => {
            let stream = events.map(|item| match item {
                Ok(payload) => Ok::<Event, std::convert::Infallible>(
                    Event::default().data(payload.to_string()),
                ),
                Err(err) => Ok(Event::default().data(
                    serde_json::json!({
                        "type": "test_complete",
                        "success": false,
                        "message": err.to_string(),
                    })
                    .to_string(),
                )),
            });
            Sse::new(stream).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/load-balancing
/// 获取负载均衡模式
pub async fn get_load_balancing_mode(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_load_balancing_mode();
    Json(response)
}

/// PUT /api/admin/config/load-balancing
/// 设置负载均衡模式
pub async fn set_load_balancing_mode(
    State(state): State<AdminState>,
    Json(payload): Json<SetLoadBalancingModeRequest>,
) -> impl IntoResponse {
    match state.service.set_load_balancing_mode(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/prompt-cache
pub async fn get_prompt_cache_config(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_prompt_cache_config())
}

/// PUT /api/admin/config/prompt-cache
pub async fn set_prompt_cache_config(
    State(state): State<AdminState>,
    Json(payload): Json<PromptCacheConfigRequest>,
) -> impl IntoResponse {
    match state.service.set_prompt_cache_config(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/proxies
pub async fn list_proxies(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.list_proxies() {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxies
pub async fn create_proxy(
    State(state): State<AdminState>,
    Json(payload): Json<ProxyUpsertRequest>,
) -> impl IntoResponse {
    match state.service.create_proxy(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/proxies/:id
pub async fn update_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<ProxyUpsertRequest>,
) -> impl IntoResponse {
    match state.service.update_proxy(id, payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/proxies/:id
pub async fn delete_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_proxy(id) {
        Ok(_) => Json(SuccessResponse::new(format!("代理 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxies/:id/test
pub async fn test_proxy(State(state): State<AdminState>, Path(id): Path<u64>) -> impl IntoResponse {
    match state.service.test_proxy(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxies/batch/test
pub async fn batch_test_proxies(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_test_proxies(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxies/batch/delete
pub async fn batch_delete_proxies(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_delete_proxies(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxies/batch/quality
pub async fn batch_quality_check_proxies(
    State(state): State<AdminState>,
    Json(payload): Json<BatchIdsRequest>,
) -> impl IntoResponse {
    match state.service.batch_quality_check_proxies(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/proxies/:id/accounts
pub async fn get_proxy_accounts(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    Json(state.service.proxy_accounts(id))
}

/// GET /api/admin/system/version
/// 获取系统版本信息
pub async fn get_system_version(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_system_version())
}

/// POST /api/admin/system/version/check
/// 检查系统版本信息
pub async fn check_system_version(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.check_system_version().await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/system/update
pub async fn update_system_version(
    State(state): State<AdminState>,
    Json(payload): Json<SystemUpdateRequest>,
) -> impl IntoResponse {
    match state.service.start_system_update(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/system/rollback
pub async fn rollback_system_version(
    State(state): State<AdminState>,
    Json(payload): Json<SystemRollbackRequest>,
) -> impl IntoResponse {
    match state.service.start_system_rollback(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/system/restart
pub async fn restart_system(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.start_system_restart().await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/system/update/jobs/:id
pub async fn get_system_job(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.service.get_system_job(&id) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}
