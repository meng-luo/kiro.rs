//! 可用模型查询数据模型
//!
//! 对应 CodeWhisperer Runtime 的 ListAvailableModels API。

use serde::Deserialize;

/// 可用模型查询响应
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModelsResponse {
    /// 当前账号可用模型列表
    #[serde(default)]
    pub models: Vec<AvailableModel>,

    /// 默认模型
    #[serde(default)]
    pub default_model: Option<AvailableModel>,

    /// 分页 token
    #[serde(default)]
    pub next_token: Option<String>,
}

/// 可用模型条目
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModel {
    /// Kiro IDE 当前返回的是 id/name 结构；保留 modelId 兼容服务端字段命名变化。
    #[serde(default)]
    pub id: Option<String>,

    #[serde(default)]
    pub model_id: Option<String>,
}

impl AvailableModel {
    pub fn model_identifier(&self) -> Option<&str> {
        self.id.as_deref().or(self.model_id.as_deref())
    }
}
