//! Minimal PDFium backend used by the P0 architecture proof.
//!
//! PDFium handles and symbols are contained entirely in this crate. They must not
//! leak into document-core or the public C ABI. The P0 entry point renders a
//! single page preview; P1 replaces it with persistent document handles and the
//! production tile request model.

use document_core::{TileBuffer, TILE_SIZE};
use libloading::Library;
use std::env;
use std::ffi::{c_char, c_int, c_void, CString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr;

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
}

impl fmt::Display for PdfiumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoadLibrary(message) => write!(f, "failed to load PDFium: {message}"),
            Self::MissingSymbol(name, message) => {
                write!(f, "missing PDFium symbol {name}: {message}")
            }
            Self::InvalidPath => write!(f, "PDF path cannot be represented for P0 PDFium loading"),
            Self::LoadDocument => write!(f, "PDFium could not open the PDF document"),
            Self::InvalidPageIndex(index) => {
                write!(f, "page index {index} is outside the document")
            }
            Self::LoadPage(index) => write!(f, "PDFium could not load page {index}"),
            Self::InvalidPageGeometry => write!(f, "PDFium returned invalid page geometry"),
            Self::CreateBitmap => write!(f, "PDFium could not create an external BGRA bitmap"),
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
    bitmap_create_ex: BitmapCreateExFn,
    bitmap_destroy: BitmapDestroyFn,
    render_page_bitmap: RenderPageBitmapFn,
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
        let bitmap_create_ex =
            load_symbol(&library, b"FPDFBitmap_CreateEx\0", "FPDFBitmap_CreateEx")?;
        let bitmap_destroy = load_symbol(&library, b"FPDFBitmap_Destroy\0", "FPDFBitmap_Destroy")?;
        let render_page_bitmap = load_symbol(
            &library,
            b"FPDF_RenderPageBitmap\0",
            "FPDF_RenderPageBitmap",
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
            bitmap_create_ex,
            bitmap_destroy,
            render_page_bitmap,
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

struct DocumentGuard<'a> {
    api: &'a PdfiumApi,
    handle: PdfiumDocumentHandle,
}

impl Drop for DocumentGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.close_document)(self.handle);
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

/// Renders one PDF page into a 512x512 BGRA preview tile.
///
/// This P0 function deliberately uses PDFium's path-based loader so PDFium owns
/// file access and the application does not read the entire PDF into RAM.
/// P1 will replace this with a persistent DocumentSource-backed document handle.
pub fn render_page_preview(path: &Path, page_index: u32) -> Result<TileBuffer, PdfiumError> {
    let api = PdfiumApi::load()?;

    let path_text = path.to_string_lossy();
    let path_c = CString::new(path_text.as_bytes()).map_err(|_| PdfiumError::InvalidPath)?;

    let document = unsafe { (api.load_document)(path_c.as_ptr(), ptr::null()) };
    if document.is_null() {
        return Err(PdfiumError::LoadDocument);
    }
    let document = DocumentGuard {
        api: &api,
        handle: document,
    };

    let page_count = unsafe { (api.get_page_count)(document.handle) };
    if page_count <= 0 || page_index >= page_count as u32 {
        return Err(PdfiumError::InvalidPageIndex(page_index));
    }

    let page = unsafe { (api.load_page)(document.handle, page_index as c_int) };
    if page.is_null() {
        return Err(PdfiumError::LoadPage(page_index));
    }
    let page = PageGuard {
        api: &api,
        handle: page,
    };

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
        api: &api,
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

    drop(bitmap);
    drop(page);
    drop(document);

    Ok(tile)
}
