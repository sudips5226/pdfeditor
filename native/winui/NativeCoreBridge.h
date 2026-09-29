#pragma once

#include <cstddef>
#include <cstdint>

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

        [[nodiscard]] NativeCoreValidation Validate() const;

    private:
        HMODULE m_module{ nullptr };

        using AbiVersionFn = std::uint32_t(__cdecl*)();
        using RenderTestTileFn = std::int32_t(__cdecl*)(struct PdfeditorTile*);
        using TileFreeFn = void(__cdecl*)(struct PdfeditorTile*);

        AbiVersionFn m_abiVersion{};
        RenderTestTileFn m_renderTestTile{};
        TileFreeFn m_tileFree{};

        [[nodiscard]] static std::filesystem::path CoreDllPath();
        [[nodiscard]] FARPROC RequireSymbol(char const* name) const;
    };
}
