//! One render worker: main visible, main prefetch, thumbnail visible, thumbnail prefetch.
use crate::presentation::{Destination, Presentation};
use crate::thumbnails::{is_thumbnail, DEFAULT_THUMBNAIL_BUDGET, MAX_THUMBNAIL_SLOTS};
use crate::viewport::{Priority, TileDemand, TileKey, ViewportError};
use crate::TileBuffer;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

pub const DEFAULT_CPU_BUDGET: usize = 128 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub struct SchedulerConfig {
    pub cpu_bytes: usize,
    pub queue_capacity: usize,
    pub completion_capacity: usize,
}
impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            cpu_bytes: DEFAULT_CPU_BUDGET,
            queue_capacity: 256,
            completion_capacity: 16,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub tile_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub renders_performed: u64,
    pub stale_renders_discarded: u64,
    pub render_errors: u64,
    pub cpu_cache_bytes: usize,
    pub queue_depth: usize,
    pub completion_depth: usize,
}
struct CacheEntry {
    tile: Arc<TileBuffer>,
    touched: u64,
}
pub struct CpuTileCache {
    entries: HashMap<TileKey, CacheEntry>,
    pinned: HashSet<TileKey>,
    budget: usize,
    bytes: usize,
    clock: u64,
}
impl CpuTileCache {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            pinned: HashSet::new(),
            budget,
            bytes: 0,
            clock: 0,
        }
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn pin(&mut self, keys: impl Iterator<Item = TileKey>) {
        self.pinned = keys.collect();
    }
    pub fn get(&mut self, key: &TileKey) -> Option<Arc<TileBuffer>> {
        let e = self.entries.get_mut(key)?;
        self.clock += 1;
        e.touched = self.clock;
        Some(Arc::clone(&e.tile))
    }
    /// Never exceed the hard budget, even if every resident tile is pinned.
    /// The completion can still hold a transient lease if admission fails.
    pub fn insert(&mut self, key: TileKey, tile: Arc<TileBuffer>) -> bool {
        let size = tile.pixels.len();
        if size > self.budget {
            return false;
        }
        if self.entries.contains_key(&key) {
            return true;
        }
        while self.bytes > self.budget - size {
            let victim = self
                .entries
                .iter()
                .filter(|(k, _)| !self.pinned.contains(k))
                .min_by_key(|(_, e)| e.touched)
                .map(|(k, _)| *k);
            let Some(victim) = victim else {
                return false;
            };
            self.bytes -= self.entries.remove(&victim).unwrap().tile.pixels.len();
        }
        self.clock += 1;
        self.entries.insert(
            key,
            CacheEntry {
                tile,
                touched: self.clock,
            },
        );
        self.bytes += size;
        true
    }
}

#[derive(Debug)]
pub struct ReadyTile {
    pub key: TileKey,
    pub generation: u64,
    pub result: Result<Arc<TileBuffer>, i32>,
}

struct Lane {
    config: SchedulerConfig,
    cache: CpuTileCache,
    demand: Vec<TileDemand>,
    queue: VecDeque<TileKey>,
    ready: VecDeque<ReadyTile>,
    done: HashSet<TileKey>,
    gpu_resident: HashSet<TileKey>,
    generation: u64,
    metrics: Metrics,
}
impl Lane {
    fn new(config: SchedulerConfig) -> Self {
        Self {
            config,
            cache: CpuTileCache::new(config.cpu_bytes),
            demand: Vec::new(),
            queue: VecDeque::new(),
            ready: VecDeque::new(),
            done: HashSet::new(),
            gpu_resident: HashSet::new(),
            generation: 0,
            metrics: Metrics::default(),
        }
    }
    fn refill(&mut self, inflight: Option<TileKey>) {
        self.queue.clear();
        let reserved = usize::from(inflight.is_some_and(|k| {
            self.demand
                .iter()
                .any(|d| d.key == k && d.priority == Priority::Visible)
        }));
        let limit = self.config.completion_capacity - reserved;
        for d in &self.demand {
            if self.gpu_resident.contains(&d.key) {
                self.done.insert(d.key);
                continue;
            }
            if self.done.contains(&d.key) || inflight == Some(d.key) {
                continue;
            }
            if let Some(tile) = self.cache.get(&d.key) {
                if d.priority == Priority::Visible {
                    if self.ready.len() >= limit {
                        continue;
                    }
                    self.ready.push_back(ReadyTile {
                        key: d.key,
                        generation: self.generation,
                        result: Ok(tile),
                    });
                }
                self.done.insert(d.key);
            } else {
                self.queue.push_back(d.key);
            }
        }
    }
    fn update(
        &mut self,
        generation: u64,
        mut demand: Vec<TileDemand>,
        inflight: Option<TileKey>,
    ) -> Result<(), ViewportError> {
        if generation == 0 || generation <= self.generation {
            return Err(ViewportError::StaleGeneration);
        }
        demand.sort_by_key(|d| d.priority);
        let mut seen = HashSet::new();
        demand.retain(|d| seen.insert(d.key));
        if demand.len() > self.config.queue_capacity {
            return Err(ViewportError::Capacity);
        }
        self.generation = generation;
        self.cache.pin(
            demand
                .iter()
                .filter(|d| d.priority == Priority::Visible)
                .map(|d| d.key),
        );
        self.metrics.tile_requests += demand.len() as u64;
        for d in &demand {
            if self.cache.entries.contains_key(&d.key) {
                self.metrics.cache_hits += 1;
            } else {
                self.metrics.cache_misses += 1;
            }
        }
        self.demand = demand;
        self.done.clear();
        self.ready.clear();
        self.refill(inflight);
        Ok(())
    }
    fn metrics(&self) -> Metrics {
        Metrics {
            cpu_cache_bytes: self.cache.bytes(),
            queue_depth: self.queue.len(),
            completion_depth: self.ready.len(),
            ..self.metrics
        }
    }
}
struct State {
    main: Lane,
    thumbnails: Lane,
    inflight: Option<TileKey>,
    stop: bool,
    presentation: Presentation,
    direction: i32,
    atomic_enabled: bool,
}
impl State {
    fn refill(&mut self) {
        self.main.refill(self.inflight);
        self.thumbnails.refill(self.inflight);
        for d in &self.main.demand {
            if self.main.cache.entries.contains_key(&d.key) {
                self.presentation.cpu_available(d.key);
            }
        }
        self.presentation.residency(&self.main.gpu_resident);
    }
    fn next(&self) -> Option<TileKey> {
        if self.atomic_enabled
            && self.presentation.pending()
            && !self.main.queue.front().is_some_and(|k| {
                self.main
                    .demand
                    .iter()
                    .any(|d| d.key == *k && d.priority == Priority::Visible)
            })
        {
            return None; // Upload/commit of newest destination precedes speculation and thumbnails.
        }
        // Main backpressure also pauses thumbnails, so polling cannot cause priority inversion.
        if !self.main.queue.is_empty() {
            return (self.main.ready.len() < self.main.config.completion_capacity)
                .then(|| self.main.queue[0]);
        }
        if self.thumbnails.ready.len() < self.thumbnails.config.completion_capacity {
            self.thumbnails.queue.front().copied()
        } else {
            None
        }
    }
    fn lane(&mut self, key: TileKey) -> &mut Lane {
        if is_thumbnail(key) {
            &mut self.thumbnails
        } else {
            &mut self.main
        }
    }
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}
pub struct RenderScheduler {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl RenderScheduler {
    pub fn new(
        config: SchedulerConfig,
        render: impl Fn(TileKey) -> Result<TileBuffer, i32> + Send + 'static,
    ) -> Result<Self, ViewportError> {
        if config.cpu_bytes == 0
            || config.queue_capacity == 0
            || config.queue_capacity > 4096
            || config.completion_capacity == 0
            || config.completion_capacity > 256
        {
            return Err(ViewportError::InvalidInput);
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                main: Lane::new(config),
                thumbnails: Lane::new(SchedulerConfig {
                    cpu_bytes: DEFAULT_THUMBNAIL_BUDGET,
                    queue_capacity: MAX_THUMBNAIL_SLOTS,
                    completion_capacity: 16,
                }),
                inflight: None,
                stop: false,
                presentation: Presentation::default(),
                direction: 0,
                atomic_enabled: false,
            }),
            wake: Condvar::new(),
        });
        let work = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("document-render".into())
            .spawn(move || loop {
                let mut s = work.lock();
                while !s.stop && s.next().is_none() {
                    s = work.wake.wait(s).unwrap_or_else(|p| p.into_inner());
                }
                if s.stop {
                    break;
                }
                let key = s.next().unwrap();
                let lane = s.lane(key);
                lane.queue.pop_front();
                lane.metrics.renders_performed += 1;
                s.inflight = Some(key);
                s.presentation.render_started(key);
                drop(s);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(key)))
                    .unwrap_or(Err(2))
                    .map(Arc::new);
                let mut s = work.lock();
                s.inflight = None;
                if s.stop {
                    break;
                }
                if result.is_ok() {
                    s.presentation.cpu_available(key);
                }
                let lane = s.lane(key);
                let current = lane.demand.iter().find(|d| d.key == key).copied();
                // Useful old thumbnail rasters may stay cached, but never enter recycled slots.
                if is_thumbnail(key) || current.is_some() {
                    if let Ok(tile) = &result {
                        lane.cache.insert(key, Arc::clone(tile));
                    }
                }
                if let Some(item) = current {
                    if result.is_err() {
                        lane.metrics.render_errors += 1;
                    }
                    lane.done.insert(key);
                    if item.priority == Priority::Visible {
                        lane.ready.push_back(ReadyTile {
                            key,
                            generation: lane.generation,
                            result,
                        });
                    }
                } else {
                    lane.metrics.stale_renders_discarded += 1;
                }
                s.refill();
                work.wake.notify_all();
            })
            .map_err(|_| ViewportError::InvalidInput)?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    /// Cancel placement demand and publications, retaining bounded pixel caches.
    /// The next client generation must still be greater than the last submission.
    pub fn invalidate_placement(&self) {
        let mut s = self.shared.lock();
        s.presentation.invalidate();
        {
            let lane = &mut s.main;
            lane.demand.clear();
            lane.queue.clear();
            lane.ready.clear();
            lane.done.clear();
            lane.cache.pin(std::iter::empty());
        }
        let lane = &mut s.thumbnails;
        lane.demand.clear();
        lane.queue.clear();
        lane.ready.clear();
        lane.done.clear();
        lane.cache.pin(std::iter::empty());
        self.shared.wake.notify_all();
    }
    pub fn capacity(&self) -> usize {
        self.shared.lock().main.config.queue_capacity
    }
    pub fn update(&self, generation: u64, demand: Vec<TileDemand>) -> Result<(), ViewportError> {
        if demand.iter().any(|d| is_thumbnail(d.key)) {
            return Err(ViewportError::InvalidInput);
        }
        let mut s = self.shared.lock();
        let inflight = s.inflight;
        s.main.update(generation, demand, inflight)?;
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn update_presentation(
        &self,
        destination: Destination,
        demand: Vec<TileDemand>,
        geometry_ready: bool,
    ) -> Result<(), ViewportError> {
        let mut s = self.shared.lock();
        let inflight = s.inflight;
        let required = demand
            .iter()
            .filter(|d| d.priority == Priority::Visible)
            .map(|d| d.key)
            .collect();
        s.main
            .update(destination.viewport.generation, demand, inflight)?;
        s.presentation
            .request(destination, required, geometry_ready);
        if let Some(key) = inflight {
            s.presentation.render_started(key);
        }
        s.refill();
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn set_gpu_residency(&self, keys: HashSet<TileKey>) -> Result<(), ViewportError> {
        if keys.len() > 512 || keys.iter().any(|k| is_thumbnail(*k)) {
            return Err(ViewportError::Capacity);
        }
        let mut s = self.shared.lock();
        // Revoked GPU keys must become eligible for CPU delivery/render again.
        s.atomic_enabled = true;
        let revoked: Vec<_> = s.main.gpu_resident.difference(&keys).copied().collect();
        for k in revoked {
            s.main.done.remove(&k);
        }
        s.main.gpu_resident = keys;
        s.refill();
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn presentation<T>(&self, read: impl FnOnce(&Presentation) -> T) -> T {
        read(&self.shared.lock().presentation)
    }
    pub fn commit_presentation(&self, generation: u64) -> Result<(), ViewportError> {
        self.shared.lock().presentation.commit(generation)?;
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn set_direction(&self, direction: i32) -> Result<(), ViewportError> {
        if !(-1..=1).contains(&direction) {
            return Err(ViewportError::InvalidInput);
        }
        self.shared.lock().direction = direction;
        Ok(())
    }
    pub fn direction(&self) -> i32 {
        self.shared.lock().direction
    }
    pub fn update_thumbnails(
        &self,
        generation: u64,
        demand: Vec<TileDemand>,
    ) -> Result<(), ViewportError> {
        if demand.iter().any(|d| !is_thumbnail(d.key)) {
            return Err(ViewportError::InvalidInput);
        }
        let mut s = self.shared.lock();
        let inflight = s.inflight;
        s.thumbnails.update(generation, demand, inflight)?;
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn configure_thumbnails(&self, budget: usize) -> Result<(), ViewportError> {
        let mut s = self.shared.lock();
        if budget == 0 || s.thumbnails.generation != 0 {
            return Err(ViewportError::InvalidInput);
        }
        s.thumbnails.cache = CpuTileCache::new(budget);
        s.thumbnails.config.cpu_bytes = budget;
        Ok(())
    }
    fn poll_lane(&self, thumbnail: bool) -> Option<ReadyTile> {
        let mut s = self.shared.lock();
        let out = if thumbnail {
            s.thumbnails.ready.pop_front()
        } else {
            s.main.ready.pop_front()
        };
        s.refill();
        self.shared.wake.notify_all();
        out
    }
    pub fn poll(&self) -> Option<ReadyTile> {
        self.poll_lane(false)
    }
    pub fn poll_thumbnail(&self) -> Option<ReadyTile> {
        self.poll_lane(true)
    }
    pub fn metrics(&self) -> Metrics {
        self.shared.lock().main.metrics()
    }
    pub fn thumbnail_metrics(&self) -> Metrics {
        self.shared.lock().thumbnails.metrics()
    }
}
impl Drop for RenderScheduler {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentId, PageId};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    fn key(x: i32) -> TileKey {
        TileKey {
            document_id: DocumentId(1),
            document_revision: 0,
            page_id: PageId(1),
            tile_x: x,
            tile_y: 0,
            physical_scale_bits: 1f64.to_bits(),
            rotation_degrees: 0,
            width: 512,
            height: 512,
            render_flags: 0,
        }
    }
    fn visible(x: i32) -> TileDemand {
        TileDemand {
            key: key(x),
            priority: Priority::Visible,
        }
    }
    fn tile() -> TileBuffer {
        TileBuffer::new_bgra(512, 512)
    }
    fn drain(s: &RenderScheduler, count: usize) -> Vec<ReadyTile> {
        let end = Instant::now() + Duration::from_secs(5);
        let mut out = Vec::new();
        while out.len() < count {
            if let Some(r) = s.poll() {
                out.push(r);
            } else {
                assert!(Instant::now() < end, "worker timed out");
                thread::yield_now();
            }
        }
        out
    }
    fn thumb(x: i32, priority: Priority) -> TileDemand {
        TileDemand {
            key: crate::thumbnails::ThumbnailKey {
                document_id: DocumentId(1),
                revision: 0,
                page_id: PageId(x as u64),
                width: 2,
                height: 2,
                dpr_bits: 1f64.to_bits(),
                rotation: 0,
                flags: 0,
            }
            .work_key(),
            priority,
        }
    }
    fn destination(generation: u64) -> Destination {
        Destination {
            current_page: generation as u32,
            viewport: crate::layout::DocumentViewport {
                origin: crate::layout::DocumentPoint {
                    x: 0.0,
                    y: generation as f64 * 100.0,
                },
                extent: crate::viewport::DeviceSize {
                    width: 1024.0,
                    height: 1024.0,
                },
                scale: 1.0,
                device_pixel_ratio: 1.0,
                page_gap: 24.0,
                generation,
                rotation_degrees: 0,
            },
        }
    }
    #[test]
    fn gpu_cached_scroll_has_no_render_or_upload_publication() {
        let s = RenderScheduler::new(Default::default(), |_| {
            panic!("GPU-resident tiles must never render")
        })
        .unwrap();
        s.set_gpu_residency(HashSet::from([key(0), key(1)]))
            .unwrap();
        for g in 1..=2 {
            s.update_presentation(destination(g), vec![visible(0), visible(1)], true)
                .unwrap();
            assert!(s.presentation(|p| p.ready()));
            assert!(s.poll().is_none());
            s.commit_presentation(g).unwrap();
        }
        assert_eq!(s.metrics().renders_performed, 0);
        assert_eq!(s.presentation(|p| p.metrics.commit_count), 2);
    }
    #[test]
    fn gated_page_transition_cpu_completion_waits_for_final_upload_and_commit() {
        let (started, rx) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let s = RenderScheduler::new(Default::default(), move |k| {
            started.send(k).unwrap();
            gate.recv().unwrap();
            Ok(tile())
        })
        .unwrap();
        s.set_gpu_residency(HashSet::from([key(0)])).unwrap();
        s.update_presentation(destination(1), vec![visible(0)], true)
            .unwrap();
        s.commit_presentation(1).unwrap();
        s.update_presentation(destination(2), vec![visible(1), visible(2)], true)
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(s.presentation(|p| p.displayed.unwrap().current_page), 1);
        resume.send(()).unwrap();
        drain(&s, 1);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.set_gpu_residency(HashSet::from([key(0), key(1)]))
            .unwrap();
        assert!(!s.presentation(|p| p.ready()));
        assert!(s.commit_presentation(2).is_err());
        resume.send(()).unwrap();
        drain(&s, 1);
        assert_eq!(s.presentation(|p| p.cpu.len()), 2);
        assert!(!s.presentation(|p| p.ready()));
        s.set_gpu_residency(HashSet::from([key(0), key(1), key(2)]))
            .unwrap();
        assert!(s.presentation(|p| p.ready()));
        s.commit_presentation(2).unwrap();
        assert_eq!(s.presentation(|p| p.metrics.partial_presentations), 0);
    }
    #[test]
    fn destination_upload_precedes_speculation_and_reversal_replaces_queue() {
        let (started, rx) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let s = RenderScheduler::new(Default::default(), move |k| {
            started.send(k).unwrap();
            gate.recv().unwrap();
            Ok(tile())
        })
        .unwrap();
        s.set_gpu_residency(HashSet::new()).unwrap();
        let speculative = |x| TileDemand {
            key: key(x),
            priority: Priority::Directional,
        };
        s.update_presentation(destination(1), vec![visible(1), speculative(2)], true)
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), key(1));
        resume.send(()).unwrap();
        drain(&s, 1);
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err()); // GPU upload has priority
        s.set_direction(-1).unwrap();
        s.update_presentation(destination(2), vec![visible(3), speculative(4)], true)
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), key(3));
        resume.send(()).unwrap();
        drain(&s, 1);
        assert!(s.commit_presentation(1).is_err());
        s.set_gpu_residency(HashSet::from([key(3), key(4)]))
            .unwrap();
        s.commit_presentation(2).unwrap();
        assert_eq!(s.metrics().renders_performed, 2); // stale forward speculation never ran
    }
    fn drain_thumbnails(s: &RenderScheduler, count: usize) -> Vec<ReadyTile> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut out = Vec::new();
        while out.len() < count {
            if let Some(r) = s.poll_thumbnail() {
                out.push(r);
            } else {
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
        }
        out
    }
    #[test]
    fn edit_invalidation_cancels_publications_and_retains_pixel_cache() {
        let s = RenderScheduler::new(Default::default(), |_| Ok(tile())).unwrap();
        s.update(1, vec![visible(0)]).unwrap();
        let original = drain(&s, 1).pop().unwrap().result.unwrap();
        s.update(2, vec![visible(0)]).unwrap(); // cached ready publication
        s.invalidate_placement();
        assert!(s.poll().is_none());
        assert!(s.poll_thumbnail().is_none());
        assert_eq!(
            s.update(2, vec![visible(0)]),
            Err(ViewportError::StaleGeneration)
        );
        s.update(3, vec![visible(0)]).unwrap();
        let moved = drain(&s, 1).pop().unwrap();
        assert_eq!(moved.generation, 3);
        assert!(Arc::ptr_eq(&original, &moved.result.unwrap()));
        assert_eq!(s.metrics().renders_performed, 1);
        s.invalidate_placement();
        let mut rotated = visible(0);
        rotated.key.rotation_degrees = 90;
        s.update(4, vec![rotated]).unwrap();
        assert_eq!(drain(&s, 1)[0].key.rotation_degrees, 90);
        assert_eq!(s.metrics().renders_performed, 2);
    }
    #[test]
    fn edit_during_running_thumbnail_rejects_old_token_demand() {
        let (started, rx) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let s = RenderScheduler::new(Default::default(), move |_| {
            started.send(()).unwrap();
            gate.recv().unwrap();
            Ok(TileBuffer::new_bgra(2, 2))
        })
        .unwrap();
        s.update_thumbnails(1, vec![thumb(1, Priority::Visible)])
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.invalidate_placement();
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.thumbnail_metrics().stale_renders_discarded == 0 {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(s.poll_thumbnail().is_none());
        s.update_thumbnails(2, vec![thumb(1, Priority::Visible)])
            .unwrap();
        assert_eq!(drain_thumbnails(&s, 1)[0].generation, 2);
        assert_eq!(s.thumbnail_metrics().renders_performed, 1);
    }
    #[test]
    fn main_work_precedes_visible_thumbnails_and_thumbnail_prefetch() {
        let (started, rx) = mpsc::channel();
        let (resume, gate) = mpsc::channel();
        let (tx, order) = mpsc::channel();
        let s = RenderScheduler::new(Default::default(), move |k| {
            if k == thumb(1, Priority::Visible).key {
                started.send(()).unwrap();
                gate.recv().unwrap();
            }
            tx.send(k).unwrap();
            Ok(TileBuffer::new_bgra(2, 2))
        })
        .unwrap();
        s.update_thumbnails(1, vec![thumb(1, Priority::Visible)])
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.update_thumbnails(
            2,
            vec![thumb(3, Priority::Prefetch), thumb(2, Priority::Visible)],
        )
        .unwrap();
        s.update(
            1,
            vec![
                TileDemand {
                    key: key(5),
                    priority: Priority::Prefetch,
                },
                visible(4),
            ],
        )
        .unwrap();
        resume.send(()).unwrap();
        let expected = [
            thumb(1, Priority::Visible).key,
            key(4),
            key(5),
            thumb(2, Priority::Visible).key,
            thumb(3, Priority::Prefetch).key,
        ];
        for k in expected {
            assert_eq!(order.recv_timeout(Duration::from_secs(5)).unwrap(), k);
        }
        assert_eq!(
            drain_thumbnails(&s, 1)[0].key,
            thumb(2, Priority::Visible).key
        );
        assert_eq!(s.thumbnail_metrics().stale_renders_discarded, 1);
        // Old running A was cached, never published in B's recycled slot.
        s.update_thumbnails(3, vec![thumb(1, Priority::Visible)])
            .unwrap();
        assert_eq!(drain_thumbnails(&s, 1)[0].generation, 3);
        assert_eq!(s.thumbnail_metrics().renders_performed, 3);
        assert!(s.poll().is_some());
    }
    #[test]
    fn thumbnail_return_reuse_separate_budget_eviction_and_failure() {
        let s = RenderScheduler::new(Default::default(), |k| {
            if k.page_id == PageId(9) {
                Err(4)
            } else {
                Ok(TileBuffer::new_bgra(2, 2))
            }
        })
        .unwrap();
        s.configure_thumbnails(32).unwrap();
        s.update_thumbnails(1, vec![thumb(1, Priority::Visible)])
            .unwrap();
        let first = drain_thumbnails(&s, 1);
        s.update_thumbnails(2, vec![thumb(2, Priority::Visible)])
            .unwrap();
        drain_thumbnails(&s, 1);
        s.update_thumbnails(3, vec![thumb(1, Priority::Visible)])
            .unwrap();
        let returned = drain_thumbnails(&s, 1);
        assert!(Arc::ptr_eq(
            first[0].result.as_ref().unwrap(),
            returned[0].result.as_ref().unwrap()
        ));
        assert_eq!(s.thumbnail_metrics().renders_performed, 2);
        s.update_thumbnails(4, vec![thumb(3, Priority::Visible)])
            .unwrap();
        drain_thumbnails(&s, 1);
        assert_eq!(s.thumbnail_metrics().cpu_cache_bytes, 32);
        assert_eq!(s.metrics().cpu_cache_bytes, 0);
        s.update_thumbnails(5, vec![thumb(2, Priority::Visible)])
            .unwrap();
        drain_thumbnails(&s, 1);
        assert_eq!(s.thumbnail_metrics().renders_performed, 4); // B was LRU
        s.update_thumbnails(6, vec![thumb(9, Priority::Visible)])
            .unwrap();
        assert_eq!(drain_thumbnails(&s, 1)[0].result, Err(4));
        assert_eq!(s.thumbnail_metrics().render_errors, 1);
    }
    #[test]
    fn thumbnail_queue_and_ready_backpressure_are_bounded() {
        let s =
            RenderScheduler::new(Default::default(), |_| Ok(TileBuffer::new_bgra(2, 2))).unwrap();
        assert_eq!(
            s.update_thumbnails(1, (1..=65).map(|i| thumb(i, Priority::Visible)).collect()),
            Err(ViewportError::Capacity)
        );
        for generation in 1..=100 {
            s.update_thumbnails(
                generation,
                (1..=8)
                    .map(|i| thumb(i + generation as i32 * 8, Priority::Visible))
                    .collect(),
            )
            .unwrap();
            assert!(s.thumbnail_metrics().queue_depth <= 8);
            assert!(s.thumbnail_metrics().completion_depth <= 16);
        }
        assert!(drain_thumbnails(&s, 8).iter().all(|r| r.generation == 100));
    }
    #[test]
    fn cross_page_stale_rejection_and_scroll_return_reuse() {
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let page = |id| TileDemand {
            key: TileKey {
                page_id: PageId(id),
                ..key(0)
            },
            priority: Priority::Visible,
        };
        let s = RenderScheduler::new(Default::default(), move |k| {
            if k.page_id == PageId(1) {
                started_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            }
            Ok(tile())
        })
        .unwrap();
        s.update(1, vec![page(1)]).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.update(2, vec![page(2)]).unwrap();
        s.update(3, vec![page(3)]).unwrap();
        resume_tx.send(()).unwrap();
        let third = drain(&s, 1);
        assert_eq!(third[0].key.page_id, PageId(3));
        assert_eq!(third[0].generation, 3);
        assert_eq!(s.metrics().stale_renders_discarded, 1);
        s.update(4, vec![page(2)]).unwrap();
        drain(&s, 1);
        let renders = s.metrics().renders_performed;
        s.update(5, vec![page(3)]).unwrap();
        let returned = drain(&s, 1);
        assert_eq!(s.metrics().renders_performed, renders);
        assert!(Arc::ptr_eq(
            third[0].result.as_ref().unwrap(),
            returned[0].result.as_ref().unwrap()
        ));
    }

    #[test]
    fn actual_bytes_lru_and_required_protection() {
        let mut c = CpuTileCache::new(8);
        let small = || Arc::new(TileBuffer::new_bgra(1, 1));
        assert!(c.insert(key(0), small()));
        assert!(c.insert(key(1), small()));
        c.get(&key(0));
        assert!(c.insert(key(2), small()));
        assert!(c.get(&key(1)).is_none());
        c.pin([key(0), key(2)].into_iter());
        assert!(!c.insert(key(3), small()));
        assert_eq!(c.bytes(), 8);
        c.pin([key(0)].into_iter());
        assert!(c.insert(key(3), small()));
        assert!(c.get(&key(0)).is_some());
        assert!(!c.insert(key(4), Arc::new(tile())));
    }
    #[test]
    fn priorities_coalescing_overlap_hits_and_queue_ownership() {
        let (tx, rx) = mpsc::channel();
        let s = RenderScheduler::new(
            SchedulerConfig {
                completion_capacity: 1,
                ..Default::default()
            },
            move |k| {
                tx.send(k).unwrap();
                Ok(tile())
            },
        )
        .unwrap();
        s.update(
            1,
            vec![
                TileDemand {
                    priority: Priority::Prefetch,
                    key: key(2),
                },
                visible(0),
                visible(0),
                visible(1),
            ],
        )
        .unwrap();
        let r = drain(&s, 2);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), key(0));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), key(1));
        let retained = r[1].result.as_ref().unwrap().clone();
        s.update(2, vec![visible(1)]).unwrap();
        let reused = drain(&s, 1);
        assert!(Arc::ptr_eq(&retained, reused[0].result.as_ref().unwrap()));
        assert_eq!(reused[0].generation, 2);
        assert!(s.metrics().cache_hits >= 1);
        assert!(s.metrics().completion_depth <= 1);
        assert_eq!(s.update(1, vec![]), Err(ViewportError::StaleGeneration));
        drop(s);
        assert_eq!(retained.pixels.len(), 1_048_576);
    }
    #[test]
    fn running_obsolete_result_and_queued_work_are_rejected() {
        let (started_tx, started_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let s = RenderScheduler::new(Default::default(), move |k| {
            if k == key(0) {
                started_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
            }
            Ok(tile())
        })
        .unwrap();
        s.update(1, vec![visible(0), visible(1)]).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.update(2, vec![visible(2)]).unwrap();
        s.update(3, vec![visible(3)]).unwrap();
        continue_tx.send(()).unwrap();
        let r = drain(&s, 1);
        assert_eq!(r[0].key, key(3));
        assert_eq!(r[0].generation, 3);
        assert_eq!(s.metrics().stale_renders_discarded, 1);
        assert_eq!(s.metrics().renders_performed, 2);
    }
    #[test]
    fn demand_and_completion_are_bounded() {
        let config = SchedulerConfig {
            cpu_bytes: 1,
            queue_capacity: 2,
            completion_capacity: 1,
        };
        let s = RenderScheduler::new(config, |_| Ok(tile())).unwrap();
        assert_eq!(
            s.update(1, vec![visible(0), visible(1), visible(2)]),
            Err(ViewportError::Capacity)
        );
        s.update(1, vec![visible(0), visible(1)]).unwrap();
        let r = drain(&s, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(s.metrics().cpu_cache_bytes, 0);
        assert_eq!(s.metrics().renders_performed, 2);
    }

    #[test]
    fn cached_viewport_movement_avoids_backend_rendering() {
        let s = RenderScheduler::new(Default::default(), |_| Ok(tile())).unwrap();
        s.update(1, vec![visible(0), visible(1)]).unwrap();
        let first = drain(&s, 2);
        assert_eq!(s.metrics().renders_performed, 2);
        s.update(2, vec![visible(1), visible(2)]).unwrap();
        let second = drain(&s, 2);
        assert_eq!(s.metrics().renders_performed, 3);
        assert!(Arc::ptr_eq(
            first[1].result.as_ref().unwrap(),
            second[0].result.as_ref().unwrap()
        ));
        s.update(3, vec![visible(0), visible(1)]).unwrap();
        drain(&s, 2);
        assert_eq!(s.metrics().renders_performed, 3);
    }

    #[test]
    fn overlapping_running_tile_is_retagged_without_duplicate_render() {
        let (tx, rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let s = RenderScheduler::new(Default::default(), move |_| {
            tx.send(()).unwrap();
            resume_rx.recv().unwrap();
            Ok(tile())
        })
        .unwrap();
        s.update(1, vec![visible(0)]).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.update(2, vec![visible(0)]).unwrap();
        resume_tx.send(()).unwrap();
        assert_eq!(drain(&s, 1)[0].generation, 2);
        assert_eq!(s.metrics().renders_performed, 1);
        assert_eq!(s.metrics().stale_renders_discarded, 0);
    }

    #[test]
    fn cached_completions_reserve_space_for_an_overlapping_running_render() {
        let (tx, rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let s = RenderScheduler::new(
            SchedulerConfig {
                completion_capacity: 1,
                ..Default::default()
            },
            move |key| {
                if key.tile_x == 1 {
                    tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }
                Ok(tile())
            },
        )
        .unwrap();
        s.update(1, vec![visible(0)]).unwrap();
        drain(&s, 1);
        s.update(2, vec![visible(1)]).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        s.update(3, vec![visible(0), visible(1)]).unwrap();
        assert_eq!(s.metrics().completion_depth, 0);
        resume_tx.send(()).unwrap();
        let results = drain(&s, 2);
        assert!(results.iter().all(|r| r.generation == 3));
        assert!(s.metrics().completion_depth <= 1);
        assert_eq!(s.metrics().renders_performed, 2);
    }
}
