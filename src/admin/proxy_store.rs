use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyItem {
    pub id: u64,
    pub name: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tested_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_test_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyListItem {
    pub id: u64,
    pub name: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub has_password: bool,
    pub disabled: bool,
    pub last_tested_at: Option<String>,
    pub last_test_status: Option<String>,
    pub last_latency_ms: Option<u64>,
    pub last_error: Option<String>,
    pub account_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStoreData {
    pub proxies: Vec<ProxyItem>,
}

#[derive(Debug, Clone)]
pub struct ProxyStore {
    path: PathBuf,
}

impl ProxyStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> anyhow::Result<ProxyStoreData> {
        if !self.path.exists() {
            return Ok(ProxyStoreData::default());
        }
        let content = fs::read_to_string(&self.path)?;
        if content.trim().is_empty() {
            return Ok(ProxyStoreData::default());
        }
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, data: &ProxyStoreData) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(data)?;
        fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn next_id(data: &ProxyStoreData) -> u64 {
        data.proxies.iter().map(|item| item.id).max().unwrap_or(0) + 1
    }
}

impl ProxyItem {
    pub fn now_timestamp() -> String {
        Utc::now().to_rfc3339()
    }

    pub fn url(&self) -> String {
        format!("{}://{}:{}", self.protocol, self.host, self.port)
    }
}
