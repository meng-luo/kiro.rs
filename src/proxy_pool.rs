use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::http_client::ProxyConfig;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolItem {
    name: Option<String>,
    protocol: String,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolData {
    proxies: Vec<ProxyPoolItem>,
}

#[derive(Debug, Clone)]
pub struct ProxyPool {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SelectedProxy {
    pub name: String,
    pub config: ProxyConfig,
}

impl ProxyPool {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path_for_cache_dir(cache_dir: Option<&Path>) -> PathBuf {
        cache_dir
            .unwrap_or_else(|| Path::new("."))
            .join("kiro_proxies.json")
    }

    pub fn random_enabled_proxy(&self) -> Option<ProxyConfig> {
        self.random_enabled_proxy_with_name()
            .map(|proxy| proxy.config)
    }

    pub fn random_enabled_proxy_with_name(&self) -> Option<SelectedProxy> {
        let data = self.load().ok()?;
        let enabled = data
            .proxies
            .into_iter()
            .filter(|proxy| !proxy.disabled)
            .collect::<Vec<_>>();
        if enabled.is_empty() {
            return None;
        }
        enabled
            .get(fastrand::usize(..enabled.len()))
            .map(|proxy| SelectedProxy {
                name: Self::proxy_display_name(proxy),
                config: Self::proxy_config_from_item(proxy),
            })
    }

    fn load(&self) -> anyhow::Result<ProxyPoolData> {
        if !self.path.exists() {
            return Ok(ProxyPoolData::default());
        }
        let content = fs::read_to_string(&self.path)?;
        if content.trim().is_empty() {
            return Ok(ProxyPoolData::default());
        }
        Ok(serde_json::from_str(&content)?)
    }

    fn proxy_config_from_item(proxy: &ProxyPoolItem) -> ProxyConfig {
        let config = ProxyConfig::new(format!(
            "{}://{}:{}",
            proxy.protocol.trim().to_lowercase(),
            proxy.host.trim(),
            proxy.port
        ));
        match (&proxy.username, &proxy.password) {
            (Some(username), Some(password)) if !username.is_empty() && !password.is_empty() => {
                config.with_auth(username.clone(), password.clone())
            }
            _ => config,
        }
    }

    fn proxy_display_name(proxy: &ProxyPoolItem) -> String {
        proxy
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}://{}:{}",
                    proxy.protocol.trim().to_lowercase(),
                    proxy.host.trim(),
                    proxy.port
                )
            })
    }
}
