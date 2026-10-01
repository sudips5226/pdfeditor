#include "pch.h"
#include "NativeCoreBridge.h"
#include "pdfeditor_ffi.h"

namespace winrt::PdfEditor::implementation
{
    namespace
    {
        [[noreturn]] void ThrowWin32(char const* operation)
        {
            const auto error = ::GetLastError();
            throw std::runtime_error(
                std::string(operation) + " failed with Win32 error " + std::to_string(error));
        }
    }

    std::filesystem::path NativeCoreBridge::ExecutableDirectory()
    {
        std::wstring buffer(32768, L'\0');
        const auto length = ::GetModuleFileNameW(
            nullptr,
            buffer.data(),
            static_cast<DWORD>(buffer.size()));

        if (length == 0 || length >= buffer.size())
        {
            ThrowWin32("GetModuleFileNameW");
        }

        buffer.resize(length);
        return std::filesystem::path(buffer).parent_path();
    }

    std::filesystem::path NativeCoreBridge::CoreDllPath()
    {
        return ExecutableDirectory() / L"pdfeditor_core.dll";
    }

    std::filesystem::path NativeCoreBridge::P0FixturePath()
    {
        return ExecutableDirectory() / L"p0-one-page.pdf";
    }

    std::filesystem::path NativeCoreBridge::P2FixturePath()
    {
        return ExecutableDirectory() / L"p2-tile-regions.pdf";
    }

    NativeCoreBridge::NativeCoreBridge()
    {
        const auto path = CoreDllPath();
        m_module = ::LoadLibraryExW(
            path.c_str(),
            nullptr,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);

        if (m_module == nullptr)
        {
            ThrowWin32("LoadLibraryExW(pdfeditor_core.dll)");
        }

        try
        {
            m_abiVersion = reinterpret_cast<AbiVersionFn>(RequireSymbol("pdfeditor_abi_version"));
            m_renderTestTile =
                reinterpret_cast<RenderTestTileFn>(RequireSymbol("pdfeditor_render_test_tile"));
            m_documentOpen = reinterpret_cast<DocumentOpenFn>(
                RequireSymbol("pdfeditor_document_open_utf8"));
            m_documentClose = reinterpret_cast<DocumentCloseFn>(
                RequireSymbol("pdfeditor_document_close"));
            m_pageCount = reinterpret_cast<PageCountFn>(
                RequireSymbol("pdfeditor_document_page_count"));
            m_pageGeometry = reinterpret_cast<PageGeometryFn>(
                RequireSymbol("pdfeditor_document_page_geometry"));
            m_renderPage = reinterpret_cast<RenderPageFn>(
                RequireSymbol("pdfeditor_document_render_page_preview"));
            m_tileFree = reinterpret_cast<TileFreeFn>(RequireSymbol("pdfeditor_tile_free"));
            if (m_abiVersion() != 3)
            {
                throw std::runtime_error("Native core requires ABI v3");
            }
            m_renderTile = reinterpret_cast<RenderTileFn>(RequireSymbol("pdfeditor_document_render_tile"));
        }
        catch (...)
        {
            ::FreeLibrary(m_module);
            m_module = nullptr;
            throw;
        }
    }

    NativeCoreBridge::~NativeCoreBridge()
    {
        if (m_document != nullptr)
        {
            m_documentClose(m_document);
        }
        if (m_module != nullptr)
        {
            ::FreeLibrary(m_module);
        }
    }

    FARPROC NativeCoreBridge::RequireSymbol(char const* name) const
    {
        const auto symbol = ::GetProcAddress(m_module, name);
        if (symbol == nullptr)
        {
            throw std::runtime_error(std::string("Missing native core export: ") + name);
        }

        return symbol;
    }

    NativeCoreValidation NativeCoreBridge::Validate(
        std::function<void(PdfeditorTile const&)> const& tileConsumer) const
    {
        PdfeditorTile tile{};
        const auto result = m_renderTestTile(&tile);

        if (result != PDFEDITOR_OK)
        {
            throw std::runtime_error(
                "pdfeditor_render_test_tile failed with code " + std::to_string(result));
        }

        return ConsumeTile(tile, tileConsumer);
    }

    PdfeditorPageGeometry NativeCoreBridge::OpenPdf(std::filesystem::path const& pdfPath)
    {
        if (!std::filesystem::is_regular_file(pdfPath))
        {
            throw std::runtime_error("PDF fixture is missing: " + pdfPath.string());
        }

        const auto utf8Path = pdfPath.u8string();
        const std::string pathBytes(
            reinterpret_cast<char const*>(utf8Path.data()), utf8Path.size());
        if (m_document != nullptr && m_documentPath != pdfPath)
        {
            const auto closeResult = m_documentClose(m_document);
            m_document = nullptr;
            if (closeResult != PDFEDITOR_OK)
            {
                throw std::runtime_error("pdfeditor_document_close failed with code " +
                    std::to_string(closeResult));
            }
        }
        if (m_document == nullptr)
        {
            const auto openResult = m_documentOpen(pathBytes.c_str(), &m_document);
            if (openResult != PDFEDITOR_OK)
            {
                throw std::runtime_error("pdfeditor_document_open_utf8 failed with code " +
                    std::to_string(openResult));
            }
            m_documentPath = pdfPath;
        }

        std::uint32_t pageCount{};
        PdfeditorPageGeometry geometry{};
        if (m_pageCount(m_document, &pageCount) != PDFEDITOR_OK || pageCount == 0 ||
            m_pageGeometry(m_document, 0, &geometry) != PDFEDITOR_OK ||
            geometry.page_id == 0 || geometry.width_points <= 0.0 ||
            geometry.height_points <= 0.0)
        {
            throw std::runtime_error("Opened PDF returned invalid page metadata");
        }

        return geometry;
    }

    NativeCoreValidation NativeCoreBridge::RenderTile(
        PdfeditorTileRequest const& request,
        std::function<void(PdfeditorTile const&)> const& tileConsumer) const
    {
        PdfeditorTile tile{};
        const auto result = m_renderTile(m_document, &request, &tile);
        if (result != PDFEDITOR_OK)
        {
            throw std::runtime_error("pdfeditor_document_render_tile failed with code " +
                std::to_string(result));
        }
        return ConsumeTile(tile, tileConsumer);
    }

    NativeCoreValidation NativeCoreBridge::RenderPdfPreview(
        std::filesystem::path const& pdfPath,
        std::function<void(PdfeditorTile const&)> const& tileConsumer)
    {
        const auto geometry = OpenPdf(pdfPath);
        (void)geometry;
        PdfeditorTile tile{};
        const auto result = m_renderPage(m_document, 0, &tile);

        if (result != PDFEDITOR_OK)
        {
            throw std::runtime_error(
                "pdfeditor_document_render_page_preview failed with code " +
                std::to_string(result));
        }

        return ConsumeTile(tile, tileConsumer);
    }

    NativeCoreValidation NativeCoreBridge::ConsumeTile(
        PdfeditorTile& tile,
        std::function<void(PdfeditorTile const&)> const& tileConsumer) const
    {
        struct TileGuard
        {
            PdfeditorTile* tile{};
            TileFreeFn free{};

            ~TileGuard()
            {
                if (tile != nullptr && free != nullptr)
                {
                    free(tile);
                }
            }
        } guard{ &tile, m_tileFree };

        if (tile.data == nullptr || tile.width != 512 || tile.height != 512 ||
            tile.stride < tile.width * 4 ||
            tile.len < static_cast<std::size_t>(tile.stride) * tile.height)
        {
            throw std::runtime_error("Rust core returned an invalid tile");
        }

        if (tileConsumer)
        {
            tileConsumer(tile);
        }

        return NativeCoreValidation{
            m_abiVersion(),
            tile.width,
            tile.height,
            tile.stride,
            tile.len,
        };
    }
}
