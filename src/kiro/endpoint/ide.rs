//! Kiro IDE 端点
//!
//! 对应 Kiro IDE 客户端目前使用的 AWS CodeWhisperer 端点：
//! - API: `https://q.{api_region}.amazonaws.com/generateAssistantResponse`
//! - MCP: `https://q.{api_region}.amazonaws.com/mcp`
//!
//! 请求头使用 aws-sdk-js User-Agent 标识。请求体会在根对象上注入 `profileArn`。

use reqwest::RequestBuilder;
use uuid::Uuid;

use super::{KiroEndpoint, RequestContext};

/// Kiro IDE 端点名称
pub const IDE_ENDPOINT_NAME: &str = "ide";

/// Kiro IDE 端点
pub struct IdeEndpoint;

impl IdeEndpoint {
    pub fn new() -> Self {
        Self
    }

    fn api_region<'a>(&self, ctx: &'a RequestContext<'_>) -> &'a str {
        ctx.credentials.effective_api_region(ctx.config)
    }

    fn host(&self, ctx: &RequestContext<'_>) -> String {
        format!("q.{}.amazonaws.com", self.api_region(ctx))
    }

    fn x_amz_user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 KiroIDE-{}-{}",
            ctx.config.kiro_version, ctx.machine_id
        )
    }

    fn user_agent(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
            ctx.config.system_version,
            ctx.config.node_version,
            ctx.config.kiro_version,
            ctx.machine_id
        )
    }
}

impl Default for IdeEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl KiroEndpoint for IdeEndpoint {
    fn name(&self) -> &'static str {
        IDE_ENDPOINT_NAME
    }

    fn api_url(&self, ctx: &RequestContext<'_>) -> String {
        format!(
            "https://q.{}.amazonaws.com/generateAssistantResponse",
            self.api_region(ctx)
        )
    }

    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String {
        format!("https://q.{}.amazonaws.com/mcp", self.api_region(ctx))
    }

    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        let mut req = req
            .header("x-amzn-codewhisperer-optout", "true")
            .header("x-amzn-kiro-agent-mode", "vibe")
            .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
            .header("user-agent", self.user_agent(ctx))
            .header("host", self.host(ctx))
            .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=3")
            .header("Authorization", format!("Bearer {}", ctx.token));

        if ctx.credentials.is_api_key_credential() {
            req = req.header("tokentype", "API_KEY");
        }
        req
    }

    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder {
        let mut req = req
            .header("x-amz-user-agent", self.x_amz_user_agent(ctx))
            .header("user-agent", self.user_agent(ctx))
            .header("host", self.host(ctx))
            .header("amz-sdk-invocation-id", Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=3")
            .header("Authorization", format!("Bearer {}", ctx.token));

        if let Some(ref arn) = ctx.credentials.profile_arn {
            req = req.header("x-amzn-kiro-profile-arn", arn);
        }
        if ctx.credentials.is_api_key_credential() {
            req = req.header("tokentype", "API_KEY");
        }
        req
    }

    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> String {
        transform_api_body(body, &ctx.credentials.profile_arn)
    }
}

/// 注入 profileArn，并在最后一跳兜底规范化 Kiro 真实 modelId。
fn transform_api_body(request_body: &str, profile_arn: &Option<String>) -> String {
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(request_body) else {
        return request_body.to_string();
    };

    normalize_model_ids(&mut json);

    if let Some(arn) = profile_arn {
        json["profileArn"] = serde_json::Value::String(arn.clone());
    }

    serde_json::to_string(&json).unwrap_or_else(|_| request_body.to_string())
}

fn normalize_model_ids(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(model_id) = map
                .get("modelId")
                .and_then(|value| value.as_str())
                .map(str::to_string)
            {
                if let Some(normalized) = normalize_kiro_model_id(&model_id) {
                    map.insert(
                        "modelId".to_string(),
                        serde_json::Value::String(normalized.to_string()),
                    );
                }
            }

            for child in map.values_mut() {
                normalize_model_ids(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                normalize_model_ids(child);
            }
        }
        _ => {}
    }
}

fn normalize_kiro_model_id(model_id: &str) -> Option<&'static str> {
    let model = model_id.to_ascii_lowercase();
    let model = model
        .strip_suffix("-thinking")
        .or_else(|| model.strip_suffix("-think"))
        .unwrap_or(&model);

    if model.contains("sonnet") {
        if model.contains("4-6") || model.contains("4.6") {
            return Some("claude-sonnet-4.6");
        }
        if model.contains("4-5") || model.contains("4.5") {
            return Some("claude-sonnet-4.5");
        }
    } else if model.contains("opus") {
        if model.contains("4-8") || model.contains("4.8") {
            return Some("claude-opus-4.8");
        }
        if model.contains("4-7") || model.contains("4.7") {
            return Some("claude-opus-4.7");
        }
        if model.contains("4-6") || model.contains("4.6") {
            return Some("claude-opus-4.6");
        }
        if model.contains("4-5") || model.contains("4.5") {
            return Some("claude-opus-4.5");
        }
    } else if model.contains("haiku") {
        return Some("claude-haiku-4.5");
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{normalize_kiro_model_id, transform_api_body};
    use serde_json::Value;

    #[test]
    fn test_inject_profile_arn_with_some() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let arn = Some("arn:aws:codewhisperer:us-east-1:123:profile/ABC".to_string());
        let result = transform_api_body(body, &arn);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            json["profileArn"],
            "arn:aws:codewhisperer:us-east-1:123:profile/ABC"
        );
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn test_inject_profile_arn_with_none() {
        let body = r#"{"conversationState":{"conversationId":"c1"}}"#;
        let result = transform_api_body(body, &None);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert!(json.get("profileArn").is_none());
        assert_eq!(json["conversationState"]["conversationId"], "c1");
    }

    #[test]
    fn test_inject_profile_arn_overwrites_existing() {
        let body = r#"{"conversationState":{},"profileArn":"old-arn"}"#;
        let arn = Some("new-arn".to_string());
        let result = transform_api_body(body, &arn);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["profileArn"], "new-arn");
    }

    #[test]
    fn test_inject_profile_arn_invalid_json() {
        let body = "not-valid-json";
        let arn = Some("arn:test".to_string());
        let result = transform_api_body(body, &arn);
        assert_eq!(result, "not-valid-json");
    }

    #[test]
    fn test_normalize_kiro_model_id_accepts_public_aliases() {
        assert_eq!(
            normalize_kiro_model_id("claude-sonnet-4-6"),
            Some("claude-sonnet-4.6")
        );
        assert_eq!(
            normalize_kiro_model_id("claude-sonnet-4-6-think"),
            Some("claude-sonnet-4.6")
        );
        assert_eq!(
            normalize_kiro_model_id("claude-sonnet-4-6-thinking"),
            Some("claude-sonnet-4.6")
        );
        assert_eq!(
            normalize_kiro_model_id("claude-opus-4-8-thinking"),
            Some("claude-opus-4.8")
        );
    }

    #[test]
    fn test_transform_api_body_normalizes_all_model_ids() {
        let body = r#"{
            "conversationState": {
                "currentMessage": {
                    "userInputMessage": {
                        "content": "hello",
                        "modelId": "claude-sonnet-4-6-think"
                    }
                },
                "history": [{
                    "userInputMessage": {
                        "content": "old",
                        "modelId": "claude-sonnet-4-6"
                    }
                }]
            }
        }"#;

        let result = transform_api_body(body, &None);
        let json: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            json["conversationState"]["currentMessage"]["userInputMessage"]["modelId"],
            "claude-sonnet-4.6"
        );
        assert_eq!(
            json["conversationState"]["history"][0]["userInputMessage"]["modelId"],
            "claude-sonnet-4.6"
        );
    }
}
