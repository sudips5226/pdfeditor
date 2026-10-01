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
            if (m_abiVersion() != 6)
            {
                throw std::runtime_error("Native core requires ABI v6");
            }
            m_thumbnailSync = reinterpret_cast<ThumbnailSyncFn>(RequireSymbol("pdfeditor_document_thumbnail_sync_current"));
            m_thumbnailUpdate = reinterpret_cast<ThumbnailUpdateFn>(RequireSymbol("pdfeditor_document_update_thumbnails"));
            m_thumbnailPoll = reinterpret_cast<ThumbnailPollFn>(RequireSymbol("pdfeditor_document_poll_ready_thumbnail"));
            m_thumbnailShow = reinterpret_cast<ThumbnailShowFn>(RequireSymbol("pdfeditor_document_thumbnail_show_current"));
            m_thumbnailMetrics = reinterpret_cast<ThumbnailMetricsFn>(RequireSymbol("pdfeditor_document_thumbnail_metrics"));
            m_continuous = reinterpret_cast<ContinuousFn>(RequireSymbol("pdfeditor_document_update_continuous_viewport"));
            m_refresh = reinterpret_cast<RefreshFn>(RequireSymbol("pdfeditor_document_layout_needs_refresh"));
            m_goTo = reinterpret_cast<GoToFn>(RequireSymbol("pdfeditor_document_go_to_page"));
            m_updateViewport = reinterpret_cast<UpdateViewportFn>(RequireSymbol("pdfeditor_document_update_viewport"));
            m_pollReady = reinterpret_cast<PollReadyFn>(RequireSymbol("pdfeditor_document_poll_ready_tile"));
            m_releaseLease = reinterpret_cast<LeaseReleaseFn>(RequireSymbol("pdfeditor_tile_lease_release"));
            m_metrics = reinterpret_cast<MetricsFn>(RequireSymbol("pdfeditor_document_renderer_metrics"));
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

    void NativeCoreBridge::OpenContinuousPdf(std::filesystem::path const& pdfPath)
    {
        if (m_document != nullptr && m_documentPath == pdfPath) return;
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
            m_openGeometry = {};
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

    }

    PdfeditorPageGeometry NativeCoreBridge::OpenPdf(std::filesystem::path const& pdfPath)
    {
        OpenContinuousPdf(pdfPath);
        if (m_openGeometry.page_id != 0) return m_openGeometry;
        std::uint32_t pageCount{};
        PdfeditorPageGeometry geometry{};
        if (m_pageCount(m_document, &pageCount) != PDFEDITOR_OK || pageCount == 0 ||
            m_pageGeometry(m_document, 0, &geometry) != PDFEDITOR_OK ||
            geometry.page_id == 0 || geometry.width_points <= 0.0 ||
            geometry.height_points <= 0.0)
        {
            throw std::runtime_error("Opened PDF returned invalid page metadata");
        }

        m_openGeometry = geometry;
        return geometry;
    }

    std::filesystem::path NativeCoreBridge::P4FixturePath()
    {
        // Local benchmark can be selected without checking it into the repo.
        std::wstring path(32768, L'\0');
        const auto length = ::GetEnvironmentVariableW(L"PDFEDITOR_DOCUMENT_PATH", path.data(), static_cast<DWORD>(path.size()));
        if (length > 0 && length < path.size()) { path.resize(length); return path; }
        return ExecutableDirectory() / L"p4-mixed-pages.pdf";
    }

    PdfeditorLayoutSnapshot NativeCoreBridge::UpdateContinuousViewport(PdfeditorDocumentViewport const& viewport,
        std::vector<PdfeditorPageLayout>& pages) const
    {
        pages.resize(64);
        PdfeditorLayoutSnapshot snapshot{};
        const auto result = m_continuous(m_document, &viewport, &snapshot, pages.data(), 64);
        if (result != PDFEDITOR_OK) throw std::runtime_error("Continuous viewport rejected: " + std::to_string(result));
        if (snapshot.returned_pages > pages.size()) throw std::runtime_error("Invalid page snapshot");
        pages.resize(snapshot.returned_pages);
        return snapshot;
    }

    PdfeditorThumbnailSnapshot NativeCoreBridge::UpdateThumbnails(PdfeditorThumbnailViewport const& v, std::vector<PdfeditorThumbnailItem>& items) const
    {
        items.resize(64);
        PdfeditorThumbnailSnapshot snapshot{};
        const auto code = m_thumbnailUpdate(m_document, &v, &snapshot, items.data(), 64);
        if (code != PDFEDITOR_OK) throw std::runtime_error("Thumbnail update failed: " + std::to_string(code));
        if (snapshot.returned > items.size()) throw std::runtime_error("Invalid thumbnail snapshot");
        items.resize(snapshot.returned); return snapshot;
    }
    bool NativeCoreBridge::PollThumbnail(std::function<void(PdfeditorReadyThumbnail const&)> const& consumer) const
    {
        PdfeditorReadyThumbnail ready{};
        const auto code = m_thumbnailPoll(m_document, &ready);
        if (code == PDFEDITOR_NO_TILE) return false;
        if (code != PDFEDITOR_OK) throw std::runtime_error("Thumbnail poll failed: " + std::to_string(code));
        struct Guard { PdfeditorTileLease* lease; LeaseReleaseFn release; ~Guard() { if (lease) release(lease); } } guard{ ready.lease, m_releaseLease };
        if (ready.status == 0 && (!ready.lease || !ready.data || ready.key.width > 1024 || ready.key.height > 1024 ||
            ready.stride != ready.key.width * 4 || ready.len != static_cast<std::size_t>(ready.stride) * ready.key.height))
            throw std::runtime_error("Invalid thumbnail lease");
        consumer(ready); return true;
    }
    double NativeCoreBridge::ShowCurrentThumbnail(PdfeditorThumbnailViewport const& v) const
    {
        double offset{};
        const auto code = m_thumbnailShow(m_document, &v, &offset);
        if (code != PDFEDITOR_OK) throw std::runtime_error("Show Current failed: " + std::to_string(code));
        return offset;
    }
    void NativeCoreBridge::SyncThumbnailCurrent(std::uint32_t current) const
    {
        const auto code = m_thumbnailSync(m_document, current);
        if (code != PDFEDITOR_OK) throw std::runtime_error("Thumbnail current sync failed: " + std::to_string(code));
    }
    PdfeditorThumbnailMetrics NativeCoreBridge::ThumbnailMetrics() const
    {
        PdfeditorThumbnailMetrics m{};
        const auto code = m_thumbnailMetrics(m_document, &m);
        if (code != PDFEDITOR_OK) throw std::runtime_error("Thumbnail metrics failed: " + std::to_string(code));
        return m;
    }

    bool NativeCoreBridge::LayoutNeedsRefresh() const
    {
        std::uint32_t ready{};
        const auto result = m_refresh(m_document, &ready);
        if (result != PDFEDITOR_OK) throw std::runtime_error("Geometry probe failed: " + std::to_string(result));
        return ready != 0;
    }

    void NativeCoreBridge::GoToPage(std::uint32_t index, PdfeditorDocumentViewport& viewport) const
    {
        double x{}, y{};
        const auto result = m_goTo(m_document, index, &viewport, &x, &y);
        if (result != PDFEDITOR_OK) throw std::runtime_error("Go to page rejected: " + std::to_string(result));
        viewport.origin_x = x; viewport.origin_y = y;
    }

    void NativeCoreBridge::UpdateViewport(PdfeditorViewport const& viewport) const
    {
        const auto result = m_updateViewport(m_document, &viewport);
        if (result != PDFEDITOR_OK) throw std::runtime_error("Viewport rejected: " + std::to_string(result));
    }

    bool NativeCoreBridge::PollReady(std::function<void(PdfeditorReadyTile const&)> const& consumer) const
    {
        PdfeditorReadyTile tile{};
        const auto result = m_pollReady(m_document, &tile);
        if (result == PDFEDITOR_NO_TILE) return false;
        if (result != PDFEDITOR_OK) throw std::runtime_error("Tile poll failed: " + std::to_string(result));
        struct Guard {
            PdfeditorTileLease* lease;
            LeaseReleaseFn release;
            ~Guard() { release(lease); }
        } guard{ tile.lease, m_releaseLease };
        if (!tile.lease || !tile.data || tile.width != 512 || tile.height != 512 ||
            tile.stride < tile.width * 4 || tile.len < static_cast<std::size_t>(tile.stride) * tile.height)
            throw std::runtime_error("Invalid tile lease");
        consumer(tile);
        return true;
    }

    PdfeditorMetrics NativeCoreBridge::Metrics() const
    {
        PdfeditorMetrics metrics{};
        const auto result = m_metrics(m_document, &metrics);
        if (result != PDFEDITOR_OK) throw std::runtime_error("Metrics failed: " + std::to_string(result));
        return metrics;
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
