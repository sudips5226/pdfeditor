//! Demand-driven navigation: fixed rows, bounded recycling, no backend handles.
use crate::viewport::{TileKey, ViewportError};
use crate::{DocumentId, PageId, PagePlan, PageSize};
use std::ops::Range;

pub const DEFAULT_THUMBNAIL_BUDGET: usize = 32 * 1024 * 1024;
pub const MAX_THUMBNAIL_SLOTS: usize = 64;
// Internal work discriminator, removed before calling the backend.
pub(crate) const THUMBNAIL_WORK: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThumbnailKey {
    pub document_id: DocumentId,
    pub revision: u64,
    pub page_id: PageId,
    pub width: u32,
    pub height: u32,
    pub dpr_bits: u64,
    // Additional rotation; intrinsic rotation is part of immutable PageId/revision.
    pub rotation: u16,
    pub flags: u32,
}
impl ThumbnailKey {
    pub fn work_key(self) -> TileKey {
        TileKey {
            document_id: self.document_id,
            document_revision: self.revision,
            page_id: self.page_id,
            width: self.width,
            height: self.height,
            physical_scale_bits: self.dpr_bits,
            rotation_degrees: self.rotation,
            render_flags: self.flags | THUMBNAIL_WORK,
            tile_x: 0,
            tile_y: 0,
        }
    }
    pub fn from_work(k: TileKey) -> Self {
        Self {
            document_id: k.document_id,
            revision: k.document_revision,
            page_id: k.page_id,
            width: k.width,
            height: k.height,
            dpr_bits: k.physical_scale_bits,
            rotation: k.rotation_degrees,
            flags: k.render_flags & !THUMBNAIL_WORK,
        }
    }
}
pub fn is_thumbnail(k: TileKey) -> bool {
    k.render_flags & THUMBNAIL_WORK != 0
}

#[derive(Clone, Copy, Debug)]
pub struct ThumbnailLayout {
    pub width: f64,
    pub height: f64,
    pub label_height: f64,
    pub gap: f64,
    pub padding: f64,
    pub overscan: usize,
}
impl Default for ThumbnailLayout {
    fn default() -> Self {
        Self {
            width: 144.0,
            height: 168.0,
            label_height: 24.0,
            gap: 8.0,
            padding: 8.0,
            overscan: 2,
        }
    }
}
impl ThumbnailLayout {
    pub fn validate(
        self,
        offset: f64,
        extent: f64,
        dpr: f64,
        rotation: u16,
    ) -> Result<(), ViewportError> {
        if [
            self.width,
            self.height,
            self.label_height,
            self.gap,
            self.padding,
            offset,
            extent,
            dpr,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
            || self.width < 1.0
            || self.height < 1.0
            || extent < 1.0
            || dpr <= 0.0
            || self.width * dpr > 1024.0
            || self.height * dpr > 1024.0
            || self.overscan > 8
            || ![0, 90, 180, 270].contains(&rotation)
        {
            return Err(ViewportError::InvalidInput);
        }
        Ok(())
    }
    pub fn pitch(self) -> f64 {
        self.height + self.label_height + self.gap
    }
    pub fn top(self, index: usize) -> f64 {
        self.padding + index as f64 * self.pitch()
    }
    pub fn total(self, count: usize) -> f64 {
        2.0 * self.padding + count as f64 * self.pitch()
    }
    pub fn visible(self, count: usize, offset: f64, extent: f64) -> Range<usize> {
        let first = ((offset - self.padding).max(0.0) / self.pitch()).floor() as usize;
        let last = ((offset + extent - self.padding).max(0.0) / self.pitch()).ceil() as usize;
        first.min(count)..last.min(count)
    }
    pub fn overscanned(self, count: usize, visible: Range<usize>) -> Range<usize> {
        visible.start.saturating_sub(self.overscan)
            ..visible.end.saturating_add(self.overscan).min(count)
    }
    pub fn show_current(self, count: usize, index: usize, extent: f64) -> f64 {
        (self.top(index.min(count.saturating_sub(1))) + self.pitch() / 2.0 - extent / 2.0)
            .clamp(0.0, (self.total(count) - extent).max(0.0))
    }
}
pub fn aspect_fit(
    size: PageSize,
    width: f64,
    height: f64,
    rotation: u16,
) -> Result<PageSize, ViewportError> {
    if [size.width, size.height, width, height]
        .iter()
        .any(|v| !v.is_finite() || *v <= 0.0)
        || ![0, 90, 180, 270].contains(&rotation)
    {
        return Err(ViewportError::InvalidInput);
    }
    let (w, h) = if rotation == 90 || rotation == 270 {
        (size.height, size.width)
    } else {
        (size.width, size.height)
    };
    let scale = (width / w).min(height / h);
    Ok(PageSize {
        width: w * scale,
        height: h * scale,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThumbnailSlot {
    pub index: u32,
    pub key: ThumbnailKey,
    pub recycle: u64,
    pub current: bool,
    pub visible: bool,
}
impl ThumbnailSlot {
    pub fn accepts(self, key: ThumbnailKey, recycle: u64) -> bool {
        self.key == key && self.recycle == recycle && self.key.page_id == key.page_id
    }
}
#[derive(Default)]
pub struct ThumbnailNavigator {
    pub slots: Vec<ThumbnailSlot>,
    pub recycled: u64,
    next_recycle: u64,
}
impl ThumbnailNavigator {
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        plan: &PagePlan,
        document: DocumentId,
        layout: ThumbnailLayout,
        offset: f64,
        extent: f64,
        dpr: f64,
        rotation: u16,
        current: u32,
    ) -> Result<(), ViewportError> {
        layout.validate(offset, extent, dpr, rotation)?;
        let visible = layout.visible(plan.entries().len(), offset, extent);
        let range = layout.overscanned(plan.entries().len(), visible.clone());
        if range.len() > MAX_THUMBNAIL_SLOTS {
            return Err(ViewportError::Capacity);
        }
        let mut next = Vec::with_capacity(range.len());
        let mut retained = 0;
        let mut assigned = 0;
        for i in range {
            let page = plan.get(i as u32).ok_or(ViewportError::InvalidInput)?;
            let key = ThumbnailKey {
                document_id: document,
                revision: 0,
                page_id: page.id,
                width: (layout.width * dpr).ceil() as u32,
                height: (layout.height * dpr).ceil() as u32,
                dpr_bits: dpr.to_bits(),
                rotation: (rotation + page.rotation) % 360,
                flags: 0,
            };
            let recycle = if let Some(old) = self.slots.iter().find(|s| s.key == key) {
                retained += 1;
                old.recycle
            } else {
                self.next_recycle += 1;
                assigned += 1;
                self.next_recycle
            };
            next.push(ThumbnailSlot {
                index: i as u32,
                key,
                recycle,
                current: i == current as usize,
                visible: visible.contains(&i),
            });
        }
        self.recycled += assigned.min(self.slots.len() - retained) as u64;
        self.slots = next;
        Ok(())
    }
    pub fn sync_current(&mut self, current: u32) {
        for s in &mut self.slots {
            s.current = s.index == current;
        }
    }
    pub fn navigation_index(&self, page: PageId, recycle: u64) -> Option<u32> {
        self.slots
            .iter()
            .find(|s| s.key.page_id == page && s.recycle == recycle)
            .map(|s| s.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aspect_geometry_refinement_and_rotation() {
        let portrait = aspect_fit(
            PageSize {
                width: 595.0,
                height: 842.0,
            },
            144.0,
            168.0,
            0,
        )
        .unwrap();
        assert_eq!(portrait.height, 168.0);
        let landscape = aspect_fit(
            PageSize {
                width: 1200.0,
                height: 600.0,
            },
            144.0,
            168.0,
            0,
        )
        .unwrap();
        assert_eq!(
            landscape,
            PageSize {
                width: 144.0,
                height: 72.0
            }
        );
        assert!(
            aspect_fit(
                PageSize {
                    width: 600.0,
                    height: 1200.0
                },
                144.0,
                168.0,
                90
            )
            .unwrap()
                == landscape
        );
        assert!(aspect_fit(
            PageSize {
                width: 0.0,
                height: 1.0
            },
            144.0,
            168.0,
            0
        )
        .is_err());
    }
    #[test]
    fn ten_thousand_pages_recycling_sync_navigation_and_keys() {
        let plan = PagePlan::from_original_pages(10_000);
        let layout = ThumbnailLayout::default();
        let mut n = ThumbnailNavigator::default();
        for index in [0, 4999, 9999] {
            let y = layout.show_current(10_000, index, 600.0);
            n.update(&plan, DocumentId(1), layout, y, 600.0, 1.0, 0, index as u32)
                .unwrap();
            assert!(n.slots.len() <= 8);
            assert!(layout.visible(10_000, y, 600.0).contains(&index));
            let slot = *n.slots.iter().find(|s| s.current).unwrap();
            assert_eq!(
                n.navigation_index(slot.key.page_id, slot.recycle),
                Some(index as u32)
            );
        }
        let old = n.slots[0];
        n.update(&plan, DocumentId(1), layout, 0.0, 600.0, 1.0, 0, 9999)
            .unwrap();
        assert!(!n.slots[0].accepts(old.key, old.recycle));
        let first = n.slots[0];
        n.sync_current(0);
        assert!(n.slots[0].current);
        n.update(&plan, DocumentId(1), layout, 0.0, 600.0, 1.0, 0, 0)
            .unwrap();
        assert_eq!(n.slots[0].key, first.key); // selection is not pixel identity
        assert_eq!(n.slots[0].recycle, first.recycle);
        n.update(&plan, DocumentId(1), layout, 0.0, 600.0, 2.0, 0, 0)
            .unwrap();
        assert_ne!(n.slots[0].key, first.key);
        let dpr = n.slots[0];
        n.update(&plan, DocumentId(1), layout, 0.0, 600.0, 2.0, 90, 0)
            .unwrap();
        assert_ne!(n.slots[0].key, dpr.key);
        assert!(n.slots[0].accepts(n.slots[0].key, n.slots[0].recycle));
        assert!(!n.slots[0].accepts(n.slots[0].key, dpr.recycle));
        assert_eq!(ThumbnailKey::from_work(first.key.work_key()), first.key);
    }
    #[test]
    fn ranges_overscan_show_current_and_capacity() {
        let l = ThumbnailLayout::default();
        assert_eq!(l.visible(10_000, 0.0, 600.0), 0..3);
        assert_eq!(l.overscanned(10_000, 0..3), 0..5);
        assert_eq!(l.overscanned(10_000, 9998..10000), 9996..10000);
        assert_eq!(l.show_current(10000, 0, 600.0), 0.0);
        assert_eq!(l.show_current(10000, 9999, 600.0), l.total(10000) - 600.0);
        let mut n = ThumbnailNavigator::default();
        assert_eq!(
            n.update(
                &PagePlan::from_original_pages(10000),
                DocumentId(1),
                l,
                0.0,
                20000.0,
                1.0,
                0,
                0
            ),
            Err(ViewportError::Capacity)
        );
        assert!(n.slots.is_empty());
    }
}
