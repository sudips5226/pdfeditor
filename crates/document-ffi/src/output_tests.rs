use super::*;
use crate::editing::pdfeditor_document_select_page;
use std::sync::mpsc;
use std::time::Duration;

struct Handle(*mut PdfeditorDocument);
impl Drop for Handle {
    fn drop(&mut self) {
        pdfeditor_document_close(self.0);
    }
}
fn fixture() -> Option<Handle> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/p5c-primary.pdf")
        .canonicalize()
        .unwrap();
    let path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let mut handle = ptr::null_mut();
    let code = unsafe { pdfeditor_document_open_utf8(path.as_ptr(), &mut handle) };
    if code == PDFEDITOR_ERROR_PDFIUM && std::env::var_os("PDFEDITOR_PDFIUM_PATH").is_none() {
        eprintln!("PDFium unavailable; output integration test needs deployed runtime");
        return None;
    }
    assert_eq!(code, 0);
    Some(Handle(handle))
}
fn directory() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts")
        .join(format!(
            "p5c-unit-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}
fn source_bytes() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/p5c-primary.pdf"),
    )
    .unwrap()
}
struct Fake {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<Option<mpsc::Receiver<()>>>,
    fail_write: bool,
    fail_verify: bool,
}
impl Fake {
    fn plain(fail_write: bool, fail_verify: bool) -> Arc<Self> {
        Arc::new(Self {
            entered: Mutex::new(None),
            release: Mutex::new(None),
            fail_write,
            fail_verify,
        })
    }
    fn gated() -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, event) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        (
            Arc::new(Self {
                entered: Mutex::new(Some(entered)),
                release: Mutex::new(Some(gate)),
                fail_write: false,
                fail_verify: false,
            }),
            event,
            release,
        )
    }
}
impl StructuralBackend for Fake {
    fn write(
        &self,
        snapshot: &ExportSnapshot,
        temp: &Path,
        progress: &mut dyn FnMut(u32, u32),
    ) -> Result<(), String> {
        progress(WRITING, 10);
        if let Some(sender) = self.entered.lock().unwrap().take() {
            sender.send(()).unwrap();
        }
        if let Some(receiver) = self.release.lock().unwrap().take() {
            receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(|e| e.to_string())?;
        }
        // Deterministically record snapshot order, independent of live editor.
        std::fs::write(temp, format!("{:?}", snapshot.entries)).map_err(|e| e.to_string())?;
        if self.fail_write {
            return Err("Injected native write failure".into());
        }
        Ok(())
    }
    fn verify(&self, _: &Path, _: usize) -> Result<(), String> {
        if self.fail_verify {
            Err("Injected verification failure".into())
        } else {
            Ok(())
        }
    }
}
fn start_fake(h: &Handle, path: &Path, backend: Arc<dyn StructuralBackend>) -> Vec<PagePlanEntry> {
    with_document(h.0, |d| {
        let snapshot = capture(d, path, 1, true)?;
        let entries = snapshot.entries.clone();
        d.output.start(
            snapshot,
            Arc::clone(&d.editor),
            Arc::clone(&d.sources),
            Arc::clone(&d.operation),
            backend,
        )?;
        Ok(entries)
    })
    .unwrap()
}
fn finish(h: &Handle) -> PdfeditorOutputStatus {
    // Join without holding the public operation gate or live editor.
    let d = documents()
        .lock()
        .unwrap()
        .get(&(h.0 as usize))
        .cloned()
        .unwrap();
    if let Some(t) = d.output.worker.lock().unwrap().take() {
        t.join().unwrap();
    }
    let mut status = PdfeditorOutputStatus::default();
    assert_eq!(
        unsafe { pdfeditor_document_output_status(h.0, &mut status) },
        0
    );
    status
}
#[test]
fn gated_save_allows_edit_browse_thumbnails_and_restores_saved_baseline() {
    let Some(h) = fixture() else {
        return;
    };
    let dir = directory();
    let path = dir.join("race.pdf");
    let (fake, entered, release) = Fake::gated();
    let snapshot = start_fake(&h, &path, fake);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let deadline = Instant::now();
    let current = with_document(h.0, |d| Ok(d.editor.lock().unwrap().current())).unwrap();
    assert_eq!(pdfeditor_document_select_page(h.0, current.0, 0, 0), 0);
    let mut edited = editing::PdfeditorEditorStatus::default();
    assert_eq!(
        unsafe { editing::pdfeditor_document_edit(h.0, 7, 0, &mut edited) },
        0
    );
    assert_eq!(edited.page_count, 7);
    let view = continuous::PdfeditorDocumentViewport {
        width: 800.,
        height: 600.,
        scale: 1.,
        device_pixel_ratio: 1.,
        page_gap: 24.,
        generation: 1,
        ..Default::default()
    };
    let mut layout = continuous::PdfeditorLayoutSnapshot::default();
    let mut pages = [continuous::PdfeditorPageLayout::default(); 64];
    assert_eq!(
        unsafe {
            continuous::pdfeditor_document_update_continuous_viewport(
                h.0,
                &view,
                &mut layout,
                pages.as_mut_ptr(),
                64,
            )
        },
        0
    );
    let tv = thumbnails::PdfeditorThumbnailViewport {
        extent: 600.,
        width: 144.,
        height: 168.,
        label_height: 24.,
        gap: 8.,
        padding: 8.,
        dpr: 1.,
        overscan: 2,
        generation: 1,
        ..Default::default()
    };
    let mut ts = thumbnails::PdfeditorThumbnailSnapshot::default();
    let mut items = [thumbnails::PdfeditorThumbnailItem::default(); 64];
    assert_eq!(
        unsafe {
            thumbnails::pdfeditor_document_update_thumbnails(
                h.0,
                &tv,
                &mut ts,
                items.as_mut_ptr(),
                64,
            )
        },
        0
    );
    assert!(ts.returned > 0);
    // The independent render scheduler actually completes a thumbnail while
    // native output is gated, proving more than just fast status lock access.
    let mut thumbnail = thumbnails::PdfeditorReadyThumbnail::default();
    let poll_start = Instant::now();
    loop {
        let code =
            unsafe { thumbnails::pdfeditor_document_poll_ready_thumbnail(h.0, &mut thumbnail) };
        if code == 0 {
            assert_eq!(thumbnail.status, 0);
            assert_eq!(viewport::pdfeditor_tile_lease_release(thumbnail.lease), 0);
            break;
        }
        assert_eq!(code, viewport::PDFEDITOR_NO_TILE);
        assert!(poll_start.elapsed() < Duration::from_secs(3));
        std::thread::yield_now();
    }
    assert!(deadline.elapsed() < Duration::from_secs(5));
    let busy = with_document(h.0, |d| {
        d.output.start(
            capture(d, &dir.join("busy.pdf"), 1, false)?,
            Arc::clone(&d.editor),
            Arc::clone(&d.sources),
            Arc::clone(&d.operation),
            Fake::plain(false, false),
        )
    })
    .unwrap_err();
    assert_eq!(busy, ERROR_OUTPUT_BUSY);
    release.send(()).unwrap();
    let status = finish(&h);
    assert_eq!(status.phase, SUCCEEDED);
    assert_eq!(status.snapshot_revision, 0);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{snapshot:?}")
    );
    with_document(h.0, |d| {
        let mut e = d.editor.lock().unwrap();
        assert!(e.dirty());
        assert_eq!(e.saved_revision, 0);
        assert_eq!(e.page_plan.entries().len(), 7);
        e.undo().unwrap();
        assert!(!e.dirty());
        e.redo().unwrap();
        assert!(e.dirty());
        Ok(())
    })
    .unwrap();
}
#[test]
fn failure_injection_preserves_existing_target_baseline_and_worker_reuse() {
    let immutable_source = source_bytes();
    let Some(h) = fixture() else {
        return;
    };
    let dir = directory();
    let path = dir.join("existing.pdf");
    let original = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/p5c-external.pdf"),
    )
    .unwrap();
    std::fs::write(&path, &original).unwrap();
    let baseline =
        with_document(h.0, |d| Ok(d.editor.lock().unwrap().saved_fingerprint())).unwrap();
    for (write, verify) in [(true, false), (false, true)] {
        start_fake(&h, &path, Fake::plain(write, verify));
        let status = finish(&h);
        assert_eq!(status.phase, FAILED);
        assert_eq!(status.temp_exists, 0);
        assert_eq!(status.saved_fingerprint, baseline);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(source_bytes(), immutable_source);
        assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pdfeditor-")));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        start_fake(&h, &path, Fake::plain(false, false));
        let status = finish(&h);
        assert_eq!(status.phase, FAILED);
        assert_eq!(status.temp_exists, 0);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(status.saved_fingerprint, baseline);
        drop(locked);
    }
    let (fake, entered, release) = Fake::gated();
    start_fake(&h, &path, fake);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(pdfeditor_document_output_cancel(h.0), 0);
    release.send(()).unwrap();
    let status = finish(&h);
    assert_eq!(status.phase, CANCELLED);
    assert_eq!(source_bytes(), immutable_source);
    assert_eq!(status.temp_exists, 0);
    assert_eq!(status.saved_fingerprint, baseline);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    start_fake(&h, &path, Fake::plain(false, false));
    assert_eq!(finish(&h).phase, SUCCEEDED);
    assert_ne!(std::fs::read(&path).unwrap(), original);
}
#[test]
fn missing_changed_sources_temp_failure_and_invalid_request_do_not_mutate_editor() {
    let immutable_source = source_bytes();
    let Some(h) = fixture() else {
        return;
    };
    let dir = directory();
    let path = dir.join("target.pdf");
    std::fs::write(&path, b"keep").unwrap();
    with_document(h.0, |d| {
        assert_eq!(capture(d, &path, 1, false).err().unwrap(), ERROR_OVERWRITE);
        assert_eq!(
            capture(d, &dir.join("missing-dir/out.pdf"), 1, false)
                .err()
                .unwrap(),
            ERROR_OUTPUT
        );
        assert_eq!(
            capture(d, &path, 2, true).err().unwrap(),
            editing::PDFEDITOR_ERROR_NO_SELECTION
        );
        let source = d
            .sources
            .records
            .read()
            .unwrap()
            .get(&SourceId(0))
            .unwrap()
            .path
            .clone();
        assert_eq!(
            capture(d, &source, 1, true).err().unwrap(),
            ERROR_SOURCE_TARGET
        );
        for mode in 0..3 {
            let mut snapshot = capture(d, &path, 1, true)?;
            if mode == 0 {
                snapshot.sources[0].path = dir.join("missing.pdf");
            }
            if mode == 1 {
                snapshot.sources[0].identity.size += 1;
            }
            if mode == 2 {
                snapshot.target = dir.join("nonexistent/temp.pdf");
            }
            let shared = Mutex::new(JobState::default());
            assert!(run(
                &snapshot,
                Fake::plain(false, false).as_ref(),
                &shared,
                &d.editor,
                &d.sources,
                &d.operation
            )
            .is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"keep");
            assert_eq!(source_bytes(), immutable_source);
            let e = d.editor.lock().unwrap();
            assert_eq!(e.revision, 0);
            assert!(!e.dirty());
            assert_eq!(e.undo_depth(), 0);
        }
        Ok(())
    })
    .unwrap();
    let bad = std::ffi::CString::new(dir.join("missing.pdf").to_str().unwrap()).unwrap();
    let mut status = editing::PdfeditorEditorStatus::default();
    assert_eq!(
        unsafe {
            pdfeditor_document_insert_source(h.0, bad.as_ptr(), 0, ptr::null(), 0, &mut status)
        },
        ERROR_SOURCE
    );
    assert_eq!(
        pdfeditor_document_select_page(h.0, u64::MAX, 0, 0),
        PDFEDITOR_ERROR_INVALID_PAGE
    );
    assert_eq!(
        unsafe {
            pdfeditor_document_output_status(ptr::null_mut(), &mut PdfeditorOutputStatus::default())
        },
        PDFEDITOR_ERROR_NULL_ARGUMENT
    );
    assert_eq!(std::mem::size_of::<PdfeditorOutputStatus>(), 3200);
}

#[test]
fn insert_output_target_during_gated_save_rejects_finalization() {
    let Some(h) = fixture() else {
        return;
    };
    let dir = directory();
    let path = dir.join("new-active-source.pdf");
    let original = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/p5c-external.pdf"),
    )
    .unwrap();
    std::fs::write(&path, &original).unwrap();
    let (fake, entered, release) = Fake::gated();
    start_fake(&h, &path, fake);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let utf8 = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let mut e = editing::PdfeditorEditorStatus::default();
    assert_eq!(
        unsafe { pdfeditor_document_insert_source(h.0, utf8.as_ptr(), 1, ptr::null(), 0, &mut e) },
        0
    );
    release.send(()).unwrap();
    let output = finish(&h);
    assert_eq!(output.phase, FAILED);
    assert_eq!(output.temp_exists, 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(e.page_count, 9);
    assert_eq!(e.structural_dirty, 1);
    assert_eq!(output.saved_revision, 0);
}

#[test]
fn close_cancels_and_joins_active_output_without_source_lifetime_loss() {
    let Some(h) = fixture() else {
        return;
    };
    let dir = directory();
    let path = dir.join("closed-output.pdf");
    std::fs::write(&path, b"keep").unwrap();
    let (fake, entered, release) = Fake::gated();
    start_fake(&h, &path, fake);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    let token = h.0 as usize;
    let document = documents().lock().unwrap().get(&token).cloned().unwrap();
    let close =
        std::thread::spawn(move || pdfeditor_document_close(token as *mut PdfeditorDocument));
    let start = Instant::now();
    while !document.output.shared.lock().unwrap().cancel {
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::yield_now();
    }
    release.send(()).unwrap();
    assert_eq!(close.join().unwrap(), 0);
    assert_eq!(
        document.output.shared.lock().unwrap().status.phase,
        CANCELLED
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    assert_eq!(
        pdfeditor_document_output_cancel(h.0),
        PDFEDITOR_ERROR_INVALID_HANDLE
    );
}
