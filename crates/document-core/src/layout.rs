//! Continuous document coordinates are unscaled points, top-left / y-down.
//! Only metadata lives here. Two Fenwick sums support lazy geometry refinement
//! and quarter-turn layout changes without rebuilding or scanning all pages.
use crate::viewport::{
    tile_demand, DeviceSize, Priority, TileDemand, ViewportError, ViewportState,
};
use crate::{DevicePoint, DocumentId, PageGeometry, PageId, PagePlan, PagePoint, PageSize};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DocumentPoint {
    pub x: f64,
    pub y: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocumentRect {
    pub origin: DocumentPoint,
    pub size: PageSize,
}
#[derive(Clone, Copy, Debug)]
pub struct PageLayout {
    pub page_id: PageId,
    pub index: u32,
    pub geometry: PageGeometry,
    pub known: bool,
    pub intrinsic_rotation_known: bool,
    pub effective_rotation: u16,
    pub bounds: DocumentRect,
    pub spacing_before: f64,
    pub spacing_after: f64,
    pub viewer_rotation: u16,
}
impl PageLayout {
    pub fn page_to_document(self, p: PagePoint) -> DocumentPoint {
        let s = self.geometry.size;
        let (x, y) = match self.viewer_rotation {
            0 => (p.x, p.y),
            90 => (s.height - p.y, p.x),
            180 => (s.width - p.x, s.height - p.y),
            270 => (p.y, s.width - p.x),
            _ => unreachable!(),
        };
        DocumentPoint {
            x: x + self.bounds.origin.x,
            y: y + self.bounds.origin.y,
        }
    }
    pub fn document_to_page(self, p: DocumentPoint) -> PagePoint {
        let x = p.x - self.bounds.origin.x;
        let y = p.y - self.bounds.origin.y;
        let s = self.geometry.size;
        let (x, y) = match self.viewer_rotation {
            0 => (x, y),
            90 => (y, s.height - x),
            180 => (s.width - x, s.height - y),
            270 => (s.width - y, x),
            _ => unreachable!(),
        };
        PagePoint { x, y }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct DocumentViewport {
    pub origin: DocumentPoint,
    pub extent: DeviceSize,
    pub scale: f64,
    pub device_pixel_ratio: f64,
    pub rotation_degrees: u16,
    pub page_gap: f64,
    pub generation: u64,
}
impl DocumentViewport {
    pub fn validate(self) -> Result<(), ViewportError> {
        let physical = self.physical_scale();
        if self.generation == 0
            || ![0, 90, 180, 270].contains(&self.rotation_degrees)
            || [
                self.origin.x,
                self.origin.y,
                self.extent.width,
                self.extent.height,
                self.scale,
                self.device_pixel_ratio,
                self.page_gap,
                physical,
            ]
            .iter()
            .any(|x| !x.is_finite())
            || self.scale <= 0.0
            || self.device_pixel_ratio <= 0.0
            || physical <= 0.0
            || self.extent.width <= 0.0
            || self.extent.height <= 0.0
            || self.page_gap < 0.0
            || self.extent.width > 16384.0
            || self.extent.height > 16384.0
            || self.origin.x.abs() > 1e15
            || self.origin.y.abs() > 1e15
            || self.page_gap > 1e9
            || self.extent.width / physical > 1e15
            || self.extent.height / physical > 1e15
        {
            return Err(ViewportError::InvalidInput);
        }
        Ok(())
    }
    pub fn physical_scale(self) -> f64 {
        self.scale * self.device_pixel_ratio
    }
    pub fn document_to_device(self, p: DocumentPoint) -> DevicePoint {
        DevicePoint {
            x: (p.x - self.origin.x) * self.physical_scale(),
            y: (p.y - self.origin.y) * self.physical_scale(),
        }
    }
    pub fn device_to_document(self, p: DevicePoint) -> DocumentPoint {
        DocumentPoint {
            x: self.origin.x + p.x / self.physical_scale(),
            y: self.origin.y + p.y / self.physical_scale(),
        }
    }
    /// The specified device anchor keeps the same unscaled document point.
    pub fn zoom_at(self, scale: f64, dpr: f64, anchor: DevicePoint) -> Result<Self, ViewportError> {
        let p = self.device_to_document(anchor);
        let mut next = Self {
            scale,
            device_pixel_ratio: dpr,
            ..self
        };
        next.validate()?;
        next.origin = DocumentPoint {
            x: p.x - anchor.x / next.physical_scale(),
            y: p.y - anchor.y / next.physical_scale(),
        };
        next.validate()?;
        Ok(next)
    }
}
#[derive(Clone)]
struct Fenwick {
    sums: Vec<f64>,
}
impl Fenwick {
    fn uniform(n: usize, v: f64) -> Self {
        let mut sums = vec![0.0; n + 1];
        for (i, x) in sums.iter_mut().enumerate().skip(1) {
            *x = v * i.isolate_lowest_one() as f64;
        }
        Self { sums }
    }
    fn add(&mut self, mut i: usize, delta: f64) {
        i += 1;
        while i < self.sums.len() {
            self.sums[i] += delta;
            i += i.isolate_lowest_one();
        }
    }
    fn prefix(&self, mut n: usize) -> f64 {
        let mut sum = 0.0;
        while n > 0 {
            sum += self.sums[n];
            n &= n - 1;
        }
        sum
    }
}
#[derive(Clone, Copy)]
struct Metadata {
    id: PageId,
    geometry: PageGeometry,
    known: bool,
    rotation_known: bool,
    editing_rotation: u16,
}
pub struct DocumentLayout {
    pages: Vec<Metadata>,
    heights: Fenwick,
    widths: Fenwick,
    max_width: f64,
    max_height: f64,
    retained: HashMap<PageId, Metadata>,
}
pub const MARGIN: f64 = 24.0;
pub const MAX_FRAME_PAGES: usize = 64;
impl DocumentLayout {
    pub fn new(plan: &PagePlan) -> Self {
        let estimate = PageGeometry {
            size: PageSize {
                width: 595.0,
                height: 842.0,
            },
            rotation_degrees: 0,
        };
        let mut layout = Self {
            pages: plan
                .entries()
                .iter()
                .map(|p| Metadata {
                    id: p.id,
                    geometry: estimate,
                    known: false,
                    rotation_known: false,
                    editing_rotation: p.rotation,
                })
                .collect(),
            heights: Fenwick::uniform(plan.entries().len(), 842.0),
            widths: Fenwick::uniform(plan.entries().len(), 595.0),
            max_width: 595.0,
            max_height: 842.0,
            retained: HashMap::new(),
        };
        layout.rebuild_sums();
        layout
    }
    /// O(N) metadata synchronization; exact geometry survives moves/deletes/undo.
    pub fn sync_plan(&mut self, plan: &PagePlan) {
        for p in &self.pages {
            if p.known {
                self.retained.insert(p.id, *p);
            }
        }
        let mut next = Self::new(plan);
        for p in &mut next.pages {
            if let Some(old) = self.retained.get(&p.id) {
                p.geometry = old.geometry;
                p.known = old.known;
                p.rotation_known = old.rotation_known;
            }
        }
        next.retained = std::mem::take(&mut self.retained);
        next.rebuild_sums();
        *self = next;
    }
    fn edited_size(p: Metadata) -> PageSize {
        if p.editing_rotation.is_multiple_of(180) {
            p.geometry.size
        } else {
            PageSize {
                width: p.geometry.size.height,
                height: p.geometry.size.width,
            }
        }
    }
    fn rebuild_sums(&mut self) {
        self.heights = Fenwick::uniform(self.pages.len(), 0.0);
        self.widths = Fenwick::uniform(self.pages.len(), 0.0);
        self.max_width = 0.0;
        self.max_height = 0.0;
        for (i, p) in self.pages.iter().enumerate() {
            let size = Self::edited_size(*p);
            self.heights.sums[i + 1] = size.height;
            self.widths.sums[i + 1] = size.width;
            self.max_width = self.max_width.max(size.width);
            self.max_height = self.max_height.max(size.height);
        }
        for i in 1..=self.pages.len() {
            let parent = i + i.isolate_lowest_one();
            if parent <= self.pages.len() {
                self.heights.sums[parent] += self.heights.sums[i];
                self.widths.sums[parent] += self.widths.sums[i];
            }
        }
    }
    pub fn geometry_known(&self, index: usize) -> bool {
        self.pages.get(index).is_some_and(|p| p.known)
    }
    pub fn len(&self) -> usize {
        self.pages.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
    pub fn metadata_bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<Metadata>()
            + (self.heights.sums.capacity() + self.widths.sums.capacity()) * 8
            + self.retained.capacity() * (std::mem::size_of::<Metadata>() + 32)
    }
    pub fn resolve(&mut self, index: usize, g: PageGeometry) -> Result<(), ViewportError> {
        if !g.size.width.is_finite()
            || !g.size.height.is_finite()
            || g.size.width <= 0.0
            || g.size.height <= 0.0
            || g.size.width > 1e9
            || g.size.height > 1e9
            || ![0, 90, 180, 270].contains(&g.rotation_degrees)
        {
            return Err(ViewportError::InvalidInput);
        }
        let p = self
            .pages
            .get_mut(index)
            .ok_or(ViewportError::InvalidInput)?;
        let old = Self::edited_size(*p);
        p.geometry = g;
        let size = Self::edited_size(*p);
        self.heights.add(index, size.height - old.height);
        self.widths.add(index, size.width - old.width);
        p.known = true;
        p.rotation_known = true;
        self.max_width = self.max_width.max(size.width);
        self.max_height = self.max_height.max(size.height);
        Ok(())
    }
    /// Size-only PDFium metadata can be used before content/rotation parsing.
    pub fn resolve_size(&mut self, index: usize, size: PageSize) -> Result<(), ViewportError> {
        if self.pages.get(index).is_some_and(|p| p.rotation_known) {
            return Ok(());
        }
        self.resolve(
            index,
            PageGeometry {
                size,
                rotation_degrees: 0,
            },
        )?;
        self.pages[index].rotation_known = false;
        Ok(())
    }
    fn top(&self, index: usize, v: DocumentViewport) -> f64 {
        MARGIN
            + if v.rotation_degrees.is_multiple_of(180) {
                self.heights.prefix(index)
            } else {
                self.widths.prefix(index)
            }
            + index as f64 * v.page_gap
    }
    pub fn extent(&self, v: DocumentViewport) -> PageSize {
        PageSize {
            width: 2.0 * MARGIN
                + if v.rotation_degrees.is_multiple_of(180) {
                    self.max_width
                } else {
                    self.max_height
                },
            height: 2.0 * MARGIN + self.top(self.len(), v)
                - MARGIN
                - self.len().min(1) as f64 * v.page_gap,
        }
    }
    pub fn page(&self, index: usize, v: DocumentViewport) -> Option<PageLayout> {
        let p = *self.pages.get(index)?;
        let edited = Self::edited_size(p);
        let rotation = (p.editing_rotation + v.rotation_degrees) % 360;
        let size = if v.rotation_degrees.is_multiple_of(180) {
            edited
        } else {
            PageSize {
                width: edited.height,
                height: edited.width,
            }
        };
        Some(PageLayout {
            page_id: p.id,
            index: index as u32,
            geometry: p.geometry,
            known: p.known,
            intrinsic_rotation_known: p.rotation_known,
            effective_rotation: (p.geometry.rotation_degrees + rotation) % 360,
            bounds: DocumentRect {
                origin: DocumentPoint {
                    x: MARGIN,
                    y: self.top(index, v),
                },
                size,
            },
            spacing_before: if index == 0 { MARGIN } else { v.page_gap },
            spacing_after: if index + 1 == self.len() {
                MARGIN
            } else {
                v.page_gap
            },
            viewer_rotation: rotation,
        })
    }
    /// Last page top <= Y. Gaps belong to the preceding page; ends clamp.
    /// Binary search of prefix sums is O(log² N), with no page scan.
    pub fn page_at_y(&self, y: f64, v: DocumentViewport) -> Option<usize> {
        if self.is_empty() || !y.is_finite() {
            return None;
        }
        let mut lo = 0;
        let mut hi = self.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.top(mid, v) <= y {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Some(lo.saturating_sub(1))
    }
    /// Page whose top precedes the viewport center (gap: preceding page).
    pub fn current_page(&self, v: DocumentViewport) -> Option<usize> {
        self.page_at_y(v.origin.y + v.extent.height / (2.0 * v.physical_scale()), v)
    }
    pub fn go_to_page(&self, index: usize, v: DocumentViewport) -> Option<DocumentPoint> {
        self.page(index, v).map(|p| DocumentPoint {
            x: 0.0,
            y: p.bounds.origin.y,
        })
    }
    pub fn visible_range(
        &self,
        v: DocumentViewport,
        neighbors: usize,
    ) -> Result<Range<usize>, ViewportError> {
        v.validate()?;
        if neighbors > MAX_FRAME_PAGES {
            return Err(ViewportError::Capacity);
        }
        if self.is_empty() {
            return Ok(0..0);
        }
        let bottom = v.origin.y + v.extent.height / v.physical_scale();
        let mut start = self.page_at_y(v.origin.y, v).unwrap();
        let p = self.page(start, v).unwrap();
        if p.bounds.origin.y + p.bounds.size.height <= v.origin.y {
            start += 1;
        }
        let mut end = self.page_at_y(bottom, v).unwrap() + 1;
        if end > 0 && self.top(end - 1, v) >= bottom {
            end -= 1;
        }
        if bottom <= MARGIN || v.origin.y >= self.extent(v).height - MARGIN {
            return Ok(0..0);
        }
        let range = start.saturating_sub(neighbors)..(end + neighbors).min(self.len());
        if range.len() > MAX_FRAME_PAGES {
            return Err(ViewportError::Capacity);
        }
        Ok(range)
    }
    pub fn intersects(p: PageLayout, v: DocumentViewport) -> bool {
        let a = v.document_to_device(p.bounds.origin);
        a.x < v.extent.width
            && a.y < v.extent.height
            && a.x + p.bounds.size.width * v.physical_scale() > 0.0
            && a.y + p.bounds.size.height * v.physical_scale() > 0.0
    }
    /// Visible pages use clipped page-local viewports, so large document Y
    /// never reaches PDFium's float32 page-coordinate boundary.
    pub fn demand(
        &self,
        id: DocumentId,
        v: DocumentViewport,
        capacity: usize,
    ) -> Result<Vec<TileDemand>, ViewportError> {
        let mut all = Vec::new();
        for index in self.visible_range(v, 1)? {
            let p = self.page(index, v).unwrap();
            if !p.known {
                continue;
            }
            let visible = Self::intersects(p, v);
            let s = v.physical_scale();
            let offset = v.document_to_device(p.bounds.origin);
            let x = (-offset.x).max(0.0);
            let y = if visible {
                (-offset.y).max(0.0)
            } else if offset.y < 0.0 {
                (p.bounds.size.height * s - 512.0).max(0.0)
            } else {
                0.0
            };
            let width = if visible {
                ((p.bounds.size.width * s).min(v.extent.width - offset.x) - x).max(0.0)
            } else {
                v.extent.width.min((p.bounds.size.width * s - x).max(0.0))
            };
            let height = if visible {
                ((p.bounds.size.height * s).min(v.extent.height - offset.y) - y).max(0.0)
            } else {
                512.0_f64.min(p.bounds.size.height * s)
            };
            if width <= 0.0 || height <= 0.0 {
                continue;
            }
            let local = ViewportState {
                page_id: p.page_id,
                origin: DevicePoint { x, y },
                extent: DeviceSize { width, height },
                scale: v.scale,
                device_pixel_ratio: v.device_pixel_ratio,
                rotation_degrees: p.viewer_rotation,
                generation: v.generation,
            };
            let mut d = tile_demand(
                id,
                0,
                p.geometry.size,
                local,
                capacity.saturating_sub(all.len()),
            )?;
            if !visible {
                for t in &mut d {
                    t.priority = Priority::Prefetch;
                }
            }
            all.extend(d);
        }
        all.sort_by_key(|d| d.priority);
        Ok(all)
    }
    /// One viewport ahead at the same exact physical scale. Speculation is
    /// bounded and optional; it must never reject an otherwise valid destination.
    pub fn demand_directional(
        &self,
        id: DocumentId,
        v: DocumentViewport,
        capacity: usize,
        direction: i32,
    ) -> Result<Vec<TileDemand>, ViewportError> {
        let mut demand = self.demand(id, v, capacity)?;
        if direction == 0 {
            return Ok(demand);
        }
        let mut ahead = v;
        ahead.origin.y =
            (v.origin.y + direction as f64 * v.extent.height / v.physical_scale()).max(0.0);
        if let Ok(next) = self.demand(id, ahead, capacity) {
            for mut item in next
                .into_iter()
                .filter(|d| d.priority == Priority::Visible)
                .take(32)
            {
                item.priority = Priority::Directional;
                if let Some(existing) = demand.iter_mut().find(|d| d.key == item.key) {
                    if existing.priority != Priority::Visible {
                        existing.priority = Priority::Directional;
                    }
                } else if demand.len() < capacity {
                    demand.push(item);
                }
            }
        }
        demand.sort_by_key(|d| d.priority);
        Ok(demand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> DocumentViewport {
        DocumentViewport {
            origin: DocumentPoint { x: 0.0, y: 0.0 },
            extent: DeviceSize {
                width: 1024.0,
                height: 1024.0,
            },
            scale: 1.0,
            device_pixel_ratio: 1.0,
            rotation_degrees: 0,
            page_gap: 24.0,
            generation: 1,
        }
    }
    fn layout(n: u32) -> DocumentLayout {
        DocumentLayout::new(&PagePlan::from_original_pages(n))
    }
    fn geometry(w: f64, h: f64, r: u16) -> PageGeometry {
        PageGeometry {
            size: PageSize {
                width: w,
                height: h,
            },
            rotation_degrees: r,
        }
    }
    #[test]
    fn directional_prefetch_preserves_exact_quality_and_visible_keys() {
        let mut l = layout(10_000);
        for i in 0..8 {
            l.resolve(i, geometry(600.0, 800.0, 0)).unwrap();
        }
        let mut v = DocumentViewport {
            scale: 2.0,
            device_pixel_ratio: 1.5,
            ..view()
        };
        v.origin = l.go_to_page(3, v).unwrap();
        let ordinary = l.demand(DocumentId(1), v, 256).unwrap();
        let mandatory: Vec<_> = ordinary
            .iter()
            .filter(|d| d.priority == Priority::Visible)
            .map(|d| d.key)
            .collect();
        for direction in [-1, 1] {
            let d = l
                .demand_directional(DocumentId(1), v, 256, direction)
                .unwrap();
            assert_eq!(
                mandatory,
                d.iter()
                    .filter(|d| d.priority == Priority::Visible)
                    .map(|d| d.key)
                    .collect::<Vec<_>>()
            );
            assert!(d.iter().any(|d| d.priority == Priority::Directional));
            assert!(d
                .iter()
                .all(|d| d.key.physical_scale_bits == 3f64.to_bits()));
            assert!(d.len() <= 256);
        }
    }
    #[test]
    fn large_document_y_clipping_keeps_exact_tile_boundaries() {
        let mut l = layout(10_000);
        l.resolve(9999, geometry(800.0, 700.0, 0)).unwrap();
        let mut v = DocumentViewport {
            scale: 2.0,
            device_pixel_ratio: 1.5,
            ..view()
        };
        v.origin = l.go_to_page(9999, v).unwrap();
        // Document Y in physical pixels exceeds the inherited P2 float limit.
        assert!(v.origin.y * v.physical_scale() > 16_777_216.0);
        let demand = l.demand(DocumentId(1), v, 256).unwrap();
        let visible: Vec<_> = demand
            .iter()
            .filter(|d| d.priority == Priority::Visible)
            .collect();
        assert_eq!(visible.len(), 4);
        assert!(visible.iter().all(|d| d.key.tile_y < 2));
    }

    #[test]
    fn page_tile_document_viewport_chain_for_all_rotations() {
        let mut l = layout(2);
        l.resolve(0, geometry(800.0, 600.0, 90)).unwrap();
        l.resolve(1, geometry(900.0, 700.0, 270)).unwrap();
        for rotation in [0, 90, 180, 270] {
            let v = DocumentViewport {
                scale: 2.0,
                device_pixel_ratio: 1.5,
                rotation_degrees: rotation,
                ..view()
            };
            for index in [0, 1] {
                let page = l.page(index, v).unwrap();
                let request = crate::TileRequest {
                    page_id: page.page_id,
                    tile_x: 1,
                    tile_y: 1,
                    scale: v.scale,
                    device_pixel_ratio: v.device_pixel_ratio,
                    rotation_degrees: rotation,
                    width: 512,
                    height: 512,
                };
                let point = PagePoint { x: 123.0, y: 234.0 };
                let local = request.page_to_tile(page.geometry.size).unwrap().map(point);
                let placed = v.document_to_device(page.page_to_document(point));
                let origin = v.document_to_device(page.bounds.origin);
                assert!((placed.x - origin.x - local.x - 512.0).abs() < 1e-9);
                assert!((placed.y - origin.y - local.y - 512.0).abs() < 1e-9);
                assert_eq!(page.document_to_page(page.page_to_document(point)), point);
            }
        }
    }

    #[test]
    fn mixed_vertical_layout_lookup_and_gaps() {
        let mut l = layout(3);
        l.resolve(0, geometry(595.0, 842.0, 0)).unwrap();
        l.resolve(1, geometry(1191.0, 842.0, 90)).unwrap();
        l.resolve(2, geometry(2400.0, 3600.0, 0)).unwrap();
        let v = view();
        assert_eq!(l.page(1, v).unwrap().bounds.origin.y, 890.0);
        assert_eq!(l.page(2, v).unwrap().bounds.origin.y, 1756.0);
        assert_eq!(l.extent(v).height, 5380.0);
        assert_eq!(l.page_at_y(880.0, v), Some(0));
        assert_eq!(l.page_at_y(890.0, v), Some(1));
        assert_eq!(l.visible_range(v, 0).unwrap(), 0..2);
        let gap = DocumentViewport {
            origin: DocumentPoint { x: 0.0, y: 868.0 },
            extent: DeviceSize {
                width: 1024.0,
                height: 10.0,
            },
            ..v
        };
        assert_eq!(l.visible_range(gap, 0).unwrap(), 1..1);
        assert_eq!(l.current_page(gap), Some(0));
        assert_eq!(l.go_to_page(2, v).unwrap().y, 1756.0);
        assert!(l.go_to_page(3, v).is_none());
    }
    #[test]
    fn transforms_rotation_dpr_zoom_and_tile_placement() {
        let mut l = layout(2);
        l.resolve(0, geometry(800.0, 600.0, 90)).unwrap();
        l.resolve(1, geometry(900.0, 700.0, 0)).unwrap();
        for rotation in [0, 90, 180, 270] {
            let v = DocumentViewport {
                rotation_degrees: rotation,
                scale: 2.0,
                device_pixel_ratio: 1.5,
                ..view()
            };
            let p = l.page(1, v).unwrap();
            let point = PagePoint { x: 123.0, y: 234.0 };
            assert_eq!(p.document_to_page(p.page_to_document(point)), point);
            let dp = p.page_to_document(point);
            let back = v.device_to_document(v.document_to_device(dp));
            assert!((back.y - dp.y).abs() < 1e-9);
            assert_eq!(p.effective_rotation, rotation);
            let anchor = DevicePoint { x: 512.0, y: 512.0 };
            let next = v.zoom_at(1.25, 2.0, anchor).unwrap();
            assert_eq!(
                v.device_to_document(anchor),
                next.device_to_document(anchor)
            );
        }
        let d = l.demand(DocumentId(1), view(), 256).unwrap();
        assert!(d
            .iter()
            .any(|t| t.key.page_id == l.page(0, view()).unwrap().page_id));
        assert!(d
            .iter()
            .any(|t| t.key.page_id == l.page(1, view()).unwrap().page_id && t.key.tile_y == 0));
        for t in d {
            let index = usize::from(t.key.page_id == l.page(1, view()).unwrap().page_id);
            let p = l.page(index, view()).unwrap();
            let device = view().document_to_device(DocumentPoint {
                x: p.bounds.origin.x + f64::from(t.key.tile_x) * 512.0,
                y: p.bounds.origin.y + f64::from(t.key.tile_y) * 512.0,
            });
            assert!(device.y >= p.bounds.origin.y);
        }
    }
    #[test]
    fn ten_thousand_pages_and_large_extent() {
        let mut l = layout(10_000);
        let v = view();
        assert_eq!(l.extent(v).height, 8_660_024.0);
        assert!(l.metadata_bytes() < 1_000_000);
        for i in [0, 1, 5000, 9999] {
            let y = l.go_to_page(i, v).unwrap().y;
            let at = DocumentViewport {
                origin: DocumentPoint { x: 0.0, y },
                ..v
            };
            assert_eq!(l.page_at_y(y, v), Some(i));
            assert_eq!(l.current_page(at), Some(i));
            assert!(l.visible_range(at, 1).unwrap().len() <= 4);
        }
        let low = DocumentViewport { scale: 0.0001, ..v };
        assert_eq!(l.visible_range(low, 0), Err(ViewportError::Capacity));
        l.resolve(0, geometry(1000.0, 1e9, 0)).unwrap();
        assert!(l.extent(v).height > 1e9);
        let huge = DocumentViewport { page_gap: 1e6, ..v };
        assert!(l.extent(huge).height > f64::from(u32::MAX));
        let last = l.go_to_page(9999, huge).unwrap();
        assert_eq!(l.page_at_y(last.y, huge), Some(9999));
    }
    #[test]
    fn lazy_refinement_and_rotation_do_not_rebuild_model() {
        let mut l = layout(10_000);
        let v = view();
        let id = l.page(9999, v).unwrap().page_id;
        l.resolve(2, geometry(1200.0, 400.0, 270)).unwrap();
        assert_eq!(
            l.go_to_page(9999, v).unwrap().y,
            24.0 + 9999.0 * 866.0 - 442.0
        );
        let rotated = DocumentViewport {
            rotation_degrees: 90,
            ..v
        };
        assert_eq!(l.page(2, rotated).unwrap().effective_rotation, 0);
        assert_eq!(l.page(9999, rotated).unwrap().page_id, id);
        assert_eq!(
            l.go_to_page(9999, rotated).unwrap().y,
            24.0 + 9999.0 * 619.0 + 605.0
        );
    }
}
