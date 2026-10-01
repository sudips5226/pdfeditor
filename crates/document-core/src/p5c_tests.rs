use crate::editing::{EditError, Editor, SelectionMode};
use crate::{PagePlan, SourceId};
fn editor(count: u32) -> Editor {
    Editor::new(PagePlan::from_original_pages(count))
}
#[test]
fn duplicates_edges_contiguous_noncontiguous_rotated_and_stable_redo() {
    for selected in [vec![0], vec![5], vec![2], vec![1, 2, 3], vec![1, 5]] {
        let mut e = editor(6);
        let original = e.page_plan.entries().to_vec();
        for (n, i) in selected.iter().enumerate() {
            e.select(
                original[*i].id,
                if n == 0 {
                    SelectionMode::Plain
                } else {
                    SelectionMode::Toggle
                },
            )
            .unwrap();
        }
        e.rotate_selected(90).unwrap();
        let before = e.page_plan.entries().to_vec();
        let current = e.current();
        e.duplicate_selected().unwrap();
        let duplicated = e.page_plan.entries().to_vec();
        let mut index = 0;
        for (i, p) in before.iter().enumerate() {
            assert_eq!(duplicated[index], *p);
            index += 1;
            if selected.contains(&i) {
                let copy = duplicated[index];
                assert_ne!(copy.id, p.id);
                assert_eq!(copy.source_ref(), p.source_ref());
                assert_eq!(copy.rotation, 90);
                assert!(e.selected(copy.id));
                index += 1;
            }
        }
        assert_eq!(e.current(), current);
        assert_eq!(e.selected_count(), selected.len());
        assert_eq!(e.undo_depth(), 2);
        e.undo().unwrap();
        assert_eq!(e.page_plan.entries(), before);
        e.redo().unwrap();
        assert_eq!(e.page_plan.entries(), duplicated);
        e.undo().unwrap();
        e.undo().unwrap();
        assert!(!e.dirty());
        e.assert_invariants();
    }
}
#[test]
fn insert_boundaries_lists_duplicate_external_page_and_transactional_errors() {
    for boundary in [0, 2, 4] {
        let mut e = editor(4);
        e.register_source(SourceId(1), 3);
        let original = e.page_plan.entries().to_vec();
        e.insert_pages(SourceId(1), &[2, 0, 2], boundary).unwrap();
        let inserted = e.page_plan.entries().to_vec();
        assert_eq!(e.selected_count(), 3);
        assert_eq!(&inserted[..boundary], &original[..boundary]);
        assert_eq!(inserted[boundary].source_index, 2);
        assert_eq!(inserted[boundary + 1].source_index, 0);
        assert_ne!(inserted[boundary].id, inserted[boundary + 2].id);
        e.undo().unwrap();
        assert_eq!(e.page_plan.entries(), original);
        assert!(!e.dirty());
        e.redo().unwrap();
        assert_eq!(e.page_plan.entries(), inserted);
        e.duplicate_selected().unwrap();
        assert_eq!(e.selected_count(), 3);
        assert!(e
            .selected_entries()
            .unwrap()
            .iter()
            .all(|p| p.source_id == SourceId(1)));
        let before = e.page_plan.entries().to_vec();
        let rev = e.revision;
        for (source, pages, boundary) in [
            (SourceId(9), vec![0], 0),
            (SourceId(1), vec![3], 0),
            (SourceId(1), vec![], 0),
            (SourceId(1), vec![0], 99),
        ] {
            assert!(e.insert_pages(source, &pages, boundary).is_err());
            assert_eq!(e.page_plan.entries(), before);
            assert_eq!(e.revision, rev);
        }
        e.assert_invariants();
    }
}
#[test]
fn extraction_is_in_plan_order_and_saved_snapshot_baseline_survives_history_pruning() {
    let mut e = editor(5);
    let p = e.page_plan.entries().to_vec();
    for (n, i) in [4, 1, 3].into_iter().enumerate() {
        e.select(
            p[i].id,
            if n == 0 {
                SelectionMode::Plain
            } else {
                SelectionMode::Toggle
            },
        )
        .unwrap();
    }
    assert_eq!(
        e.selected_entries()
            .unwrap()
            .iter()
            .map(|p| p.source_index)
            .collect::<Vec<_>>(),
        [1, 3, 4]
    );
    e.move_selected(0).unwrap();
    let saved = e.page_plan.entries().to_vec();
    let r = e.revision;
    e.rotate_selected(90).unwrap();
    e.mark_saved(saved.clone(), r);
    assert!(e.dirty());
    e.undo().unwrap();
    assert!(!e.dirty());
    e.redo().unwrap();
    assert!(e.dirty());
    e.configure_history(0, 0);
    e.rotate_selected(-90).unwrap();
    assert!(!e.dirty());
    assert_eq!(e.page_plan.entries(), saved);
    assert_eq!(e.saved_revision, r);
    let mut empty = editor(1);
    assert_eq!(empty.duplicate_selected(), Err(EditError::NoSelection));
}
#[test]
fn ten_thousand_pages_plus_inserts_snapshot_mapping_measurement() {
    let mut e = editor(10_000);
    e.register_source(SourceId(1), 300);
    let id = e.page_plan.get(4999).unwrap().id;
    e.select(id, SelectionMode::Plain).unwrap();
    e.duplicate_selected().unwrap();
    e.move_selected(0).unwrap();
    e.rotate_selected(90).unwrap();
    e.insert_pages(SourceId(1), &(0..300).collect::<Vec<_>>(), 5000)
        .unwrap();
    let started = std::time::Instant::now();
    let snapshot = e.page_plan.entries().to_vec();
    let mapping: Vec<_> = snapshot
        .iter()
        .map(|e| (e.source_ref(), e.rotation))
        .collect();
    eprintln!(
        "P5C 10k snapshot+mapping: {} us, entries {} bytes, mapping {} bytes",
        started.elapsed().as_micros(),
        snapshot.capacity() * std::mem::size_of_val(&snapshot[0]),
        mapping.capacity() * std::mem::size_of_val(&mapping[0])
    );
    assert_eq!(snapshot.len(), 10_301);
    e.assert_invariants();
}
