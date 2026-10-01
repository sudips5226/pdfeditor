#pragma once

#include <cstdint>

#include <d3d11.h>
#include <dxgi1_3.h>
#include <winrt/base.h>
#include <winrt/Microsoft.UI.Xaml.Controls.h>

namespace winrt::PdfEditor::implementation
{
    class DocumentCanvasRenderer final
    {
    public:
        explicit DocumentCanvasRenderer(
            Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel);

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

    private:
        static constexpr std::uint32_t CanvasSize = 1024;

        winrt::com_ptr<ID3D11Device> m_device;
        winrt::com_ptr<ID3D11DeviceContext> m_context;
        winrt::com_ptr<IDXGISwapChain1> m_swapChain;

        winrt::com_ptr<ID3D11Texture2D> m_frameBuffer;

        void CreateDevice();
        void CreateSwapChain(
            Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel);
    };
}
