#pragma once

#include <cstdint>
#include <map>
#include <set>
#include <tuple>
#include <vector>
#include "pdfeditor_ffi.h"

#include <d3d11_1.h>
#include <dxgi1_3.h>
#include <winrt/base.h>
#include <winrt/Microsoft.UI.Xaml.Controls.h>

namespace winrt::PdfEditor::implementation
{
    struct TileKeyLess {
        bool operator()(PdfeditorTileKey const& a, PdfeditorTileKey const& b) const {
            return std::tie(a.document_id, a.document_revision, a.page_id, a.physical_scale_bits,
                a.tile_x, a.tile_y, a.width, a.height, a.render_flags, a.rotation_degrees) <
                std::tie(b.document_id, b.document_revision, b.page_id, b.physical_scale_bits,
                b.tile_x, b.tile_y, b.width, b.height, b.render_flags, b.rotation_degrees);
        }
    };

    class DocumentCanvasRenderer final
    {
    public:
        explicit DocumentCanvasRenderer(
            Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel,
            std::size_t gpuByteBudget = 128ull * 1024 * 1024, std::size_t gpuCountBudget = 256);

        DocumentCanvasRenderer(DocumentCanvasRenderer const&) = delete;
        DocumentCanvasRenderer& operator=(DocumentCanvasRenderer const&) = delete;
        DocumentCanvasRenderer(DocumentCanvasRenderer&&) = delete;
        DocumentCanvasRenderer& operator=(DocumentCanvasRenderer&&) = delete;

        void PresentBgra(
            std::uint8_t const* pixels,
            std::uint32_t width,
            std::uint32_t height,
            std::uint32_t stride);

        void BeginFrame(float compositionScaleX = 1.0f, float compositionScaleY = 1.0f);
        void UploadBgra(
            std::uint8_t const* pixels, std::uint32_t width, std::uint32_t height,
            std::uint32_t stride, std::uint32_t canvasX, std::uint32_t canvasY);
        void EndFrame();
        void Resize(std::uint32_t width, std::uint32_t height);
        void InvalidateFrame() {} // Edits retain the successful frame until replacement.
        void SetViewport(PdfeditorDocumentViewport const& viewport, std::vector<PdfeditorPageLayout> const& pages, std::vector<PdfeditorTileKey> const& required);
        std::vector<PdfeditorTileKey> ResidentKeys() const;
        [[nodiscard]] bool MemoryBlocked() const { return m_memoryBlocked; }
        bool CacheTile(PdfeditorReadyTile const& tile);
        bool ComposeViewport(float compositionScaleX, float compositionScaleY);
        [[nodiscard]] std::size_t GpuBytes() const { return m_gpuBytes; }
        [[nodiscard]] std::size_t GpuCount() const { return m_tiles.size(); }
        [[nodiscard]] std::uint64_t GpuUploads() const { return m_gpuUploads; }
        [[nodiscard]] std::uint64_t GpuHits() const { return m_gpuHits; }

    private:
        std::uint32_t m_canvasWidth = 1024, m_canvasHeight = 1024;

        winrt::com_ptr<ID3D11Device> m_device;
        winrt::com_ptr<ID3D11DeviceContext> m_context;
        winrt::com_ptr<IDXGISwapChain1> m_swapChain;

        winrt::com_ptr<ID3D11Texture2D> m_frameBuffer;

        struct TextureEntry {
            winrt::com_ptr<ID3D11Texture2D> texture;
            winrt::com_ptr<ID3D11ShaderResourceView> view;
            std::size_t bytes{};
            std::uint64_t touched{};
        };
        std::map<PdfeditorTileKey, TextureEntry, TileKeyLess> m_tiles;
        std::size_t m_gpuByteBudget{}, m_gpuCountBudget{}, m_gpuBytes{};
        std::uint64_t m_clock{}, m_gpuUploads{}, m_gpuHits{};
        PdfeditorDocumentViewport m_viewport{};
        std::vector<PdfeditorPageLayout> m_pages;
        std::set<PdfeditorTileKey, TileKeyLess> m_required, m_displayedKeys;
        bool m_memoryBlocked{};
        void TrimCache(std::size_t bytes, std::size_t count);
        winrt::com_ptr<ID3D11VertexShader> m_vertexShader;
        winrt::com_ptr<ID3D11PixelShader> m_pixelShader;
        winrt::com_ptr<ID3D11Buffer> m_constants;
        winrt::com_ptr<ID3D11SamplerState> m_sampler;
        winrt::com_ptr<ID3D11RasterizerState> m_rasterizer;
        void CreateCompositionPipeline();
        void DrawTile(PdfeditorTileKey const& key, TextureEntry const& entry);
        PdfeditorPageLayout const* FindPage(std::uint64_t pageId) const;
        D3D11_RECT PageClip(PdfeditorPageLayout const&) const;
        void CreateDevice();
        void CreateSwapChain(
            Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel);
    };
}
