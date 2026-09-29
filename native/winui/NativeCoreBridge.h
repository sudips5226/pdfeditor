#pragma once

#include "pdfeditor_ffi.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <windows.h>

namespace winrt::PdfEditor::implementation
{
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

    private:
        HMODULE m_module{ nullptr };

        using AbiVersionFn = std::uint32_t(__cdecl*)();
        using RenderTestTileFn = std::int32_t(__cdecl*)(PdfeditorTile*);
        using TileFreeFn = void(__cdecl*)(PdfeditorTile*);

        AbiVersionFn m_abiVersion{};
        RenderTestTileFn m_renderTestTile{};
        TileFreeFn m_tileFree{};

        [[nodiscard]] static std::filesystem::path CoreDllPath();
        [[nodiscard]] FARPROC RequireSymbol(char const* name) const;
    };
}
