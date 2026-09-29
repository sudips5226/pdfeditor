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

        m_abiVersion = reinterpret_cast<AbiVersionFn>(RequireSymbol("pdfeditor_abi_version"));
        m_renderTestTile =
            reinterpret_cast<RenderTestTileFn>(RequireSymbol("pdfeditor_render_test_tile"));
        m_renderPdfPreview = reinterpret_cast<RenderPdfPreviewFn>(
            RequireSymbol("pdfeditor_render_pdf_preview_utf8"));
        m_tileFree = reinterpret_cast<TileFreeFn>(RequireSymbol("pdfeditor_tile_free"));
    }

    NativeCoreBridge::~NativeCoreBridge()
    {
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

    NativeCoreValidation NativeCoreBridge::RenderPdfPreview(
        std::filesystem::path const& pdfPath,
        std::function<void(PdfeditorTile const&)> const& tileConsumer) const
    {
        if (!std::filesystem::is_regular_file(pdfPath))
        {
            throw std::runtime_error("P0 PDF fixture is missing: " + pdfPath.string());
        }

        const auto utf8Path = pdfPath.u8string();
        const std::string pathBytes(
            reinterpret_cast<char const*>(utf8Path.data()), utf8Path.size());
        PdfeditorTile tile{};
        const auto result = m_renderPdfPreview(pathBytes.c_str(), 0, &tile);

        if (result != PDFEDITOR_OK)
        {
            throw std::runtime_error(
                "pdfeditor_render_pdf_preview_utf8 failed with code " +
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
            throw std::runtime_error("Rust core returned an invalid P0 tile");
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
