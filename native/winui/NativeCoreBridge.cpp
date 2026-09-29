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

    std::filesystem::path NativeCoreBridge::CoreDllPath()
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
        return std::filesystem::path(buffer).parent_path() / L"pdfeditor_core.dll";
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

    NativeCoreValidation NativeCoreBridge::Validate() const
    {
        PdfeditorTile tile{};
        const auto result = m_renderTestTile(&tile);

        if (result != PDFEDITOR_OK)
        {
            throw std::runtime_error(
                "pdfeditor_render_test_tile failed with code " + std::to_string(result));
        }

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

        if (tile.data == nullptr || tile.width != 512 || tile.height != 512)
        {
            throw std::runtime_error("Rust core returned an invalid P0 tile");
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
