//! One bounded worker, deterministic priority/LRU, and shared-buffer completions.
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

struct State {
    config: SchedulerConfig,
    cache: CpuTileCache,
    demand: Vec<TileDemand>,
    queue: VecDeque<TileKey>,
    ready: VecDeque<ReadyTile>,
    done: HashSet<TileKey>,
    inflight: Option<TileKey>,
    generation: u64,
    stop: bool,
    metrics: Metrics,
}
impl State {
    fn refill(&mut self) {
        self.queue.clear();
        // Reserve room for a current visible render already outside the lock.
        // An update/poll can otherwise fill ready from cache before it finishes.
        let reserved = usize::from(self.inflight.is_some_and(|key| {
            self.demand
                .iter()
                .any(|d| d.key == key && d.priority == Priority::Visible)
        }));
        let ready_limit = self.config.completion_capacity - reserved;
        for item in &self.demand {
            if self.done.contains(&item.key) || self.inflight == Some(item.key) {
                continue;
            }
            if let Some(tile) = self.cache.get(&item.key) {
                if item.priority == Priority::Visible {
                    if self.ready.len() >= ready_limit {
                        continue;
                    }
                    self.ready.push_back(ReadyTile {
                        key: item.key,
                        generation: self.generation,
                        result: Ok(tile),
                    });
                }
                self.done.insert(item.key);
            } else {
                self.queue.push_back(item.key);
            }
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
                config,
                cache: CpuTileCache::new(config.cpu_bytes),
                demand: Vec::new(),
                queue: VecDeque::new(),
                ready: VecDeque::new(),
                done: HashSet::new(),
                inflight: None,
                generation: 0,
                stop: false,
                metrics: Metrics::default(),
            }),
            wake: Condvar::new(),
        });
        let work = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("tile-render".into())
            .spawn(move || {
                loop {
                    let mut s = work.lock();
                    while !s.stop
                        && (s.queue.is_empty() || s.ready.len() == s.config.completion_capacity)
                    {
                        s = work.wake.wait(s).unwrap_or_else(|p| p.into_inner());
                    }
                    if s.stop {
                        break;
                    }
                    let key = s.queue.pop_front().unwrap();
                    s.inflight = Some(key);
                    s.metrics.renders_performed += 1;
                    drop(s);
                    // No scheduler/cache lock is held during serialized backend work.
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(key)))
                            .unwrap_or(Err(2))
                            .map(Arc::new);
                    let mut s = work.lock();
                    s.inflight = None;
                    if s.stop {
                        break;
                    }
                    let current = s.demand.iter().find(|d| d.key == key).copied();
                    if let Some(item) = current {
                        if let Ok(tile) = &result {
                            s.cache.insert(key, Arc::clone(tile));
                        } else {
                            s.metrics.render_errors += 1;
                        }
                        s.done.insert(key);
                        if item.priority == Priority::Visible {
                            let generation = s.generation;
                            s.ready.push_back(ReadyTile {
                                key,
                                generation,
                                result,
                            });
                        }
                    } else {
                        // Queued obsolete work is replaced on update; running work
                        // finishes safely, then is discarded without publishing.
                        s.metrics.stale_renders_discarded += 1;
                    }
                    s.refill();
                    work.wake.notify_all();
                }
            })
            .map_err(|_| ViewportError::InvalidInput)?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    pub fn capacity(&self) -> usize {
        self.shared.lock().config.queue_capacity
    }
    pub fn update(
        &self,
        generation: u64,
        mut demand: Vec<TileDemand>,
    ) -> Result<(), ViewportError> {
        let mut s = self.shared.lock();
        if generation == 0 || generation <= s.generation {
            return Err(ViewportError::StaleGeneration);
        }
        demand.sort_by_key(|d| d.priority);
        let mut seen = HashSet::new();
        demand.retain(|d| seen.insert(d.key));
        if demand.len() > s.config.queue_capacity {
            return Err(ViewportError::Capacity);
        }
        s.generation = generation;
        s.cache.pin(
            demand
                .iter()
                .filter(|d| d.priority == Priority::Visible)
                .map(|d| d.key),
        );
        s.metrics.tile_requests += demand.len() as u64;
        for d in &demand {
            if s.cache.entries.contains_key(&d.key) {
                s.metrics.cache_hits += 1;
            } else {
                s.metrics.cache_misses += 1;
            }
        }
        s.demand = demand;
        s.done.clear();
        s.ready.clear();
        s.refill();
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn poll(&self) -> Option<ReadyTile> {
        let mut s = self.shared.lock();
        let ready = s.ready.pop_front();
        s.refill();
        self.shared.wake.notify_all();
        ready
    }
    pub fn metrics(&self) -> Metrics {
        let s = self.shared.lock();
        Metrics {
            cpu_cache_bytes: s.cache.bytes(),
            queue_depth: s.queue.len(),
            completion_depth: s.ready.len(),
            ..s.metrics
        }
    }
}
impl Drop for RenderScheduler {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
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
