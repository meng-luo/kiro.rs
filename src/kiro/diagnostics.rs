use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::model::config::DiagnosticsConfig;

#[derive(Debug, Clone, Default)]
pub struct RequestDiagnosticUpdate {
    pub request_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub original_model: Option<String>,
    pub mapped_model: Option<String>,
    pub credential_id: Option<u64>,
    pub dispatch_path: Option<String>,
    pub sticky_hit: bool,
    pub sticky_detached: bool,
    pub session_hash: Option<String>,
    pub success: bool,
    pub upstream_status: Option<u16>,
    pub upstream_error_code: Option<String>,
    pub upstream_message_short: Option<String>,
    pub rate_limit_kind: Option<String>,
    pub cooldown_ms: Option<u64>,
    pub cooldown_until: Option<String>,
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDiagnosticEntry {
    pub request_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub original_model: Option<String>,
    pub mapped_model: Option<String>,
    pub credential_id: Option<u64>,
    pub dispatch_path: Option<String>,
    pub sticky_hit: bool,
    pub sticky_detached: bool,
    pub session_hash: Option<String>,
    pub success: bool,
    pub upstream_status: Option<u16>,
    pub upstream_error_code: Option<String>,
    pub upstream_message_short: Option<String>,
    pub rate_limit_kind: Option<String>,
    pub cooldown_ms: Option<u64>,
    pub cooldown_until: Option<String>,
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticsQuery {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub credential_id: Option<u64>,
    pub model: Option<String>,
    pub success: Option<bool>,
    pub rate_limit_kind: Option<String>,
    pub dispatch_path: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsRequestsResponse {
    pub items: Vec<RequestDiagnosticEntry>,
    pub next_cursor: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsBucket {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsSummaryResponse {
    pub total_requests: u64,
    pub success_requests: u64,
    pub failed_requests: u64,
    pub rate_limited_requests: u64,
    pub suspicious_requests: u64,
    pub average_duration_ms: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub model_rank: Vec<DiagnosticsBucket>,
    pub credential_rank: Vec<DiagnosticsBucket>,
    pub error_rank: Vec<DiagnosticsBucket>,
}

struct DiagnosticsStoreInner {
    entries: Vec<RequestDiagnosticEntry>,
    next_cursor: usize,
}

pub struct DiagnosticsStore {
    config: DiagnosticsConfig,
    path: Option<PathBuf>,
    inner: Mutex<DiagnosticsStoreInner>,
}

impl DiagnosticsStore {
    pub fn new(config: DiagnosticsConfig, path: Option<PathBuf>) -> Self {
        let entries = if config.enabled && config.persist {
            path.as_ref()
                .map(|p| Self::load_entries(p, &config))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Self {
            config,
            path,
            inner: Mutex::new(DiagnosticsStoreInner {
                next_cursor: entries.len(),
                entries,
            }),
        }
    }

    pub fn record(&self, update: RequestDiagnosticUpdate) {
        if !self.config.enabled {
            return;
        }

        let entry = RequestDiagnosticEntry {
            request_id: update.request_id,
            started_at: update.started_at.to_rfc3339(),
            finished_at: update.finished_at.to_rfc3339(),
            duration_ms: update.duration_ms,
            original_model: update.original_model,
            mapped_model: update.mapped_model,
            credential_id: update.credential_id,
            dispatch_path: update.dispatch_path,
            sticky_hit: update.sticky_hit,
            sticky_detached: update.sticky_detached,
            session_hash: update.session_hash,
            success: update.success,
            upstream_status: update.upstream_status,
            upstream_error_code: update.upstream_error_code,
            upstream_message_short: update.upstream_message_short,
            rate_limit_kind: update.rate_limit_kind,
            cooldown_ms: update.cooldown_ms,
            cooldown_until: update.cooldown_until,
            input_tokens: update.input_tokens,
            output_tokens: update.output_tokens,
        };

        {
            let mut inner = self.inner.lock();
            inner.entries.push(entry.clone());
            inner.next_cursor += 1;
            self.trim_locked(&mut inner);
        }

        if self.config.persist {
            self.append_entry(&entry);
        }
    }

    pub fn query(&self, query: &DiagnosticsQuery) -> DiagnosticsRequestsResponse {
        let inner = self.inner.lock();
        let mut filtered: Vec<RequestDiagnosticEntry> = inner
            .entries
            .iter()
            .filter(|entry| Self::matches(entry, query))
            .cloned()
            .collect();
        filtered.sort_by(|a, b| b.started_at.cmp(&a.started_at));

        let total = filtered.len();
        let start = query.cursor.unwrap_or(0).min(total);
        let limit = query.limit.unwrap_or(100).clamp(1, 500);
        let end = (start + limit).min(total);
        let next_cursor = if end < total { Some(end) } else { None };

        DiagnosticsRequestsResponse {
            items: filtered[start..end].to_vec(),
            next_cursor,
            total,
        }
    }

    pub fn update_tokens(
        &self,
        request_id: &str,
        input_tokens: Option<i32>,
        output_tokens: Option<i32>,
    ) {
        if !self.config.enabled {
            return;
        }

        let mut inner = self.inner.lock();
        let mut updated = None;
        if let Some(entry) = inner
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.request_id == request_id)
        {
            if input_tokens.is_some() {
                entry.input_tokens = input_tokens;
            }
            if output_tokens.is_some() {
                entry.output_tokens = output_tokens;
            }
            updated = Some(entry.clone());
        }
        drop(inner);

        if self.config.persist {
            if let Some(entry) = updated {
                self.append_entry(&entry);
            }
        }
    }

    pub fn summary(&self, query: &DiagnosticsQuery) -> DiagnosticsSummaryResponse {
        let inner = self.inner.lock();
        let entries: Vec<&RequestDiagnosticEntry> = inner
            .entries
            .iter()
            .filter(|entry| Self::matches(entry, query))
            .collect();

        let total_requests = entries.len() as u64;
        let success_requests = entries.iter().filter(|entry| entry.success).count() as u64;
        let failed_requests = total_requests.saturating_sub(success_requests);
        let rate_limited_requests = entries
            .iter()
            .filter(|entry| entry.rate_limit_kind.is_some())
            .count() as u64;
        let suspicious_requests = entries
            .iter()
            .filter(|entry| entry.rate_limit_kind.as_deref() == Some("suspicious_activity"))
            .count() as u64;
        let total_duration: u64 = entries.iter().map(|entry| entry.duration_ms).sum();
        let average_duration_ms = if total_requests > 0 {
            total_duration / total_requests
        } else {
            0
        };
        let input_tokens = entries
            .iter()
            .filter_map(|entry| entry.input_tokens)
            .map(i64::from)
            .sum();
        let output_tokens = entries
            .iter()
            .filter_map(|entry| entry.output_tokens)
            .map(i64::from)
            .sum();

        DiagnosticsSummaryResponse {
            total_requests,
            success_requests,
            failed_requests,
            rate_limited_requests,
            suspicious_requests,
            average_duration_ms,
            input_tokens,
            output_tokens,
            model_rank: Self::rank(
                entries
                    .iter()
                    .filter_map(|entry| entry.original_model.clone()),
            ),
            credential_rank: Self::rank(
                entries
                    .iter()
                    .filter_map(|entry| entry.credential_id.map(|id| format!("#{}", id))),
            ),
            error_rank: Self::rank(entries.iter().filter_map(|entry| {
                entry
                    .rate_limit_kind
                    .clone()
                    .or_else(|| entry.upstream_status.map(|status| status.to_string()))
            })),
        }
    }

    fn load_entries(path: &PathBuf, config: &DiagnosticsConfig) -> Vec<RequestDiagnosticEntry> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };

        let cutoff = Utc::now() - Duration::hours(config.retention_hours.max(1));
        let mut by_request = HashMap::<String, RequestDiagnosticEntry>::new();
        for entry in BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str::<RequestDiagnosticEntry>(&line).ok())
            .filter(|entry| Self::entry_started_at(entry).is_none_or(|started| started >= cutoff))
        {
            by_request.insert(entry.request_id.clone(), entry);
        }

        let mut entries = by_request.into_values().collect::<Vec<_>>();
        entries.sort_by(|a, b| a.started_at.cmp(&b.started_at));

        if entries.len() > config.max_entries {
            entries = entries.split_off(entries.len() - config.max_entries);
        }
        entries
    }

    fn append_entry(&self, entry: &RequestDiagnosticEntry) {
        let Some(path) = &self.path else {
            return;
        };

        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!("创建诊断日志目录失败: {}", error);
                return;
            }
        }

        let payload = match serde_json::to_string(entry) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!("序列化诊断事件失败: {}", error);
                return;
            }
        };

        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{}", payload) {
                    tracing::warn!("写入诊断日志失败: {}", error);
                }
            }
            Err(error) => tracing::warn!("打开诊断日志失败: {}", error),
        }
    }

    fn trim_locked(&self, inner: &mut DiagnosticsStoreInner) {
        let cutoff = Utc::now() - Duration::hours(self.config.retention_hours.max(1));
        inner
            .entries
            .retain(|entry| Self::entry_started_at(entry).is_none_or(|started| started >= cutoff));

        if inner.entries.len() > self.config.max_entries {
            let remove_count = inner.entries.len() - self.config.max_entries;
            inner.entries.drain(0..remove_count);
        }
    }

    fn matches(entry: &RequestDiagnosticEntry, query: &DiagnosticsQuery) -> bool {
        let started_at = Self::entry_started_at(entry);
        if let (Some(since), Some(started)) = (query.since, started_at) {
            if started < since {
                return false;
            }
        }
        if let (Some(until), Some(started)) = (query.until, started_at) {
            if started > until {
                return false;
            }
        }
        if query
            .credential_id
            .is_some_and(|id| entry.credential_id != Some(id))
        {
            return false;
        }
        if let Some(model) = &query.model {
            if entry.original_model.as_deref() != Some(model.as_str()) {
                return false;
            }
        }
        if query
            .success
            .is_some_and(|success| entry.success != success)
        {
            return false;
        }
        if let Some(kind) = &query.rate_limit_kind {
            if entry.rate_limit_kind.as_deref() != Some(kind.as_str()) {
                return false;
            }
        }
        if let Some(path) = &query.dispatch_path {
            if entry.dispatch_path.as_deref() != Some(path.as_str()) {
                return false;
            }
        }
        true
    }

    fn entry_started_at(entry: &RequestDiagnosticEntry) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&entry.started_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    fn rank(values: impl Iterator<Item = String>) -> Vec<DiagnosticsBucket> {
        let mut counts = HashMap::<String, u64>::new();
        for value in values {
            *counts.entry(value).or_default() += 1;
        }
        let mut items = counts
            .into_iter()
            .map(|(key, count)| DiagnosticsBucket { key, count })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        items.truncate(10);
        items
    }
}
