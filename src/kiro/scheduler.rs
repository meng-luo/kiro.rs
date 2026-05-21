use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration as StdDuration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

use crate::model::config::{SchedulerConfig, SchedulerModelOverrideConfig};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerModelStateSnapshot {
    /// 调度内部使用的模型 key。
    pub model: String,
    /// 发往 Kiro 上游的模型 ID，用于管理台展示。
    pub upstream_model: String,
    pub window: u32,
    pub inflight: u32,
    pub success_streak: u32,
    pub backoff_remaining_ms: u64,
    pub next_backoff_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerRuntimeSnapshot {
    pub config: SchedulerConfig,
    pub models: Vec<SchedulerModelStateSnapshot>,
}

#[derive(Debug)]
struct ModelRuntimeState {
    window: u32,
    inflight: u32,
    success_streak: u32,
    backoff_until: Option<Instant>,
    next_backoff_ms: u64,
}

impl ModelRuntimeState {
    fn new(config: &EffectiveSchedulerConfig, account_capacity: u32) -> Self {
        Self {
            window: account_capacity.max(config.min_model_concurrency),
            inflight: 0,
            success_streak: 0,
            backoff_until: None,
            next_backoff_ms: config.normal_429_backoff_initial_ms,
        }
    }
}

#[derive(Debug, Clone)]
struct EffectiveSchedulerConfig {
    max_model_concurrency: Option<u32>,
    min_model_concurrency: u32,
    normal_429_backoff_initial_ms: u64,
    normal_429_backoff_max_ms: u64,
    normal_429_backoff_multiplier: f64,
    normal_429_jitter_ratio: f64,
    model_decrease_ratio: f64,
    model_increase_step: u32,
}

pub struct ModelDispatchLease {
    model: String,
    released: bool,
    scheduler: Weak<Scheduler>,
}

impl ModelDispatchLease {
    fn new(model: String, scheduler: &Arc<Scheduler>) -> Self {
        Self {
            model,
            released: false,
            scheduler: Arc::downgrade(scheduler),
        }
    }

    pub fn release(&mut self) {
        if self.released {
            return;
        }
        if let Some(scheduler) = self.scheduler.upgrade() {
            scheduler.release_model_slot(&self.model);
        }
        self.released = true;
    }
}

impl Drop for ModelDispatchLease {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct Scheduler {
    config: Mutex<SchedulerConfig>,
    models: Mutex<HashMap<String, ModelRuntimeState>>,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            models: Mutex::new(HashMap::new()),
        })
    }

    pub fn config(&self) -> SchedulerConfig {
        self.config.lock().clone()
    }

    pub fn update_config(&self, config: SchedulerConfig) {
        *self.config.lock() = config;
    }

    pub fn snapshot(&self) -> SchedulerRuntimeSnapshot {
        let config = self.config();
        let now = Instant::now();
        let mut models: Vec<_> = self
            .models
            .lock()
            .iter()
            .map(|(model, state)| SchedulerModelStateSnapshot {
                model: model.clone(),
                upstream_model: model.clone(),
                window: state.window,
                inflight: state.inflight,
                success_streak: state.success_streak,
                backoff_remaining_ms: state
                    .backoff_until
                    .and_then(|deadline| deadline.checked_duration_since(now))
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0),
                next_backoff_ms: state.next_backoff_ms,
            })
            .collect();
        models.sort_by(|a, b| a.model.cmp(&b.model));
        SchedulerRuntimeSnapshot { config, models }
    }

    pub fn try_acquire_model_slot(
        self: &Arc<Self>,
        model: Option<&str>,
        account_capacity: u32,
    ) -> Result<Option<ModelDispatchLease>, SchedulerWait> {
        let config = self.config();
        if !config.enabled {
            return Ok(None);
        }
        let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
            return Ok(None);
        };

        let effective = Self::effective_config(&config, model);
        let capacity = self.effective_capacity(account_capacity, &effective);
        if capacity == 0 {
            return Err(SchedulerWait::new(100));
        }

        let now = Instant::now();
        let mut models = self.models.lock();
        let state = models
            .entry(model.to_string())
            .or_insert_with(|| ModelRuntimeState::new(&effective, capacity));
        state.window = state
            .window
            .clamp(effective.min_model_concurrency, capacity);

        if let Some(deadline) = state.backoff_until {
            if deadline > now {
                return Err(SchedulerWait::new(
                    deadline.duration_since(now).as_millis() as u64
                ));
            }
            state.backoff_until = None;
        }

        if state.inflight >= state.window || state.inflight >= capacity {
            return Err(SchedulerWait::new(100));
        }

        state.inflight += 1;
        Ok(Some(ModelDispatchLease::new(model.to_string(), self)))
    }

    pub fn report_model_success(&self, model: Option<&str>, account_capacity: u32) {
        let config = self.config();
        if !config.enabled {
            return;
        }
        let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
            return;
        };
        let effective = Self::effective_config(&config, model);
        let capacity = self.effective_capacity(account_capacity, &effective);
        let mut models = self.models.lock();
        let state = models
            .entry(model.to_string())
            .or_insert_with(|| ModelRuntimeState::new(&effective, capacity));
        state.success_streak = state.success_streak.saturating_add(1);
        state.backoff_until = None;
        state.next_backoff_ms = effective.normal_429_backoff_initial_ms;
        if state.window < capacity {
            state.window = (state.window + effective.model_increase_step.max(1)).min(capacity);
        }
    }

    pub fn report_model_capacity_limited(&self, model: Option<&str>, account_capacity: u32) -> u64 {
        let config = self.config();
        if !config.enabled {
            return 0;
        }
        let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
            return 0;
        };
        let effective = Self::effective_config(&config, model);
        let capacity = self.effective_capacity(account_capacity, &effective);
        let mut models = self.models.lock();
        let state = models
            .entry(model.to_string())
            .or_insert_with(|| ModelRuntimeState::new(&effective, capacity.max(1)));

        let next_window = ((state.window as f64) * effective.model_decrease_ratio)
            .floor()
            .max(effective.min_model_concurrency as f64) as u32;
        state.window = next_window.min(capacity.max(effective.min_model_concurrency));
        state.success_streak = 0;

        let jitter = if effective.normal_429_jitter_ratio > 0.0 {
            let span = (state.next_backoff_ms as f64 * effective.normal_429_jitter_ratio) as u64;
            if span > 0 { fastrand::u64(0..=span) } else { 0 }
        } else {
            0
        };
        let wait_ms = (state.next_backoff_ms + jitter).min(effective.normal_429_backoff_max_ms);
        state.backoff_until = Some(Instant::now() + StdDuration::from_millis(wait_ms.max(1)));
        state.next_backoff_ms = ((state.next_backoff_ms as f64)
            * effective.normal_429_backoff_multiplier.max(1.0))
        .round() as u64;
        state.next_backoff_ms = state.next_backoff_ms.clamp(
            effective.normal_429_backoff_initial_ms,
            effective.normal_429_backoff_max_ms,
        );
        wait_ms.max(1)
    }

    fn release_model_slot(&self, model: &str) {
        let mut models = self.models.lock();
        if let Some(state) = models.get_mut(model) {
            state.inflight = state.inflight.saturating_sub(1);
        }
    }

    fn effective_capacity(&self, account_capacity: u32, config: &EffectiveSchedulerConfig) -> u32 {
        if account_capacity == 0 {
            return 0;
        }
        let capped = config
            .max_model_concurrency
            .map(|max| account_capacity.min(max.max(1)))
            .unwrap_or(account_capacity);
        capped.max(config.min_model_concurrency.min(capped.max(1)))
    }

    fn effective_config(config: &SchedulerConfig, model: &str) -> EffectiveSchedulerConfig {
        let override_config = config.model_overrides.get(model);
        EffectiveSchedulerConfig {
            max_model_concurrency: override_config.and_then(|o| o.max_model_concurrency),
            min_model_concurrency: override_config
                .and_then(|o| o.min_model_concurrency)
                .unwrap_or(config.min_model_concurrency)
                .max(1),
            normal_429_backoff_initial_ms: override_config
                .and_then(|o| o.normal_429_backoff_initial_ms)
                .unwrap_or(config.normal_429_backoff_initial_ms)
                .max(1),
            normal_429_backoff_max_ms: override_config
                .and_then(|o| o.normal_429_backoff_max_ms)
                .unwrap_or(config.normal_429_backoff_max_ms)
                .max(1),
            normal_429_backoff_multiplier: config.normal_429_backoff_multiplier,
            normal_429_jitter_ratio: config.normal_429_jitter_ratio.clamp(0.0, 1.0),
            model_decrease_ratio: override_config
                .and_then(|o| o.model_decrease_ratio)
                .unwrap_or(config.model_decrease_ratio)
                .clamp(0.1, 1.0),
            model_increase_step: override_config
                .and_then(|o| o.model_increase_step)
                .unwrap_or(config.model_increase_step)
                .max(1),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulerWait {
    pub wait_ms: u64,
}

impl SchedulerWait {
    fn new(wait_ms: u64) -> Self {
        Self {
            wait_ms: wait_ms.max(1),
        }
    }
}

#[allow(dead_code)]
fn _assert_override_send_sync(_: SchedulerModelOverrideConfig) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_429_reduces_window_and_sets_backoff() {
        let mut config = SchedulerConfig::default();
        config.normal_429_jitter_ratio = 0.0;
        config.normal_429_backoff_initial_ms = 100;
        config.model_decrease_ratio = 0.5;
        let scheduler = Scheduler::new(config);

        let lease = scheduler
            .try_acquire_model_slot(Some("claude-opus-4.7"), 8)
            .unwrap();
        drop(lease);

        let wait_ms = scheduler.report_model_capacity_limited(Some("claude-opus-4.7"), 8);
        let snapshot = scheduler.snapshot();
        let state = snapshot
            .models
            .iter()
            .find(|item| item.model == "claude-opus-4.7")
            .unwrap();

        assert_eq!(wait_ms, 100);
        assert_eq!(state.window, 4);
        assert!(state.backoff_remaining_ms > 0);
    }

    #[test]
    fn success_recovers_window_slowly() {
        let mut config = SchedulerConfig::default();
        config.normal_429_jitter_ratio = 0.0;
        config.model_increase_step = 1;
        config.model_decrease_ratio = 0.5;
        let scheduler = Scheduler::new(config);

        let lease = scheduler
            .try_acquire_model_slot(Some("claude-opus-4.7"), 6)
            .unwrap();
        drop(lease);
        scheduler.report_model_capacity_limited(Some("claude-opus-4.7"), 6);
        scheduler.report_model_success(Some("claude-opus-4.7"), 6);

        let snapshot = scheduler.snapshot();
        let state = snapshot
            .models
            .iter()
            .find(|item| item.model == "claude-opus-4.7")
            .unwrap();

        assert_eq!(state.window, 4);
        assert_eq!(state.success_streak, 1);
        assert_eq!(state.backoff_remaining_ms, 0);
    }

    #[test]
    fn zero_account_capacity_does_not_grant_model_slot() {
        let scheduler = Scheduler::new(SchedulerConfig::default());

        let err = match scheduler.try_acquire_model_slot(Some("claude-opus-4.7"), 0) {
            Ok(_) => panic!("账号可用并发为 0 时不应发放模型槽"),
            Err(err) => err,
        };

        assert_eq!(err.wait_ms, 100);
        assert!(scheduler.snapshot().models.is_empty());
    }
}
