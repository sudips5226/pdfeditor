//! Quality-first presentation: exact visible keys, CPU completion and GPU residency
//! are separate facts. Only a successful native Present acknowledges a commit.
use crate::layout::DocumentViewport;
use crate::viewport::{TileKey, ViewportError};
use std::collections::HashSet;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct Destination {
    pub viewport: DocumentViewport,
    pub current_page: u32,
}
#[derive(Clone, Debug, Default)]
pub struct PresentationMetrics {
    pub requested_at: u64,
    pub render_started_at: u64,
    pub cpu_ready_at: u64,
    pub gpu_ready_at: u64,
    pub committed_at: u64,
    pub hold_micros: u64,
    pub commit_count: u64,
    pub stale_destinations: u64,
    pub coalesced_requests: u64,
    pub partial_presentations: u64,
}
pub struct Presentation {
    epoch: Instant,
    pub requested: Option<Destination>,
    pub displayed: Option<Destination>,
    pub required: Vec<TileKey>,
    pub cpu: HashSet<TileKey>,
    pub gpu: HashSet<TileKey>,
    pub geometry_ready: bool,
    pub metrics: PresentationMetrics,
    hold_started: u64,
    valid: bool,
}
impl Default for Presentation {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            requested: None,
            displayed: None,
            required: Vec::new(),
            cpu: HashSet::new(),
            gpu: HashSet::new(),
            geometry_ready: false,
            metrics: PresentationMetrics::default(),
            hold_started: 0,
            valid: false,
        }
    }
}
impl Presentation {
    fn now(&self) -> u64 {
        1 + self.epoch.elapsed().as_micros() as u64
    }
    pub fn request(
        &mut self,
        destination: Destination,
        mut required: Vec<TileKey>,
        geometry_ready: bool,
    ) {
        if self.pending() {
            self.metrics.stale_destinations += 1;
            self.metrics.coalesced_requests += 1;
        }
        let now = self.now();
        if self.hold_started == 0 {
            self.hold_started = now;
        }
        self.metrics.requested_at = now;
        self.metrics.render_started_at = 0;
        self.metrics.cpu_ready_at = 0;
        self.metrics.gpu_ready_at = 0;
        self.metrics.committed_at = 0;
        self.requested = Some(destination);
        let mut seen = HashSet::new();
        required.retain(|k| seen.insert(*k));
        self.required = required;
        self.cpu.clear();
        self.gpu.clear();
        self.geometry_ready = geometry_ready;
        self.valid = true;
        self.refresh_times();
    }
    pub fn pending(&self) -> bool {
        self.requested.is_some_and(|r| {
            self.displayed
                .is_none_or(|d| d.viewport.generation != r.viewport.generation)
        })
    }
    pub fn ready(&self) -> bool {
        self.valid
            && self.pending()
            && self.geometry_ready
            && self.required.iter().all(|k| self.gpu.contains(k))
    }
    pub fn cpu_available(&mut self, key: TileKey) {
        if self.required.contains(&key) {
            self.cpu.insert(key);
            self.refresh_times();
        }
    }
    pub fn render_started(&mut self, key: TileKey) {
        if self.required.contains(&key) && self.metrics.render_started_at == 0 {
            self.metrics.render_started_at = self.now();
        }
    }
    /// The native owner reports the complete resident set, not completion counts.
    /// Losing a texture revokes readiness; CPU-only completion never grants it.
    pub fn residency(&mut self, resident: &HashSet<TileKey>) {
        self.gpu = self
            .required
            .iter()
            .filter(|k| resident.contains(k))
            .copied()
            .collect();
        self.cpu.extend(self.gpu.iter().copied());
        if self.gpu.len() != self.required.len() {
            self.metrics.gpu_ready_at = 0;
        }
        self.refresh_times();
    }
    fn refresh_times(&mut self) {
        if !self.valid || !self.geometry_ready {
            return;
        }
        if self.cpu.len() == self.required.len() && self.metrics.cpu_ready_at == 0 {
            self.metrics.cpu_ready_at = self.now();
        }
        if self.gpu.len() == self.required.len() && self.metrics.gpu_ready_at == 0 {
            self.metrics.gpu_ready_at = self.now();
        }
    }
    pub fn commit(&mut self, generation: u64) -> Result<(), ViewportError> {
        if self
            .requested
            .is_none_or(|r| r.viewport.generation != generation)
            || !self.valid
        {
            return Err(ViewportError::StaleGeneration);
        }
        if !self.ready() {
            return Err(ViewportError::InvalidInput);
        }
        self.displayed = self.requested;
        self.metrics.committed_at = self.now();
        self.metrics.hold_micros = self.metrics.committed_at - self.hold_started;
        self.hold_started = 0;
        self.metrics.commit_count += 1;
        Ok(())
    }
    pub fn invalidate(&mut self) {
        // Editing cancels the pending placement, but the last successful frame
        // (including deleted pages) remains valid until its replacement commits.
        self.valid = false;
        self.gpu.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::DocumentPoint;
    use crate::viewport::DeviceSize;
    use crate::{DocumentId, PageId};
    fn key(x: i32, scale: f64) -> TileKey {
        TileKey {
            document_id: DocumentId(1),
            document_revision: 0,
            page_id: PageId(1),
            tile_x: x,
            tile_y: 0,
            physical_scale_bits: scale.to_bits(),
            rotation_degrees: 0,
            width: 512,
            height: 512,
            render_flags: 0,
        }
    }
    fn dest(generation: u64, scale: f64) -> Destination {
        Destination {
            current_page: generation as u32,
            viewport: DocumentViewport {
                origin: DocumentPoint {
                    x: 0.0,
                    y: generation as f64 * 100.0,
                },
                extent: DeviceSize {
                    width: 1024.0,
                    height: 1024.0,
                },
                scale,
                device_pixel_ratio: 1.0,
                page_gap: 24.0,
                generation,
                rotation_degrees: 0,
            },
        }
    }
    fn initial() -> Presentation {
        let mut p = Presentation::default();
        p.request(dest(1, 1.0), vec![key(0, 1.0)], true);
        p.residency(&HashSet::from([key(0, 1.0)]));
        p.commit(1).unwrap();
        p
    }
    #[test]
    fn n_minus_one_cpu_and_missing_gpu_cannot_present_slow_next_page() {
        let mut p = initial();
        let keys: Vec<_> = (0..4).map(|x| key(x, 2.0)).collect();
        p.request(dest(2, 2.0), keys.clone(), true);
        for k in &keys {
            p.cpu_available(*k);
            assert!(!p.ready());
            assert!(p.commit(2).is_err());
        }
        p.residency(&keys[..3].iter().copied().collect());
        assert!(!p.ready());
        assert_eq!(p.displayed.unwrap().viewport.generation, 1);
        p.residency(&keys.iter().copied().collect());
        assert!(p.ready());
        p.commit(2).unwrap();
        assert_eq!(p.displayed.unwrap().viewport.scale, 2.0);
        assert!(p.commit(2).is_err());
        assert_eq!(p.metrics.commit_count, 2);
        assert_eq!(p.metrics.partial_presentations, 0);
    }
    #[test]
    fn latest_target_reversal_and_stale_uploads_cannot_commit() {
        let mut p = initial();
        for g in 2..=4 {
            p.request(dest(g, g as f64), vec![key(0, g as f64)], true);
        }
        p.residency(&HashSet::from([key(0, 2.0), key(0, 3.0)]));
        assert!(p.commit(2).is_err());
        assert!(p.commit(3).is_err());
        assert!(!p.ready());
        assert_eq!(p.displayed.unwrap().viewport.generation, 1);
        p.residency(&HashSet::from([key(0, 4.0)]));
        p.commit(4).unwrap();
        assert_eq!(p.metrics.coalesced_requests, 2);
        assert_eq!(p.metrics.commit_count, 2);
    }
    #[test]
    fn cached_scroll_immediately_ready_and_geometry_never_blank_commits() {
        let mut p = initial();
        p.request(dest(2, 1.0), vec![key(0, 1.0)], true);
        p.residency(&HashSet::from([key(0, 1.0)]));
        assert!(p.ready());
        p.commit(2).unwrap();
        p.request(dest(3, 1.0), vec![], false);
        assert!(!p.ready());
        p.request(dest(4, 1.0), vec![], true);
        assert!(p.ready()); // genuine document gap
    }
    #[test]
    fn loss_of_residency_and_edit_invalidation_revoke_readiness() {
        let mut p = initial();
        p.request(dest(2, 1.0), vec![key(0, 1.0)], true);
        p.residency(&HashSet::from([key(0, 1.0)]));
        assert!(p.ready());
        p.residency(&HashSet::new());
        assert!(!p.ready());
        p.residency(&HashSet::from([key(0, 1.0)]));
        p.invalidate();
        assert!(!p.ready());
        assert!(p.commit(2).is_err());
        assert_eq!(p.displayed.unwrap().viewport.generation, 1);
    }
    #[test]
    fn readiness_requires_every_identity_and_quality_component() {
        let original = key(0, 1.0);
        let variants = [
            TileKey {
                document_id: DocumentId(2),
                ..original
            },
            TileKey {
                document_revision: 1,
                ..original
            },
            TileKey {
                page_id: PageId(2),
                ..original
            },
            TileKey {
                physical_scale_bits: 2f64.to_bits(),
                ..original
            },
            TileKey {
                rotation_degrees: 90,
                ..original
            },
            TileKey {
                tile_x: 1,
                ..original
            },
            TileKey {
                tile_y: 1,
                ..original
            },
            TileKey {
                width: 256,
                ..original
            },
            TileKey {
                height: 256,
                ..original
            },
            TileKey {
                render_flags: 1,
                ..original
            },
        ];
        for k in variants {
            let mut p = initial();
            p.request(dest(2, 1.0), vec![k], true);
            p.residency(&HashSet::from([original]));
            assert!(!p.ready());
            assert!(p.commit(2).is_err());
        }
        let mut p = initial();
        p.request(dest(2, 1.0), vec![original, original], true);
        p.residency(&HashSet::from([original]));
        assert_eq!(p.required.len(), 1);
        p.commit(2).unwrap();
    }
}
