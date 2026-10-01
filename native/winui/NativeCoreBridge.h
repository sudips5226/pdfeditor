#pragma once

#include "pdfeditor_ffi.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <vector>
#include <windows.h>

namespace winrt::PdfEditor::implementation
{
    static_assert(sizeof(PdfeditorTileRequest) == 48);
    static_assert(offsetof(PdfeditorTileRequest, width) == 36);
    static_assert(sizeof(PdfeditorViewport) == 72);
    static_assert(sizeof(PdfeditorTileKey) == 56);
    static_assert(sizeof(PdfeditorReadyTile) == 104);
    static_assert(sizeof(PdfeditorDocumentViewport) == 72);
    static_assert(sizeof(PdfeditorPageLayout) == 88);
    static_assert(sizeof(PdfeditorLayoutSnapshot) == 96);
    static_assert(sizeof(PdfeditorThumbnailViewport) == 88);
    static_assert(sizeof(PdfeditorThumbnailKey) == 48);
    static_assert(sizeof(PdfeditorThumbnailItem) == 80);
    static_assert(sizeof(PdfeditorReadyThumbnail) == 96);
    static_assert(sizeof(PdfeditorThumbnailMetrics) == 88);
    struct NativeCoreValidation
    {
        std::uint32_t abiVersion{};
        std::uint32_t width{};
        std::uint32_t height{};
        std::uint32_t stride{};
        std::size_t byteCount{};
    };

    class NativeCoreBridge final
    {
    public:
        NativeCoreBridge();
        ~NativeCoreBridge();

        NativeCoreBridge(NativeCoreBridge const&) = delete;
        NativeCoreBridge& operator=(NativeCoreBridge const&) = delete;
        NativeCoreBridge(NativeCoreBridge&&) = delete;
        NativeCoreBridge& operator=(NativeCoreBridge&&) = delete;

        [[nodiscard]] NativeCoreValidation Validate(
            std::function<void(PdfeditorTile const&)> const& tileConsumer = {}) const;
        [[nodiscard]] NativeCoreValidation RenderPdfPreview(
            std::filesystem::path const& pdfPath,
            std::function<void(PdfeditorTile const&)> const& tileConsumer);
        [[nodiscard]] PdfeditorPageGeometry OpenPdf(std::filesystem::path const& pdfPath);
        void OpenContinuousPdf(std::filesystem::path const& pdfPath);
        PdfeditorLayoutSnapshot UpdateContinuousViewport(PdfeditorDocumentViewport const&, std::vector<PdfeditorPageLayout>&) const;
        bool LayoutNeedsRefresh() const;
        PdfeditorThumbnailSnapshot UpdateThumbnails(PdfeditorThumbnailViewport const&, std::vector<PdfeditorThumbnailItem>&) const;
        bool PollThumbnail(std::function<void(PdfeditorReadyThumbnail const&)> const&) const;
        double ShowCurrentThumbnail(PdfeditorThumbnailViewport const&) const;
        PdfeditorThumbnailMetrics ThumbnailMetrics() const;
        void SyncThumbnailCurrent(std::uint32_t) const;
        void GoToPage(std::uint32_t index, PdfeditorDocumentViewport&) const;
        [[nodiscard]] static std::filesystem::path P4FixturePath();
        [[nodiscard]] NativeCoreValidation RenderTile(
            PdfeditorTileRequest const& request,
            std::function<void(PdfeditorTile const&)> const& tileConsumer) const;
        void UpdateViewport(PdfeditorViewport const& viewport) const;
        bool PollReady(std::function<void(PdfeditorReadyTile const&)> const& consumer) const;
        [[nodiscard]] PdfeditorMetrics Metrics() const;
        [[nodiscard]] static std::filesystem::path P0FixturePath();
        [[nodiscard]] static std::filesystem::path P2FixturePath();

    private:
        HMODULE m_module{ nullptr };

        using AbiVersionFn = std::uint32_t(__cdecl*)();
        using RenderTestTileFn = std::int32_t(__cdecl*)(PdfeditorTile*);
        using DocumentOpenFn = std::int32_t(__cdecl*)(char const*, PdfeditorDocument**);
        using DocumentCloseFn = std::int32_t(__cdecl*)(PdfeditorDocument*);
        using PageCountFn = std::int32_t(__cdecl*)(PdfeditorDocument*, std::uint32_t*);
        using PageGeometryFn = std::int32_t(__cdecl*)(PdfeditorDocument*, std::uint32_t, PdfeditorPageGeometry*);
        using RenderPageFn = std::int32_t(__cdecl*)(PdfeditorDocument*, std::uint32_t, PdfeditorTile*);
        using RenderTileFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorTileRequest const*, PdfeditorTile*);
        using TileFreeFn = void(__cdecl*)(PdfeditorTile*);

        AbiVersionFn m_abiVersion{};
        RenderTestTileFn m_renderTestTile{};
        DocumentOpenFn m_documentOpen{};
        DocumentCloseFn m_documentClose{};
        PageCountFn m_pageCount{};
        PageGeometryFn m_pageGeometry{};
        RenderPageFn m_renderPage{};
        RenderTileFn m_renderTile{};
        TileFreeFn m_tileFree{};
        using UpdateViewportFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorViewport const*);
        using PollReadyFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorReadyTile*);
        using LeaseReleaseFn = std::int32_t(__cdecl*)(PdfeditorTileLease*);
        using MetricsFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorMetrics*);
        UpdateViewportFn m_updateViewport{};
        PollReadyFn m_pollReady{};
        LeaseReleaseFn m_releaseLease{};
        MetricsFn m_metrics{};
        using ContinuousFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorDocumentViewport const*, PdfeditorLayoutSnapshot*, PdfeditorPageLayout*, std::uint32_t);
        using RefreshFn = std::int32_t(__cdecl*)(PdfeditorDocument*, std::uint32_t*);
        using GoToFn = std::int32_t(__cdecl*)(PdfeditorDocument*, std::uint32_t, PdfeditorDocumentViewport const*, double*, double*);
        ContinuousFn m_continuous{};
        RefreshFn m_refresh{};
        GoToFn m_goTo{};
        using ThumbnailUpdateFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorThumbnailViewport const*, PdfeditorThumbnailSnapshot*, PdfeditorThumbnailItem*, std::uint32_t);
        using ThumbnailPollFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorReadyThumbnail*);
        using ThumbnailShowFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorThumbnailViewport const*, double*);
        using ThumbnailMetricsFn = std::int32_t(__cdecl*)(PdfeditorDocument*, PdfeditorThumbnailMetrics*);
        ThumbnailUpdateFn m_thumbnailUpdate{};
        ThumbnailPollFn m_thumbnailPoll{};
        ThumbnailShowFn m_thumbnailShow{};
        ThumbnailMetricsFn m_thumbnailMetrics{};
        using ThumbnailSyncFn = std::int32_t(__cdecl*)(PdfeditorDocument*, std::uint32_t);
        ThumbnailSyncFn m_thumbnailSync{};
        PdfeditorDocument* m_document{};
        std::filesystem::path m_documentPath;
        PdfeditorPageGeometry m_openGeometry{};

        [[nodiscard]] static std::filesystem::path ExecutableDirectory();
        [[nodiscard]] static std::filesystem::path CoreDllPath();
        [[nodiscard]] NativeCoreValidation ConsumeTile(
            PdfeditorTile& tile,
            std::function<void(PdfeditorTile const&)> const& tileConsumer) const;
        [[nodiscard]] FARPROC RequireSymbol(char const* name) const;
    };
}
