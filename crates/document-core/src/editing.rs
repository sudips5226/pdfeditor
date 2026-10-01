//! Logical editing only: no source bytes, backend objects, or raster resources.
use crate::{PageId, PagePlan, PagePlanEntry, SourceId};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditError {
    NoSelection,
    StalePage,
    InvalidDestination,
    LastPage,
    InvalidRotation,
    NoHistory,
    InvalidSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionMode {
    Plain,
    Toggle,
    Range,
}

impl PagePlan {
    fn validate_ids(&self, ids: &HashSet<PageId>) -> Result<(), EditError> {
        if ids.is_empty() {
            return Err(EditError::NoSelection);
        }
        if ids.iter().any(|id| self.position_of(*id).is_none()) {
            return Err(EditError::StalePage);
        }
        Ok(())
    }
    pub fn delete_pages(&mut self, ids: &HashSet<PageId>) -> Result<(), EditError> {
        self.validate_ids(ids)?;
        if ids.len() == self.entries().len() {
            return Err(EditError::LastPage);
        }
        self.replace(
            self.entries()
                .iter()
                .filter(|e| !ids.contains(&e.id))
                .copied()
                .collect(),
        );
        Ok(())
    }
    /// Insert before the original zero-based boundary (len = end). Count removed
    /// selected entries before that boundary to normalize it. Group order is stable.
    pub fn move_pages(&mut self, ids: &HashSet<PageId>, boundary: usize) -> Result<(), EditError> {
        self.validate_ids(ids)?;
        if boundary > self.entries().len() {
            return Err(EditError::InvalidDestination);
        }
        let mut group = Vec::with_capacity(ids.len());
        let mut remaining = Vec::with_capacity(self.entries().len() - ids.len());
        let mut removed_before = 0;
        for (i, e) in self.entries().iter().enumerate() {
            if ids.contains(&e.id) {
                group.push(*e);
                removed_before += usize::from(i < boundary);
            } else {
                remaining.push(*e);
            }
        }
        remaining.splice(boundary - removed_before..boundary - removed_before, group);
        self.replace(remaining);
        Ok(())
    }
    pub fn rotate_pages(&mut self, ids: &HashSet<PageId>, degrees: i32) -> Result<(), EditError> {
        self.validate_ids(ids)?;
        if ![-270, -180, -90, 90, 180, 270].contains(&degrees) {
            return Err(EditError::InvalidRotation);
        }
        let entries = self
            .entries()
            .iter()
            .map(|e| {
                let mut e = *e;
                if ids.contains(&e.id) {
                    e.rotation = (i32::from(e.rotation) + degrees).rem_euclid(360) as u16;
                }
                e
            })
            .collect();
        self.replace(entries);
        Ok(())
    }
}

#[derive(Clone)]
struct Snapshot {
    entries: Vec<PagePlanEntry>,
    selected: HashSet<PageId>,
    anchor: Option<PageId>,
    current: PageId,
}
impl Snapshot {
    fn bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<PagePlanEntry>()
            + self.selected.capacity() * 32
            + 128
    }
}
struct Command {
    before: Snapshot,
    after: Snapshot,
}
impl Command {
    fn bytes(&self) -> usize {
        self.before.bytes() + self.after.bytes()
    }
}

pub struct Editor {
    pub page_plan: PagePlan,
    selected: HashSet<PageId>,
    anchor: Option<PageId>,
    current: PageId,
    baseline: Vec<PagePlanEntry>,
    saved_plan_fingerprint: u64,
    dirty: bool,
    undo: VecDeque<Command>,
    redo: VecDeque<Command>,
    limit: usize,
    byte_limit: usize,
    pub revision: u64,
    pub selection_revision: u64,
    pub saved_revision: u64,
    source_counts: std::collections::HashMap<SourceId, u32>,
}
impl Editor {
    pub fn new(plan: PagePlan) -> Self {
        let primary_count = plan.entries().len() as u32;
        let current = plan
            .get(0)
            .expect("an opened document must contain a page")
            .id;
        Self {
            dirty: false,
            baseline: plan.entries().to_vec(),
            saved_plan_fingerprint: plan_fingerprint(plan.entries()),
            page_plan: plan,
            selected: HashSet::new(),
            anchor: None,
            current,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            limit: 100,
            byte_limit: 64 * 1024 * 1024,
            revision: 0,
            selection_revision: 0,
            saved_revision: 0,
            source_counts: [(SourceId(0), primary_count)].into(),
        }
    }
    pub fn selected(&self, id: PageId) -> bool {
        self.selected.contains(&id)
    }
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }
    pub fn current(&self) -> PageId {
        self.current
    }
    pub fn current_index(&self) -> usize {
        self.page_plan
            .position_of(self.current)
            .expect("active current")
    }
    pub fn dirty(&self) -> bool {
        self.dirty
    }
    /// Exact saved-plan identity survives pruning. An older snapshot completion
    /// updates the baseline without discarding edits made while writing.
    pub fn mark_saved(&mut self, entries: Vec<PagePlanEntry>, revision: u64) {
        self.saved_plan_fingerprint = plan_fingerprint(&entries);
        self.baseline = entries;
        self.saved_revision = revision;
        self.dirty = self.page_plan.entries() != self.baseline;
    }
    pub fn saved_fingerprint(&self) -> u64 {
        self.saved_plan_fingerprint
    }
    pub fn selected_entries(&self) -> Result<Vec<PagePlanEntry>, EditError> {
        self.page_plan.validate_ids(&self.selected)?;
        Ok(self
            .page_plan
            .entries()
            .iter()
            .filter(|e| self.selected(e.id))
            .copied()
            .collect())
    }
    pub fn register_source(&mut self, id: SourceId, count: u32) {
        self.source_counts.insert(id, count);
    }
    pub fn duplicate_selected(&mut self) -> Result<bool, EditError> {
        self.page_plan.validate_ids(&self.selected)?;
        let before = self.snapshot();
        let mut entries = Vec::with_capacity(before.entries.len() + self.selected.len());
        let mut duplicates = HashSet::with_capacity(self.selected.len());
        let mut anchor = None;
        for entry in &before.entries {
            entries.push(*entry);
            if self.selected(entry.id) {
                let copy = PagePlanEntry {
                    id: PageId::new(),
                    ..*entry
                };
                anchor = anchor.or(Some(copy.id));
                duplicates.insert(copy.id);
                entries.push(copy);
            }
        }
        self.page_plan.replace(entries);
        self.selected = duplicates;
        self.anchor = anchor;
        Ok(self.commit(before))
    }
    /// Explicit zero-based insertion boundary. Validate the whole source list
    /// before allocating identities or changing selection/history.
    pub fn insert_pages(
        &mut self,
        source: SourceId,
        pages: &[u32],
        boundary: usize,
    ) -> Result<bool, EditError> {
        let count = self
            .source_counts
            .get(&source)
            .ok_or(EditError::InvalidSource)?;
        if pages.is_empty() || pages.iter().any(|p| p >= count) {
            return Err(EditError::InvalidSource);
        }
        if boundary > self.page_plan.entries().len() {
            return Err(EditError::InvalidDestination);
        }
        let before = self.snapshot();
        let inserted: Vec<_> = pages
            .iter()
            .map(|index| PagePlanEntry {
                id: PageId::new(),
                source_id: source,
                source_index: *index,
                rotation: 0,
            })
            .collect();
        self.selected = inserted.iter().map(|e| e.id).collect();
        self.anchor = Some(inserted[0].id);
        let mut entries = before.entries.clone();
        entries.splice(boundary..boundary, inserted);
        self.page_plan.replace(entries);
        Ok(self.commit(before))
    }
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }
    pub fn history_bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Command::bytes).sum()
    }
    pub fn configure_history(&mut self, count: usize, bytes: usize) {
        self.limit = count;
        self.byte_limit = bytes;
        self.trim();
    }
    fn trim(&mut self) {
        while self.undo.len() + self.redo.len() > self.limit
            || self.history_bytes() > self.byte_limit
        {
            if self.undo.pop_front().is_none() {
                self.redo.pop_front();
            }
        }
    }
    pub fn set_current(&mut self, id: PageId) -> Result<(), EditError> {
        if self.page_plan.position_of(id).is_none() {
            return Err(EditError::StalePage);
        }
        self.current = id;
        Ok(())
    }
    /// All click modes navigate to the clicked PageId. Shift replaces the selection;
    /// Ctrl+Shift has range precedence. Missing anchor falls back to the clicked page.
    pub fn select(&mut self, id: PageId, mode: SelectionMode) -> Result<(), EditError> {
        let index = self.page_plan.position_of(id).ok_or(EditError::StalePage)?;
        match mode {
            SelectionMode::Plain => {
                self.selected.clear();
                self.selected.insert(id);
                self.anchor = Some(id);
            }
            SelectionMode::Toggle => {
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.anchor = Some(id);
            }
            SelectionMode::Range => {
                let anchor = self
                    .anchor
                    .and_then(|a| self.page_plan.position_of(a))
                    .unwrap_or_else(|| {
                        self.anchor = Some(id);
                        index
                    });
                self.selected = self.page_plan.entries()[index.min(anchor)..=index.max(anchor)]
                    .iter()
                    .map(|e| e.id)
                    .collect();
            }
        }
        self.current = id;
        self.selection_revision += 1;
        self.assert_invariants();
        Ok(())
    }
    pub fn select_all(&mut self) {
        self.selection_revision += 1;
        self.selected = self.page_plan.entries().iter().map(|e| e.id).collect();
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            entries: self.page_plan.entries().to_vec(),
            selected: self.selected.clone(),
            anchor: self.anchor,
            current: self.current,
        }
    }
    fn restore(&mut self, s: &Snapshot) {
        self.page_plan.replace(s.entries.clone());
        self.selected = s.selected.clone();
        self.anchor = s.anchor;
        self.current = s.current;
        self.dirty = self.page_plan.entries() != self.baseline;
        self.selection_revision += 1;
        self.revision += 1;
        self.assert_invariants();
    }
    fn commit(&mut self, before: Snapshot) -> bool {
        if before.entries == self.page_plan.entries() {
            return false;
        }
        self.undo.push_back(Command {
            before,
            after: self.snapshot(),
        });
        self.redo.clear();
        self.dirty = self.page_plan.entries() != self.baseline;
        self.selection_revision += 1;
        self.revision += 1;
        self.trim();
        self.assert_invariants();
        true
    }
    pub fn delete_selected(&mut self) -> Result<bool, EditError> {
        let before = self.snapshot();
        let index = self.current_index();
        self.page_plan.delete_pages(&self.selected)?;
        if self.page_plan.position_of(self.current).is_none() {
            self.current = self
                .page_plan
                .get(index.min(self.page_plan.entries().len() - 1) as u32)
                .unwrap()
                .id;
        }
        self.selected.clear();
        if self
            .anchor
            .is_some_and(|a| self.page_plan.position_of(a).is_none())
        {
            self.anchor = None;
        }
        Ok(self.commit(before))
    }
    pub fn move_selected(&mut self, boundary: usize) -> Result<bool, EditError> {
        let before = self.snapshot();
        self.page_plan.move_pages(&self.selected, boundary)?;
        Ok(self.commit(before))
    }
    pub fn rotate_selected(&mut self, degrees: i32) -> Result<bool, EditError> {
        let before = self.snapshot();
        self.page_plan.rotate_pages(&self.selected, degrees)?;
        Ok(self.commit(before))
    }
    pub fn undo(&mut self) -> Result<bool, EditError> {
        let c = self.undo.pop_back().ok_or(EditError::NoHistory)?;
        self.restore(&c.before);
        self.redo.push_back(c);
        Ok(true)
    }
    pub fn redo(&mut self) -> Result<bool, EditError> {
        let c = self.redo.pop_back().ok_or(EditError::NoHistory)?;
        self.restore(&c.after);
        self.undo.push_back(c);
        Ok(true)
    }
    pub fn assert_invariants(&self) {
        debug_assert!(!self.page_plan.entries().is_empty());
        debug_assert_eq!(
            self.page_plan
                .entries()
                .iter()
                .map(|e| e.id)
                .collect::<HashSet<_>>()
                .len(),
            self.page_plan.entries().len()
        );
        debug_assert!(self.page_plan.entries().iter().all(|e| e.rotation < 360
            && e.rotation.is_multiple_of(90)
            && self
                .source_counts
                .get(&e.source_id)
                .is_some_and(|count| e.source_index < *count)));
        debug_assert!(self.page_plan.position_of(self.current).is_some());
        debug_assert!(self
            .selected
            .iter()
            .all(|id| self.page_plan.position_of(*id).is_some()));
        debug_assert!(self
            .anchor
            .is_none_or(|id| self.page_plan.position_of(id).is_some()));
    }
}

/// Development identity only; exact entry equality remains the dirty oracle.
pub fn plan_fingerprint(entries: &[PagePlanEntry]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for entry in entries {
        for value in [
            entry.id.0,
            entry.source_id.0,
            u64::from(entry.source_index),
            u64::from(entry.rotation),
        ] {
            for byte in value.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{DocumentLayout, DocumentPoint, DocumentViewport};
    use crate::thumbnails::{ThumbnailLayout, ThumbnailNavigator};
    use crate::viewport::DeviceSize;
    use crate::{DocumentId, PageGeometry, PageSize};
    fn editor(n: u32) -> Editor {
        Editor::new(PagePlan::from_original_pages(n))
    }
    fn ids(e: &Editor) -> Vec<PageId> {
        e.page_plan.entries().iter().map(|p| p.id).collect()
    }
    fn select(e: &mut Editor, pages: &[PageId]) {
        for (i, id) in pages.iter().enumerate() {
            e.select(
                *id,
                if i == 0 {
                    SelectionMode::Plain
                } else {
                    SelectionMode::Toggle
                },
            )
            .unwrap();
        }
    }
    fn view() -> DocumentViewport {
        DocumentViewport {
            origin: DocumentPoint::default(),
            extent: DeviceSize {
                width: 800.0,
                height: 600.0,
            },
            scale: 1.0,
            device_pixel_ratio: 1.0,
            rotation_degrees: 0,
            page_gap: 24.0,
            generation: 1,
        }
    }
    #[test]
    fn selection_modes_anchor_current_independence_and_stale_atomicity() {
        let mut e = editor(8);
        let p = ids(&e);
        e.select(p[2], SelectionMode::Plain).unwrap();
        e.select(p[5], SelectionMode::Range).unwrap();
        assert_eq!(e.selected_count(), 4);
        e.select(p[3], SelectionMode::Toggle).unwrap();
        assert!(!e.selected(p[3]));
        assert_eq!(e.selected_count(), 3);
        e.set_current(p[7]).unwrap();
        assert_eq!(e.selected_count(), 3);
        e.select(p[3], SelectionMode::Toggle).unwrap();
        assert!(e.selected(p[3]));
        e.select(p[1], SelectionMode::Plain).unwrap();
        assert_eq!(e.selected_count(), 1);
        let before = e.snapshot();
        assert_eq!(
            e.select(PageId(0), SelectionMode::Range),
            Err(EditError::StalePage)
        );
        assert_eq!(e.selected, before.selected);
        assert_eq!(e.current(), before.current);
        e.delete_selected().unwrap();
        assert_eq!(e.anchor, None);
        assert_eq!(e.selected_count(), 0);
        e.select(p[6], SelectionMode::Range).unwrap();
        assert_eq!(e.selected_count(), 1);
        assert_eq!(e.anchor, Some(p[6]));
    }
    #[test]
    fn delete_edges_groups_recovery_and_stable_undo_redo() {
        for indices in [
            vec![0],
            vec![5],
            vec![2],
            vec![1, 2, 3],
            vec![0, 2, 4],
            vec![0, 1, 2, 3, 4],
        ] {
            let mut e = editor(6);
            let original = ids(&e);
            let selected: Vec<_> = indices.iter().map(|i| original[*i]).collect();
            select(&mut e, &selected);
            let current_index = *indices.last().unwrap();
            let expected: Vec<_> = original
                .iter()
                .enumerate()
                .filter(|(i, _)| !indices.contains(i))
                .map(|(_, id)| *id)
                .collect();
            e.delete_selected().unwrap();
            assert_eq!(ids(&e), expected);
            assert_eq!(e.selected_count(), 0);
            assert_eq!(e.current(), expected[current_index.min(expected.len() - 1)]);
            assert!(e.dirty());
            assert_eq!(e.undo_depth(), 1);
            e.undo().unwrap();
            assert_eq!(ids(&e), original);
            assert_eq!(e.selected_count(), selected.len());
            assert!(!e.dirty());
            e.redo().unwrap();
            assert_eq!(ids(&e), expected);
            assert!(e.dirty());
            e.assert_invariants();
        }
        let mut e = editor(1);
        let p = ids(&e);
        select(&mut e, &p);
        assert_eq!(e.delete_selected(), Err(EditError::LastPage));
        assert_eq!(ids(&e), p);
        assert!(!e.dirty());
        let mut e = editor(3);
        e.select_all();
        let p = ids(&e);
        assert_eq!(e.delete_selected(), Err(EditError::LastPage));
        assert_eq!(ids(&e), p);
        assert_eq!(e.undo_depth(), 0);
    }
    #[test]
    fn current_survives_deleting_other_pages() {
        let mut e = editor(5);
        let p = ids(&e);
        select(&mut e, &[p[0], p[1]]);
        e.set_current(p[3]).unwrap();
        e.delete_selected().unwrap();
        assert_eq!(e.current(), p[3]);
        assert_eq!(e.current_index(), 1);
    }
    #[test]
    fn moves_normalize_original_boundaries_preserve_group_and_current() {
        for (selected, boundary, expected) in [
            (vec![1], 6, vec![0, 2, 3, 4, 5, 1]),
            (vec![4], 0, vec![4, 0, 1, 2, 3, 5]),
            (vec![1, 2], 5, vec![0, 3, 4, 1, 2, 5]),
            (vec![3, 4], 1, vec![0, 3, 4, 1, 2, 5]),
            (vec![1, 3, 4], 6, vec![0, 2, 5, 1, 3, 4]),
            (vec![1, 3, 4], 3, vec![0, 2, 1, 3, 4, 5]),
            (vec![1, 2, 3], 2, vec![0, 1, 2, 3, 4, 5]),
        ] {
            let mut e = editor(6);
            let p = ids(&e);
            select(&mut e, &selected.iter().map(|i| p[*i]).collect::<Vec<_>>());
            let current = e.current();
            let changed = e.move_selected(boundary).unwrap();
            assert_eq!(ids(&e), expected.iter().map(|i| p[*i]).collect::<Vec<_>>());
            assert_eq!(e.current(), current);
            assert_eq!(e.selected_count(), selected.len());
            if changed {
                e.undo().unwrap();
                assert_eq!(ids(&e), p);
                e.redo().unwrap();
            } else {
                assert_eq!(e.undo_depth(), 0);
                assert!(!e.dirty());
            }
        }
    }
    #[test]
    fn shift_ranges_follow_new_order_and_survive_slot_recycling() {
        let mut e = editor(100);
        let p = ids(&e);
        select(&mut e, &[p[3], p[5]]);
        e.move_selected(0).unwrap();
        e.select(p[5], SelectionMode::Range).unwrap();
        assert_eq!(e.selected_count(), 1);
        e.select(p[3], SelectionMode::Range).unwrap();
        assert_eq!(e.selected_count(), 2);
        let mut nav = ThumbnailNavigator::default();
        for offset in [0.0, 10_000.0, 0.0] {
            nav.update(
                &e.page_plan,
                DocumentId(1),
                ThumbnailLayout::default(),
                offset,
                600.0,
                1.0,
                0,
                e.current_index() as u32,
            )
            .unwrap();
            assert!(nav.slots.len() <= 8);
        }
        assert!(
            nav.slots
                .iter()
                .filter(|s| e.selected(s.key.page_id))
                .count()
                == 2
        );
    }
    #[test]
    fn rotations_layout_intrinsic_separation_and_pixel_identity() {
        let mut e = editor(3);
        let p = ids(&e);
        select(&mut e, &[p[0], p[1]]);
        let mut layout = DocumentLayout::new(&e.page_plan);
        layout
            .resolve(
                0,
                PageGeometry {
                    size: PageSize {
                        width: 800.0,
                        height: 600.0,
                    },
                    rotation_degrees: 90,
                },
            )
            .unwrap();
        let mut nav = ThumbnailNavigator::default();
        let update = |nav: &mut ThumbnailNavigator, e: &Editor| {
            nav.update(
                &e.page_plan,
                DocumentId(1),
                ThumbnailLayout::default(),
                0.0,
                600.0,
                1.0,
                0,
                0,
            )
            .unwrap()
        };
        update(&mut nav, &e);
        let key = nav.slots[0].key;
        e.rotate_selected(90).unwrap();
        layout.sync_plan(&e.page_plan);
        update(&mut nav, &e);
        let page = layout.page(0, view()).unwrap();
        assert_eq!(
            page.bounds.size,
            PageSize {
                width: 600.0,
                height: 800.0
            }
        );
        assert_eq!(page.effective_rotation, 180);
        assert_eq!(page.viewer_rotation, 90);
        assert_ne!(nav.slots[0].key, key);
        let tiles = layout.demand(DocumentId(1), view(), 128).unwrap();
        assert!(tiles
            .iter()
            .filter(|t| t.key.page_id == p[0])
            .all(|t| t.key.rotation_degrees == 90));
        assert_eq!(
            layout
                .page(
                    0,
                    DocumentViewport {
                        rotation_degrees: 90,
                        ..view()
                    }
                )
                .unwrap()
                .bounds
                .size,
            PageSize {
                width: 800.0,
                height: 600.0
            }
        );
        e.rotate_selected(-90).unwrap();
        assert!(!e.dirty());
        e.undo().unwrap();
        assert!(e.dirty());
        e.undo().unwrap();
        assert!(!e.dirty());
        e.redo().unwrap();
        assert_eq!(e.page_plan.get(1).unwrap().rotation, 90);
        e.rotate_selected(270).unwrap();
        assert_eq!(e.page_plan.get(1).unwrap().rotation, 0);
        layout.sync_plan(&e.page_plan);
        assert_eq!(layout.len(), e.page_plan.entries().len());
        select(&mut e, &[p[0]]);
        e.move_selected(3).unwrap();
        layout.sync_plan(&e.page_plan);
        update(&mut nav, &e);
        assert_eq!(layout.page(2, view()).unwrap().page_id, p[0]);
        assert!(layout.geometry_known(2));
        assert_eq!(nav.slots[2].key, key);
        assert_eq!(nav.slots[2].index, 2);
        e.delete_selected().unwrap();
        layout.sync_plan(&e.page_plan);
        e.undo().unwrap();
        layout.sync_plan(&e.page_plan);
        assert!(layout.geometry_known(2));
    }
    #[test]
    fn validation_is_transactional_and_new_edit_clears_redo() {
        let mut e = editor(5);
        let p = ids(&e);
        assert_eq!(e.delete_selected(), Err(EditError::NoSelection));
        assert_eq!(e.undo(), Err(EditError::NoHistory));
        select(&mut e, &[p[1], p[2]]);
        assert_eq!(e.move_selected(6), Err(EditError::InvalidDestination));
        assert_eq!(e.rotate_selected(45), Err(EditError::InvalidRotation));
        assert_eq!(ids(&e), p);
        assert!(!e.dirty());
        assert_eq!(e.undo_depth(), 0);
        e.rotate_selected(90).unwrap();
        assert_eq!(e.undo_depth(), 1);
        e.undo().unwrap();
        assert_eq!(e.redo_depth(), 1);
        e.move_selected(5).unwrap();
        assert_eq!(e.redo(), Err(EditError::NoHistory));
        let invalid = HashSet::from([p[0], PageId(0)]);
        let before = e.page_plan.entries().to_vec();
        assert_eq!(
            e.page_plan.delete_pages(&invalid),
            Err(EditError::StalePage)
        );
        assert_eq!(e.page_plan.entries(), before);
        assert_eq!(
            e.page_plan.move_pages(&invalid, 0),
            Err(EditError::StalePage)
        );
        assert_eq!(
            e.page_plan.rotate_pages(&invalid, 90),
            Err(EditError::StalePage)
        );
    }
    #[test]
    fn history_count_memory_bounds_and_exact_baseline() {
        let mut e = editor(10);
        let p = ids(&e);
        select(&mut e, &[p[0]]);
        e.configure_history(3, 1_000_000);
        for _ in 0..8 {
            e.rotate_selected(90).unwrap();
            assert!(e.undo_depth() <= 3);
        }
        assert!(!e.dirty()); // exact baseline works even when earliest commands are evicted
        for _ in 0..3 {
            e.undo().unwrap();
        }
        assert_eq!(e.undo(), Err(EditError::NoHistory));
        assert_eq!(e.redo_depth(), 3);
        e.configure_history(100, 1);
        assert_eq!(e.history_bytes(), 0);
        e.rotate_selected(-90).unwrap();
        assert_eq!(e.undo_depth(), 0);
        assert!(!e.dirty());
    }
    #[test]
    fn ten_thousand_pages_editing_and_logical_numbers() {
        let start = std::time::Instant::now();
        let mut e = editor(10_000);
        let p = ids(&e);
        e.select(p[1], SelectionMode::Plain).unwrap();
        e.select(p[4999], SelectionMode::Plain).unwrap();
        e.select(p[6999], SelectionMode::Range).unwrap();
        assert_eq!(e.selected_count(), 2001);
        e.set_current(p[5999]).unwrap();
        e.delete_selected().unwrap();
        assert_eq!(e.page_plan.entries().len(), 7999);
        assert_eq!(e.current(), e.page_plan.get(5999).unwrap().id);
        e.undo().unwrap();
        assert_eq!(ids(&e), p);
        select(&mut e, &[p[9996], p[9998], p[9999]]);
        e.move_selected(0).unwrap();
        assert_eq!(e.page_plan.get(0).unwrap().id, p[9996]);
        e.select(p[4999], SelectionMode::Range).unwrap();
        e.rotate_selected(-90).unwrap();
        assert_eq!(e.undo_depth(), 2);
        e.undo().unwrap();
        e.undo().unwrap();
        assert_eq!(ids(&e), p);
        assert!(!e.dirty());
        e.redo().unwrap();
        e.redo().unwrap();
        let mut layout = DocumentLayout::new(&e.page_plan);
        layout.sync_plan(&e.page_plan);
        assert_eq!(layout.len(), 10_000);
        for (i, id) in ids(&e).iter().enumerate() {
            assert_eq!(e.page_plan.position_of(*id), Some(i));
            assert_eq!(layout.page(i, view()).unwrap().index as usize, i);
        }
        e.assert_invariants();
        eprintln!(
            "10k selection/delete/move/rotate/history/layout: {:?}; history {} bytes",
            start.elapsed(),
            e.history_bytes()
        );
    }
}
