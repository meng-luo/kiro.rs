//! Prompt Cache 展示管理。
//!
//! Kiro 上游不返回 Anthropic prompt cache usage。这里仅在配置 Redis 后维护本地
//! cache_control 命中账本，并把 creation/read usage 返回给调用方。

use std::sync::Arc;

use parking_lot::RwLock;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::types::{CacheControl, Message, MessagesRequest, SystemMessage, Tool};
use crate::token;

const DEFAULT_TTL_SECS: u64 = 5 * 60;
const EXTENDED_TTL_SECS: u64 = 60 * 60;

#[derive(Debug, Clone)]
pub struct CacheBreakpoint {
    hash: String,
    tokens: i32,
    ttl: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheResult {
    pub cache_creation_input_tokens: i32,
    pub cache_read_input_tokens: i32,
    pub uncached_input_tokens: i32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheStatus {
    pub configured: bool,
    pub connected: bool,
    pub redis_url: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone)]
struct RedisState {
    raw_url: String,
    masked_url: String,
    connection: ConnectionManager,
}

#[derive(Default)]
struct PromptCacheInner {
    redis: Option<RedisState>,
    last_error: Option<String>,
}

#[derive(Default)]
pub struct PromptCacheManager {
    inner: RwLock<PromptCacheInner>,
}

impl PromptCacheManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn from_optional_url(redis_url: Option<&str>) -> Arc<Self> {
        let manager = Arc::new(Self::new());
        if let Some(url) = redis_url.and_then(normalize_redis_url) {
            if let Err(error) = manager.apply_redis_url(Some(url)).await {
                tracing::warn!("初始化 Prompt Cache Redis 失败: {}", error);
            }
        }
        manager
    }

    pub fn status(&self) -> PromptCacheStatus {
        let inner = self.inner.read();
        PromptCacheStatus {
            configured: inner.redis.is_some(),
            connected: inner.redis.is_some(),
            redis_url: inner.redis.as_ref().map(|state| state.masked_url.clone()),
            last_error: inner.last_error.clone(),
        }
    }

    pub fn raw_redis_url(&self) -> Option<String> {
        self.inner
            .read()
            .redis
            .as_ref()
            .map(|state| state.raw_url.clone())
    }

    pub async fn apply_redis_url(
        &self,
        redis_url: Option<String>,
    ) -> anyhow::Result<PromptCacheStatus> {
        let Some(redis_url) = redis_url.as_deref().and_then(normalize_redis_url) else {
            let mut inner = self.inner.write();
            inner.redis = None;
            inner.last_error = None;
            return Ok(PromptCacheStatus {
                configured: false,
                connected: false,
                redis_url: None,
                last_error: None,
            });
        };

        match connect_redis(&redis_url).await {
            Ok(connection) => {
                let masked_url = mask_redis_url(&redis_url);
                let mut inner = self.inner.write();
                inner.redis = Some(RedisState {
                    raw_url: redis_url,
                    masked_url: masked_url.clone(),
                    connection,
                });
                inner.last_error = None;
                Ok(PromptCacheStatus {
                    configured: true,
                    connected: true,
                    redis_url: Some(masked_url),
                    last_error: None,
                })
            }
            Err(error) => {
                let message = error.to_string();
                self.inner.write().last_error = Some(message.clone());
                Err(anyhow::anyhow!(message))
            }
        }
    }

    pub async fn lookup_or_create(
        &self,
        api_key: &str,
        request: &MessagesRequest,
        total_input_tokens: i32,
    ) -> CacheResult {
        let redis = self.inner.read().redis.clone();
        let Some(redis) = redis else {
            return CacheResult {
                uncached_input_tokens: total_input_tokens,
                ..Default::default()
            };
        };

        let breakpoints =
            compute_cache_breakpoints(&request.tools, &request.system, &request.messages);
        if breakpoints.is_empty() {
            return CacheResult {
                uncached_input_tokens: total_input_tokens,
                ..Default::default()
            };
        }

        lookup_or_create_with_connection(
            redis.connection,
            api_key,
            &breakpoints,
            total_input_tokens,
        )
        .await
    }
}

async fn connect_redis(redis_url: &str) -> anyhow::Result<ConnectionManager> {
    let client = redis::Client::open(redis_url)?;
    let mut connection = ConnectionManager::new(client).await?;
    let _: String = redis::cmd("PING").query_async(&mut connection).await?;
    Ok(connection)
}

fn normalize_redis_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_ttl(cache_control: &CacheControl) -> u64 {
    match cache_control.ttl.as_deref() {
        Some("1h") => EXTENDED_TTL_SECS,
        _ => DEFAULT_TTL_SECS,
    }
}

fn update_with_json(hasher: &mut Sha256, value: &serde_json::Value) {
    let normalized = normalize_json_value(value.clone());
    let json = serde_json::to_string(&normalized).unwrap_or_default();
    hasher.update(json.as_bytes());
}

fn normalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, normalize_json_value(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(normalize_json_value).collect())
        }
        other => other,
    }
}

pub fn compute_cache_breakpoints(
    tools: &Option<Vec<Tool>>,
    system: &Option<Vec<SystemMessage>>,
    messages: &[Message],
) -> Vec<CacheBreakpoint> {
    let mut hasher = Sha256::new();
    let mut breakpoints = Vec::new();
    let mut cumulative_tokens = 0;

    if let Some(tools) = tools {
        for tool in tools {
            let value = serde_json::to_value(tool).unwrap_or_default();
            update_with_json(&mut hasher, &value);
            cumulative_tokens +=
                token::count_tokens(&serde_json::to_string(&value).unwrap_or_default()) as i32;
            if let Some(cache_control) = &tool.cache_control {
                breakpoints.push(CacheBreakpoint {
                    hash: format!("{:x}", hasher.clone().finalize()),
                    tokens: cumulative_tokens,
                    ttl: parse_ttl(cache_control),
                });
            }
        }
    }

    if let Some(system) = system {
        for message in system {
            hasher.update(message.text.as_bytes());
            cumulative_tokens += token::count_tokens(&message.text) as i32;
            if let Some(cache_control) = &message.cache_control {
                breakpoints.push(CacheBreakpoint {
                    hash: format!("{:x}", hasher.clone().finalize()),
                    tokens: cumulative_tokens,
                    ttl: parse_ttl(cache_control),
                });
            }
        }
    }

    for message in messages {
        let value = serde_json::to_value(message).unwrap_or_default();
        update_with_json(&mut hasher, &value);
        cumulative_tokens += count_message_tokens(message);

        if let Some(cache_control) = message_cache_control(message) {
            breakpoints.push(CacheBreakpoint {
                hash: format!("{:x}", hasher.clone().finalize()),
                tokens: cumulative_tokens,
                ttl: parse_ttl(&cache_control),
            });
        }
    }

    breakpoints
}

fn count_message_tokens(message: &Message) -> i32 {
    let mut total = token::count_tokens(&message.role) as i32;
    match &message.content {
        serde_json::Value::String(text) => total += token::count_tokens(text) as i32,
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                if let Some(text) = block.get("text").and_then(|value| value.as_str()) {
                    total += token::count_tokens(text) as i32;
                } else {
                    total += token::count_tokens(&serde_json::to_string(block).unwrap_or_default())
                        as i32;
                }
            }
        }
        other => {
            total += token::count_tokens(&serde_json::to_string(other).unwrap_or_default()) as i32
        }
    }
    total
}

fn message_cache_control(message: &Message) -> Option<CacheControl> {
    let blocks = message.content.as_array()?;
    blocks
        .iter()
        .filter_map(|block| block.get("cache_control"))
        .filter_map(|value| serde_json::from_value::<CacheControl>(value.clone()).ok())
        .last()
}

async fn lookup_or_create_with_connection(
    mut connection: ConnectionManager,
    api_key: &str,
    breakpoints: &[CacheBreakpoint],
    total_input_tokens: i32,
) -> CacheResult {
    let mut result = CacheResult::default();
    let namespace = hash_api_key(api_key);

    for (index, breakpoint) in breakpoints.iter().enumerate().rev() {
        let key = format!("prompt-cache:{}:{}", namespace, breakpoint.hash);
        let cached: Option<i32> = match connection.get(&key).await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!("读取 Prompt Cache 失败: {}", error);
                return CacheResult {
                    uncached_input_tokens: total_input_tokens,
                    ..Default::default()
                };
            }
        };

        if let Some(cached_tokens) = cached {
            result.cache_read_input_tokens = cached_tokens;
            let _ = connection
                .expire::<_, ()>(&key, breakpoint.ttl as i64)
                .await;

            let mut previous_tokens = cached_tokens;
            for later in breakpoints.iter().skip(index + 1) {
                let later_key = format!("prompt-cache:{}:{}", namespace, later.hash);
                if let Err(error) = connection
                    .set_ex::<_, _, ()>(&later_key, later.tokens, later.ttl)
                    .await
                {
                    tracing::warn!("写入 Prompt Cache 失败: {}", error);
                }
                result.cache_creation_input_tokens += (later.tokens - previous_tokens).max(0);
                previous_tokens = later.tokens;
            }
            break;
        }
    }

    if result.cache_read_input_tokens == 0 {
        let mut previous_tokens = 0;
        for breakpoint in breakpoints {
            let key = format!("prompt-cache:{}:{}", namespace, breakpoint.hash);
            if let Err(error) = connection
                .set_ex::<_, _, ()>(&key, breakpoint.tokens, breakpoint.ttl)
                .await
            {
                tracing::warn!("写入 Prompt Cache 失败: {}", error);
            }
            result.cache_creation_input_tokens += (breakpoint.tokens - previous_tokens).max(0);
            previous_tokens = breakpoint.tokens;
        }
    }

    let cached_tokens = result.cache_read_input_tokens + result.cache_creation_input_tokens;
    result.uncached_input_tokens = (total_input_tokens - cached_tokens).max(0);
    result
}

fn hash_api_key(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn mask_redis_url(redis_url: &str) -> String {
    let Some(scheme_end) = redis_url.find("://") else {
        return redis_url.to_string();
    };
    let prefix_end = scheme_end + 3;
    let after_scheme = &redis_url[prefix_end..];
    let Some(at_index) = after_scheme.find('@') else {
        return redis_url.to_string();
    };
    let credentials = &after_scheme[..at_index];
    if credentials.is_empty() {
        return redis_url.to_string();
    }
    format!(
        "{}***@{}",
        &redis_url[..prefix_end],
        &after_scheme[at_index + 1..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_request_with_cache_control() -> MessagesRequest {
        MessagesRequest {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 100,
            messages: vec![Message {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": "hello",
                        "cache_control": { "type": "ephemeral" }
                    }
                ]),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn no_cache_control_has_no_breakpoints() {
        let mut request = test_request_with_cache_control();
        request.messages[0].content = serde_json::json!("hello");
        assert!(
            compute_cache_breakpoints(&request.tools, &request.system, &request.messages)
                .is_empty()
        );
    }

    #[test]
    fn message_cache_control_creates_breakpoint() {
        let request = test_request_with_cache_control();
        let breakpoints =
            compute_cache_breakpoints(&request.tools, &request.system, &request.messages);
        assert_eq!(breakpoints.len(), 1);
        assert!(breakpoints[0].tokens > 0);
    }

    #[test]
    fn masks_redis_password() {
        assert_eq!(
            mask_redis_url("redis://default:secret@example.com:6379/0"),
            "redis://***@example.com:6379/0"
        );
    }
}
