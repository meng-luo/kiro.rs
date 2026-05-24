//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, patch, post, put},
};

use super::{
    handlers::{
        add_credential, batch_delete_credentials, batch_delete_proxies,
        batch_quality_check_proxies, batch_refresh_balances, batch_refresh_credentials,
        batch_reset_credentials, batch_set_credential_disabled, batch_test_proxies,
        batch_update_credentials, check_system_version, create_proxy, delete_credential,
        delete_proxy, force_refresh_token, get_admin_settings, get_all_credentials,
        get_credential_balance, get_diagnostics_cli, get_diagnostics_request,
        get_diagnostics_requests, get_diagnostics_summary, get_load_balancing_mode,
        get_prompt_cache_config, get_proxy_accounts, get_scheduler_config, get_system_job,
        get_system_version, list_proxies, recover_credential, refresh_credential_email,
        refresh_credential_models, reset_failure_count, restart_system, rollback_system_version,
        set_admin_settings, set_credential_disabled, set_credential_max_concurrent,
        set_credential_priority, set_load_balancing_mode, set_prompt_cache_config,
        set_scheduler_config, stream_credentials, test_credential, test_proxy, update_proxy,
        update_system_version,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `DELETE /credentials/:id` - 删除凭据
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `POST /credentials/:id/refresh` - 强制刷新 Token
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /config/load-balancing` - 获取负载均衡模式
/// - `PUT /config/load-balancing` - 设置负载均衡模式
/// - `GET /system/version` - 获取系统版本信息
/// - `POST /system/version/check` - 检查系统版本信息
/// - `POST /system/update` - 发起更新任务
/// - `POST /system/rollback` - 发起回滚任务
/// - `POST /system/restart` - 发起重启任务
/// - `GET /system/update/jobs/:id` - 查询任务状态
///
/// # 认证
/// 需要 Admin API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/stream", get(stream_credentials))
        .route("/credentials/batch", patch(batch_update_credentials))
        .route(
            "/credentials/batch/disabled",
            post(batch_set_credential_disabled),
        )
        .route("/credentials/batch/reset", post(batch_reset_credentials))
        .route(
            "/credentials/batch/refresh",
            post(batch_refresh_credentials),
        )
        .route("/credentials/batch/balance", post(batch_refresh_balances))
        .route("/credentials/batch/delete", post(batch_delete_credentials))
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route(
            "/credentials/{id}/max-concurrent",
            post(set_credential_max_concurrent),
        )
        .route("/credentials/{id}/recover", post(recover_credential))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route(
            "/credentials/{id}/models/refresh",
            post(refresh_credential_models),
        )
        .route(
            "/credentials/{id}/email/refresh",
            post(refresh_credential_email),
        )
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/credentials/{id}/test", post(test_credential))
        .route("/diagnostics/summary", get(get_diagnostics_summary))
        .route("/diagnostics/requests", get(get_diagnostics_requests))
        .route(
            "/diagnostics/requests/{request_id}",
            get(get_diagnostics_request),
        )
        .route("/diagnostics/cli", get(get_diagnostics_cli))
        .route("/settings", get(get_admin_settings).put(set_admin_settings))
        .route("/proxies", get(list_proxies).post(create_proxy))
        .route("/proxies/batch/test", post(batch_test_proxies))
        .route("/proxies/batch/delete", post(batch_delete_proxies))
        .route("/proxies/batch/quality", post(batch_quality_check_proxies))
        .route("/proxies/{id}", put(update_proxy).delete(delete_proxy))
        .route("/proxies/{id}/test", post(test_proxy))
        .route("/proxies/{id}/accounts", get(get_proxy_accounts))
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .route(
            "/config/prompt-cache",
            get(get_prompt_cache_config).put(set_prompt_cache_config),
        )
        .route(
            "/config/scheduler",
            get(get_scheduler_config).put(set_scheduler_config),
        )
        .route("/system/version", get(get_system_version))
        .route("/system/version/check", post(check_system_version))
        .route("/system/update", post(update_system_version))
        .route("/system/rollback", post(rollback_system_version))
        .route("/system/restart", post(restart_system))
        .route("/system/update/jobs/{id}", get(get_system_job))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
