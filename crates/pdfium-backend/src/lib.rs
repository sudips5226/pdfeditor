//! Persistent PDFium document backend. PDFium handles never leave this crate.

use document_core::{
    DocumentSource, PageGeometry, PagePoint, PageSize, SourceLocation, TileBuffer, TileRequest,
    TileRequestError, TILE_SIZE,
};
use libloading::Library;
use std::env;
use std::ffi::{c_char, c_int, c_void, CString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Mutex, OnceLock};

type PdfiumDocumentHandle = *mut c_void;
type PdfiumPageHandle = *mut c_void;
type PdfiumBitmapHandle = *mut c_void;

type InitLibraryFn = unsafe extern "system" fn();
type DestroyLibraryFn = unsafe extern "system" fn();
type LoadDocumentFn =
    unsafe extern "system" fn(*const c_char, *const c_char) -> PdfiumDocumentHandle;
type CloseDocumentFn = unsafe extern "system" fn(PdfiumDocumentHandle);
type GetPageCountFn = unsafe extern "system" fn(PdfiumDocumentHandle) -> c_int;
type LoadPageFn = unsafe extern "system" fn(PdfiumDocumentHandle, c_int) -> PdfiumPageHandle;
type ClosePageFn = unsafe extern "system" fn(PdfiumPageHandle);
type GetPageWidthFn = unsafe extern "system" fn(PdfiumPageHandle) -> f64;
type GetPageHeightFn = unsafe extern "system" fn(PdfiumPageHandle) -> f64;
type GetPageRotationFn = unsafe extern "system" fn(PdfiumPageHandle) -> c_int;
#[repr(C)]
struct FsSize {
    width: f32,
    height: f32,
}
type GetPageSizeFn = unsafe extern "system" fn(PdfiumDocumentHandle, c_int, *mut FsSize) -> c_int;
type BitmapCreateExFn =
    unsafe extern "system" fn(c_int, c_int, c_int, *mut c_void, c_int) -> PdfiumBitmapHandle;
type BitmapDestroyFn = unsafe extern "system" fn(PdfiumBitmapHandle);
type RenderPageBitmapFn = unsafe extern "system" fn(
    PdfiumBitmapHandle,
    PdfiumPageHandle,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
);

const FPDF_BITMAP_BGRA: c_int = 4;

#[repr(C)]
struct FsMatrix {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}
#[repr(C)]
struct FsRect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}
type RenderPageMatrixFn = unsafe extern "system" fn(
    PdfiumBitmapHandle,
    PdfiumPageHandle,
    *const FsMatrix,
    *const FsRect,
    c_int,
);

#[derive(Debug)]
pub enum PdfiumError {
    LoadLibrary(String),
    MissingSymbol(&'static str, String),
    InvalidPath,
    LoadDocument,
    InvalidPageIndex(u32),
    LoadPage(u32),
    InvalidPageGeometry,
    CreateBitmap,
    InvalidTileRequest(TileRequestError),
}

impl fmt::Display for PdfiumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoadLibrary(message) => write!(f, "failed to load PDFium: {message}"),
            Self::MissingSymbol(name, message) => {
                write!(f, "missing PDFium symbol {name}: {message}")
            }
            Self::InvalidPath => write!(f, "PDF path cannot be represented for PDFium loading"),
            Self::LoadDocument => write!(f, "PDFium could not open the PDF document"),
            Self::InvalidPageIndex(index) => {
                write!(f, "page index {index} is outside the document")
            }
            Self::LoadPage(index) => write!(f, "PDFium could not load page {index}"),
            Self::InvalidPageGeometry => write!(f, "PDFium returned invalid page geometry"),
            Self::CreateBitmap => write!(f, "PDFium could not create an external BGRA bitmap"),
            Self::InvalidTileRequest(error) => write!(f, "invalid tile request: {error:?}"),
        }
    }
}

impl std::error::Error for PdfiumError {}

struct PdfiumApi {
    _library: Library,
    destroy_library: DestroyLibraryFn,
    load_document: LoadDocumentFn,
    close_document: CloseDocumentFn,
    get_page_count: GetPageCountFn,
    load_page: LoadPageFn,
    close_page: ClosePageFn,
    get_page_width: GetPageWidthFn,
    get_page_height: GetPageHeightFn,
    get_page_rotation: GetPageRotationFn,
    get_page_size: GetPageSizeFn,
    bitmap_create_ex: BitmapCreateExFn,
    bitmap_destroy: BitmapDestroyFn,
    render_page_bitmap: RenderPageBitmapFn,
    render_page_matrix: RenderPageMatrixFn,
}

struct RuntimeState {
    api: Option<PdfiumApi>,
    documents: usize,
}

static RUNTIME: OnceLock<Mutex<RuntimeState>> = OnceLock::new();

fn runtime() -> &'static Mutex<RuntimeState> {
    RUNTIME.get_or_init(|| {
        Mutex::new(RuntimeState {
            api: None,
            documents: 0,
        })
    })
}

impl PdfiumApi {
    fn library_path() -> PathBuf {
        env::var_os("PDFEDITOR_PDFIUM_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("pdfium.dll"))
    }

    fn load() -> Result<Self, PdfiumError> {
        let library_path = Self::library_path();
        let library = unsafe { Library::new(&library_path) }
            .map_err(|error| PdfiumError::LoadLibrary(error.to_string()))?;

        let init_library: InitLibraryFn =
            load_symbol(&library, b"FPDF_InitLibrary\0", "FPDF_InitLibrary")?;
        let destroy_library =
            load_symbol(&library, b"FPDF_DestroyLibrary\0", "FPDF_DestroyLibrary")?;
        let load_document = load_symbol(&library, b"FPDF_LoadDocument\0", "FPDF_LoadDocument")?;
        let close_document = load_symbol(&library, b"FPDF_CloseDocument\0", "FPDF_CloseDocument")?;
        let get_page_count = load_symbol(&library, b"FPDF_GetPageCount\0", "FPDF_GetPageCount")?;
        let load_page = load_symbol(&library, b"FPDF_LoadPage\0", "FPDF_LoadPage")?;
        let close_page = load_symbol(&library, b"FPDF_ClosePage\0", "FPDF_ClosePage")?;
        let get_page_width = load_symbol(&library, b"FPDF_GetPageWidth\0", "FPDF_GetPageWidth")?;
        let get_page_height = load_symbol(&library, b"FPDF_GetPageHeight\0", "FPDF_GetPageHeight")?;
        let get_page_rotation =
            load_symbol(&library, b"FPDFPage_GetRotation\0", "FPDFPage_GetRotation")?;
        let get_page_size = load_symbol(
            &library,
            b"FPDF_GetPageSizeByIndexF\0",
            "FPDF_GetPageSizeByIndexF",
        )?;
        let bitmap_create_ex =
            load_symbol(&library, b"FPDFBitmap_CreateEx\0", "FPDFBitmap_CreateEx")?;
        let bitmap_destroy = load_symbol(&library, b"FPDFBitmap_Destroy\0", "FPDFBitmap_Destroy")?;
        let render_page_bitmap = load_symbol(
            &library,
            b"FPDF_RenderPageBitmap\0",
            "FPDF_RenderPageBitmap",
        )?;
        let render_page_matrix = load_symbol(
            &library,
            b"FPDF_RenderPageBitmapWithMatrix\0",
            "FPDF_RenderPageBitmapWithMatrix",
        )?;

        unsafe {
            init_library();
        }

        Ok(Self {
            _library: library,
            destroy_library,
            load_document,
            close_document,
            get_page_count,
            load_page,
            close_page,
            get_page_width,
            get_page_height,
            get_page_rotation,
            get_page_size,
            bitmap_create_ex,
            bitmap_destroy,
            render_page_bitmap,
            render_page_matrix,
        })
    }
}

impl Drop for PdfiumApi {
    fn drop(&mut self) {
        unsafe {
            (self.destroy_library)();
        }
    }
}

fn load_symbol<T: Copy>(
    library: &Library,
    bytes: &[u8],
    name: &'static str,
) -> Result<T, PdfiumError> {
    unsafe { library.get::<T>(bytes) }
        .map(|symbol| *symbol)
        .map_err(|error| PdfiumError::MissingSymbol(name, error.to_string()))
}

/// Owns one opened PDFium document. Calls and destruction are serialized with
/// the shared PDFium runtime lock; the library is destroyed after the last close.
pub struct PdfiumDocument {
    handle: PdfiumDocumentHandle,
}

// PDFium access to this pointer is always guarded by the process-wide mutex.
unsafe impl Send for PdfiumDocument {}
unsafe impl Sync for PdfiumDocument {}

impl Drop for PdfiumDocument {
    fn drop(&mut self) {
        let mut state = runtime()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let api = state
            .api
            .as_ref()
            .expect("live document has PDFium runtime");
        unsafe {
            (api.close_document)(self.handle);
        }
        state.documents -= 1;
        if state.documents == 0 {
            state.api.take();
        }
    }
}

struct PageGuard<'a> {
    api: &'a PdfiumApi,
    handle: PdfiumPageHandle,
}

impl Drop for PageGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.close_page)(self.handle);
        }
    }
}

struct BitmapGuard<'a> {
    api: &'a PdfiumApi,
    handle: PdfiumBitmapHandle,
}

impl Drop for BitmapGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.bitmap_destroy)(self.handle);
        }
    }
}

impl PdfiumDocument {
    pub fn open(source: &dyn DocumentSource) -> Result<Self, PdfiumError> {
        let SourceLocation::LocalFile(path) = source.location();
        let path_c = pdfium_path(path)?;
        let mut state = runtime()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.api.is_none() {
            state.api = Some(PdfiumApi::load()?);
        }
        let api = state.api.as_ref().expect("runtime initialized");
        let handle = unsafe { (api.load_document)(path_c.as_ptr(), ptr::null()) };
        if handle.is_null() {
            if state.documents == 0 {
                state.api.take();
            }
            return Err(PdfiumError::LoadDocument);
        }
        state.documents += 1;
        Ok(Self { handle })
    }

    pub fn page_count(&self) -> u32 {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let api = state
            .api
            .as_ref()
            .expect("live document has PDFium runtime");
        unsafe { (api.get_page_count)(self.handle).max(0) as u32 }
    }

    pub fn page_geometry(&self, page_index: u32) -> Result<PageGeometry, PdfiumError> {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let api = state
            .api
            .as_ref()
            .expect("live document has PDFium runtime");
        let page = self.load_page(api, page_index)?;
        let width = unsafe { (api.get_page_width)(page.handle) };
        let height = unsafe { (api.get_page_height)(page.handle) };
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(PdfiumError::InvalidPageGeometry);
        }
        let quarter_turns = unsafe { (api.get_page_rotation)(page.handle) };
        if !(0..=3).contains(&quarter_turns) {
            return Err(PdfiumError::InvalidPageGeometry);
        }
        Ok(PageGeometry {
            size: PageSize { width, height },
            rotation_degrees: quarter_turns as u16 * 90,
        })
    }

    /// Narrow size metadata query: no FPDF_LoadPage/content parsing. Dimensions
    /// are normalized by PDFium (crop/intrinsic rotation already applied).
    /// Intrinsic rotation itself needs the separate loaded-page query.
    pub fn page_size_by_index(&self, page_index: u32) -> Result<PageSize, PdfiumError> {
        let state = runtime().lock().unwrap_or_else(|p| p.into_inner());
        let api = state
            .api
            .as_ref()
            .expect("live document has PDFium runtime");
        if page_index > c_int::MAX as u32 {
            return Err(PdfiumError::InvalidPageIndex(page_index));
        }
        let mut size = FsSize {
            width: 0.0,
            height: 0.0,
        };
        if unsafe { (api.get_page_size)(self.handle, page_index as c_int, &mut size) } == 0 {
            return Err(PdfiumError::InvalidPageIndex(page_index));
        }
        if !size.width.is_finite()
            || !size.height.is_finite()
            || size.width <= 0.0
            || size.height <= 0.0
        {
            return Err(PdfiumError::InvalidPageGeometry);
        }
        Ok(PageSize {
            width: f64::from(size.width),
            height: f64::from(size.height),
        })
    }

    fn load_page<'a>(
        &self,
        api: &'a PdfiumApi,
        page_index: u32,
    ) -> Result<PageGuard<'a>, PdfiumError> {
        let count = unsafe { (api.get_page_count)(self.handle) };
        if count <= 0 || page_index >= count as u32 {
            return Err(PdfiumError::InvalidPageIndex(page_index));
        }
        let page = unsafe { (api.load_page)(self.handle, page_index as c_int) };
        if page.is_null() {
            return Err(PdfiumError::LoadPage(page_index));
        }
        Ok(PageGuard { api, handle: page })
    }

    /// The caller resolves request.page_id to this source index. Only the tile
    /// buffer is allocated. PDFium's base display matrix handles PDF y-up,
    /// crop origin and intrinsic rotation; our matrix handles normalized page
    /// space -> additional rotation -> physical scale -> negative grid offset.
    pub fn render_tile(
        &self,
        page_index: u32,
        request: &TileRequest,
    ) -> Result<TileBuffer, PdfiumError> {
        request
            .validate()
            .map_err(PdfiumError::InvalidTileRequest)?;
        let state = runtime()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let api = state
            .api
            .as_ref()
            .expect("live document has PDFium runtime");
        let page = self.load_page(api, page_index)?;
        let size = PageSize {
            width: unsafe { (api.get_page_width)(page.handle) },
            height: unsafe { (api.get_page_height)(page.handle) },
        };
        let transform = request
            .page_to_tile(size)
            .map_err(PdfiumError::InvalidTileRequest)?;
        let p = transform.map(PagePoint { x: 0.0, y: 0.0 });
        let q = transform.map(PagePoint {
            x: size.width,
            y: size.height,
        });
        let clipping = FsRect {
            left: p.x.min(q.x).max(0.0) as f32,
            top: p.y.min(q.y).max(0.0) as f32,
            right: p.x.max(q.x).min(f64::from(request.width)) as f32,
            bottom: p.y.max(q.y).min(f64::from(request.height)) as f32,
        };
        let mut tile = TileBuffer::new_bgra(request.width, request.height);
        tile.pixels.fill(255); // Opaque white, including outside the page.
        if clipping.left >= clipping.right || clipping.top >= clipping.bottom {
            return Ok(tile);
        }
        let matrix = FsMatrix {
            a: transform.a as f32,
            b: transform.b as f32,
            c: transform.c as f32,
            d: transform.d as f32,
            e: transform.e as f32,
            f: transform.f as f32,
        };
        let bitmap = unsafe {
            (api.bitmap_create_ex)(
                request.width as c_int,
                request.height as c_int,
                FPDF_BITMAP_BGRA,
                tile.pixels.as_mut_ptr().cast(),
                tile.stride as c_int,
            )
        };
        if bitmap.is_null() {
            return Err(PdfiumError::CreateBitmap);
        }
        let bitmap = BitmapGuard {
            api,
            handle: bitmap,
        };
        unsafe {
            (api.render_page_matrix)(bitmap.handle, page.handle, &matrix, &clipping, 0);
        }
        Ok(tile)
    }

    /// Legacy P0/P1 compatibility proof; the P2 viewer uses render_tile.
    pub fn render_page_preview(&self, page_index: u32) -> Result<TileBuffer, PdfiumError> {
        let state = runtime()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let api = state
            .api
            .as_ref()
            .expect("live document has PDFium runtime");
        let page = self.load_page(api, page_index)?;

        let page_width = unsafe { (api.get_page_width)(page.handle) };
        let page_height = unsafe { (api.get_page_height)(page.handle) };
        if !page_width.is_finite()
            || !page_height.is_finite()
            || page_width <= 0.0
            || page_height <= 0.0
        {
            return Err(PdfiumError::InvalidPageGeometry);
        }

        let mut tile = TileBuffer::new_bgra(TILE_SIZE, TILE_SIZE);
        tile.pixels.fill(255);

        let bitmap = unsafe {
            (api.bitmap_create_ex)(
                TILE_SIZE as c_int,
                TILE_SIZE as c_int,
                FPDF_BITMAP_BGRA,
                tile.pixels.as_mut_ptr().cast(),
                tile.stride as c_int,
            )
        };
        if bitmap.is_null() {
            return Err(PdfiumError::CreateBitmap);
        }
        let bitmap = BitmapGuard {
            api,
            handle: bitmap,
        };

        let scale = (TILE_SIZE as f64 / page_width).min(TILE_SIZE as f64 / page_height);
        let render_width = (page_width * scale).round().clamp(1.0, TILE_SIZE as f64) as c_int;
        let render_height = (page_height * scale).round().clamp(1.0, TILE_SIZE as f64) as c_int;
        let start_x = (TILE_SIZE as c_int - render_width) / 2;
        let start_y = (TILE_SIZE as c_int - render_height) / 2;

        unsafe {
            (api.render_page_bitmap)(
                bitmap.handle,
                page.handle,
                start_x,
                start_y,
                render_width,
                render_height,
                0,
                0,
            );
        }

        Ok(tile)
    }
}

fn pdfium_path(path: &Path) -> Result<CString, PdfiumError> {
    let path_text = path.to_str().ok_or(PdfiumError::InvalidPath)?;
    CString::new(path_text).map_err(|_| PdfiumError::InvalidPath)
}
