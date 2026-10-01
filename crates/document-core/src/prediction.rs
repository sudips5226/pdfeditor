//! Deterministic, bounded full-quality viewport prediction. Residency is not presentation.
use crate::layout::DocumentViewport;
use crate::viewport::TileKey;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub const MAX_PREDICTIVE_TILES: usize = 64;
pub const MAX_PREDICTIVE_DEPTH: usize = 4;

#[derive(Clone, Copy, Default, Debug)]
pub struct PredictionMetrics {
    pub velocity: f64,
    pub preparation_us: f64,
    pub lead_distance: f64,
    pub direction: i32,
    pub depth: u32,
    pub input_sequence: u64,
    pub viewport_requests: u64,
    pub keys_generated: u64,
    pub cpu_completed: u64,
    pub gpu_uploaded: u64,
    pub cpu_used: u64,
    pub gpu_used: u64,
    pub cpu_wasted: u64,
    pub gpu_wasted: u64,
    pub invalidated: u64,
    pub entry_required: u32,
    pub entry_cpu: u32,
    pub entry_gpu: u32,
    pub entry_predictive_cpu: u32,
    pub entry_predictive_gpu: u32,
    pub mandatory_rendered: u32,
    pub mandatory_uploaded: u32,
    pub predicted_at: u64,
    pub predicted_cpu_at: u64,
    pub predicted_gpu_at: u64,
    pub requested_at: u64,
}
#[derive(Default)]
struct Record {
    predicted_at: u64,
    cpu_at: u64,
    gpu_at: u64,
    cpu_used: bool,
    gpu_used: bool,
}
pub struct Predictor {
    epoch: Instant,
    pub metrics: PredictionMetrics,
    pub keys: Vec<TileKey>,
    records: HashMap<TileKey, Record>,
    last_input: Option<(f64, u64)>,
    samples: u32,
    quality: Option<(u64, u16, u64, u64)>,
    pub gpu_budget: usize,
    pub gpu_entries: usize,
    pub enabled: bool,
}
impl Default for Predictor {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            metrics: PredictionMetrics {
                preparation_us: 100_000.0,
                ..Default::default()
            },
            keys: Vec::new(),
            records: HashMap::new(),
            last_input: None,
            samples: 0,
            quality: None,
            gpu_budget: 128 * 1024 * 1024,
            gpu_entries: 256,
            enabled: true,
        }
    }
}
impl Predictor {
    pub fn now(&self) -> u64 {
        1 + self.epoch.elapsed().as_micros() as u64
    }
    pub fn invalidate(&mut self) {
        self.metrics.invalidated += self.keys.len() as u64;
        self.keys.clear();
        self.metrics.depth = 0;
        self.metrics.lead_distance = 0.0;
    }
    pub fn direction(&mut self, direction: i32) {
        if self.metrics.direction != direction {
            self.invalidate();
            self.last_input = None;
            self.samples = 0;
            self.metrics.velocity = 0.0;
        }
        self.metrics.direction = direction;
    }
    /// Only actual input calls this; layout/scrollbar corrections never update velocity.
    pub fn input(&mut self, direction: i32, y: f64, now: u64, viewport_height: f64, jump: bool) {
        self.direction(direction);
        self.metrics.input_sequence += 1;
        if direction == 0 || jump {
            self.invalidate();
            self.last_input = Some((y, now));
            self.samples = 0;
            self.metrics.velocity = 0.0;
            return;
        }
        if let Some((previous, at)) = self.last_input {
            let dt = now.saturating_sub(at);
            let displacement = y - previous;
            if dt > 500_000 || displacement.abs() > viewport_height * 3.0 {
                self.invalidate();
                self.samples = 0;
                self.metrics.velocity = 0.0;
            } else if displacement * direction as f64 > 0.0 && dt != 0 {
                let sample = displacement / (dt.max(1_000) as f64 / 1_000_000.0);
                self.metrics.velocity = if self.samples == 0 {
                    sample
                } else {
                    0.35 * sample + 0.65 * self.metrics.velocity
                };
                self.samples = (self.samples + 1).min(100);
            }
        }
        self.last_input = Some((y, now));
    }
    pub fn latency(&mut self, micros: u64) {
        // Cache-only microsecond commits must not erase the estimate for new content.
        let sample = (micros as f64).clamp(10_000.0, 500_000.0);
        self.metrics.preparation_us = 0.2 * sample + 0.8 * self.metrics.preparation_us;
    }
    pub fn plan(
        &mut self,
        v: DocumentViewport,
        mandatory: usize,
        queue: usize,
        cpu_budget: usize,
    ) -> (usize, usize) {
        let quality = (
            v.physical_scale().to_bits(),
            v.rotation_degrees,
            v.extent.width.to_bits(),
            v.extent.height.to_bits(),
        );
        if self.quality.is_some_and(|q| q != quality) {
            self.invalidate();
            self.samples = 0;
            self.last_input = None;
            self.metrics.velocity = 0.0;
        }
        self.quality = Some(quality);
        let tile_bytes = 512 * 512 * 4;
        let normal_capacity = (self.gpu_budget.min(cpu_budget) / tile_bytes).min(self.gpu_entries);
        let capacity = normal_capacity
            .saturating_sub(mandatory)
            .min(MAX_PREDICTIVE_TILES);
        if !self.enabled
            || self.samples == 0
            || self.metrics.direction == 0
            || capacity < mandatory.max(1)
        {
            self.metrics.depth = 0;
            self.metrics.lead_distance = 0.0;
            return (0, 0);
        }
        let height = v.extent.height / v.physical_scale();
        let lead = self.metrics.velocity.abs() * self.metrics.preparation_us / 1_000_000.0 * 1.5;
        let mut depth = ((lead / height + 0.5).ceil() as usize).clamp(1, MAX_PREDICTIVE_DEPTH);
        // A rolling queue has no per-page backlog. Reduce speculation when it cannot drain.
        if queue > 48 {
            depth = depth.min(1);
        } else if queue > 24 {
            depth = depth.min(2);
        }
        depth = depth.min((capacity / mandatory.max(1)).max(1));
        self.metrics.depth = depth as u32;
        self.metrics.lead_distance = height * depth as f64;
        (depth, capacity)
    }
    pub fn set_keys(&mut self, keys: Vec<TileKey>) {
        let mut unique = HashSet::new();
        let keys: Vec<_> = keys
            .into_iter()
            .filter(|key| unique.insert(*key))
            .take(MAX_PREDICTIVE_TILES)
            .collect();
        let now = self.now();
        self.metrics.invalidated += self.keys.iter().filter(|k| !keys.contains(k)).count() as u64;
        self.metrics.viewport_requests += self.metrics.depth as u64;
        for k in &keys {
            if !self.records.contains_key(k) {
                self.metrics.keys_generated += 1;
                self.records.insert(
                    *k,
                    Record {
                        predicted_at: now,
                        ..Default::default()
                    },
                );
            }
        }
        self.keys = keys;
    }
    pub fn cpu_completed(&mut self, key: TileKey) {
        if let Some(r) = self.records.get_mut(&key) {
            if r.cpu_at == 0 {
                r.cpu_at = 1 + self.epoch.elapsed().as_micros() as u64;
                self.metrics.cpu_completed += 1;
            }
        }
    }
    pub fn gpu_uploaded(&mut self, key: TileKey) {
        let mut latency = None;
        if let Some(r) = self.records.get_mut(&key) {
            if r.gpu_at == 0 {
                r.gpu_at = 1 + self.epoch.elapsed().as_micros() as u64;
                self.metrics.gpu_uploaded += 1;
                latency = Some(r.gpu_at.saturating_sub(r.predicted_at));
            }
        }
        if let Some(latency) = latency {
            self.latency(latency);
        }
    }
    pub fn cpu_used(&mut self, key: TileKey) {
        if let Some(r) = self.records.get_mut(&key) {
            if r.cpu_at != 0 && !r.cpu_used {
                r.cpu_used = true;
                self.metrics.cpu_used += 1;
            }
        }
    }
    pub fn entry(&mut self, required: &[TileKey], cpu: &HashSet<TileKey>, gpu: &HashSet<TileKey>) {
        let m = &mut self.metrics;
        m.entry_required = required.len() as u32;
        m.entry_cpu = required
            .iter()
            .filter(|k| cpu.contains(k) || gpu.contains(k))
            .count() as u32;
        m.entry_gpu = required.iter().filter(|k| gpu.contains(k)).count() as u32;
        m.entry_predictive_cpu = 0;
        m.entry_predictive_gpu = 0;
        m.mandatory_rendered = 0;
        m.mandatory_uploaded = 0;
        m.predicted_at = 0;
        m.predicted_cpu_at = 0;
        m.predicted_gpu_at = 0;
        m.requested_at = 1 + self.epoch.elapsed().as_micros() as u64;
        for k in required {
            if let Some(r) = self.records.get_mut(k) {
                if r.cpu_at != 0 && cpu.contains(k) {
                    m.entry_predictive_cpu += 1;
                    if !r.cpu_used {
                        r.cpu_used = true;
                        m.cpu_used += 1;
                    }
                }
                if r.gpu_at != 0 && gpu.contains(k) {
                    m.entry_predictive_gpu += 1;
                    if !r.gpu_used {
                        r.gpu_used = true;
                        m.gpu_used += 1;
                    }
                }
                m.predicted_at = m.predicted_at.max(r.predicted_at);
                m.predicted_cpu_at = m.predicted_cpu_at.max(r.cpu_at);
                m.predicted_gpu_at = m.predicted_gpu_at.max(r.gpu_at);
            }
        }
    }
    /// Keep bookkeeping bounded by the cache + active window, never page count.
    pub fn reconcile(&mut self, cpu: &HashSet<TileKey>, gpu: &HashSet<TileKey>) {
        for (k, r) in &mut self.records {
            if r.cpu_at != 0 && !cpu.contains(k) {
                if !r.cpu_used {
                    self.metrics.cpu_wasted += 1;
                }
                r.cpu_at = 0;
                r.cpu_used = false;
            }
            if r.gpu_at != 0 && !gpu.contains(k) {
                if !r.gpu_used {
                    self.metrics.gpu_wasted += 1;
                }
                r.gpu_at = 0;
                r.gpu_used = false;
            }
        }
        self.records
            .retain(|k, _| self.keys.contains(k) || cpu.contains(k) || gpu.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::DocumentPoint;
    use crate::viewport::DeviceSize;
    fn view() -> DocumentViewport {
        DocumentViewport {
            origin: DocumentPoint { x: 0.0, y: 0.0 },
            extent: DeviceSize {
                width: 1024.0,
                height: 768.0,
            },
            scale: 1.0,
            device_pixel_ratio: 1.0,
            rotation_degrees: 0,
            page_gap: 24.0,
            generation: 1,
        }
    }
    #[test]
    fn speed_latency_pressure_and_quality_bound_prediction() {
        let mut p = Predictor::default();
        let v = view();
        p.input(1, 0.0, 1, 768.0, false);
        p.input(1, 80.0, 100_001, 768.0, false);
        assert_eq!(p.plan(v, 4, 0, 128 << 20).0, 1);
        p.metrics.velocity = 8000.0;
        let medium = p.plan(v, 4, 0, 128 << 20).0;
        assert!(medium >= 2);
        for _ in 0..10 {
            p.latency(500_000);
        }
        assert_eq!(p.plan(v, 4, 0, 128 << 20).0, 4);
        assert_eq!(p.plan(v, 4, 50, 128 << 20).0, 1);
        p.gpu_budget = 6 << 20;
        assert_eq!(p.plan(v, 4, 0, 128 << 20), (0, 0));
        p.gpu_budget = 128 << 20;
        for _ in 0..40 {
            p.latency(10_000);
        }
        assert!(p.plan(v, 4, 0, 128 << 20).0 < medium);
        p.metrics.velocity = 100.0;
        assert_eq!(p.plan(v, 4, 0, 128 << 20).0, 1);
        let mut zoom = v;
        zoom.scale = 2.0;
        assert_eq!(p.plan(zoom, 4, 0, 128 << 20).0, 0);
    }
    #[test]
    fn reversal_jump_and_pause_reset_only_actual_input() {
        let mut p = Predictor::default();
        p.input(1, 0.0, 1, 768.0, false);
        p.input(1, 300.0, 100_001, 768.0, false);
        assert!(p.metrics.velocity > 0.0);
        p.input(-1, 100.0, 200_001, 768.0, false);
        assert_eq!(p.metrics.velocity, 0.0);
        p.input(-1, 0.0, 300_001, 768.0, false);
        assert!(p.metrics.velocity < 0.0);
        p.input(1, 100_000.0, 400_001, 768.0, true);
        assert_eq!(p.plan(view(), 4, 0, 128 << 20).0, 0);
        p.input(1, 100_100.0, 1_400_001, 768.0, false);
        assert_eq!(p.metrics.velocity, 0.0);
    }
}
