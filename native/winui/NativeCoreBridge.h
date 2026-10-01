#pragma once

#include "pdfeditor_ffi.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <windows.h>

namespace winrt::PdfEditor::implementation
{
    static_assert(sizeof(PdfeditorTileRequest) == 48);
    static_assert(offsetof(PdfeditorTileRequest, width) == 36);
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
        [[nodiscard]] NativeCoreValidation RenderTile(
            PdfeditorTileRequest const& request,
            std::function<void(PdfeditorTile const&)> const& tileConsumer) const;
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
        PdfeditorDocument* m_document{};
        std::filesystem::path m_documentPath;

        [[nodiscard]] static std::filesystem::path ExecutableDirectory();
        [[nodiscard]] static std::filesystem::path CoreDllPath();
        [[nodiscard]] NativeCoreValidation ConsumeTile(
            PdfeditorTile& tile,
            std::function<void(PdfeditorTile const&)> const& tileConsumer) const;
        [[nodiscard]] FARPROC RequireSymbol(char const* name) const;
    };
}
