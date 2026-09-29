#include "pch.h"
#include "DocumentCanvasRenderer.h"

#include <microsoft.ui.xaml.media.dxinterop.h>

namespace winrt::PdfEditor::implementation
{
    DocumentCanvasRenderer::DocumentCanvasRenderer(
        Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel)
    {
        CreateDevice();
        CreateSwapChain(panel);
    }

    void DocumentCanvasRenderer::CreateDevice()
    {
        constexpr UINT flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        D3D_FEATURE_LEVEL featureLevel{};

        auto result = ::D3D11CreateDevice(
            nullptr,
            D3D_DRIVER_TYPE_HARDWARE,
            nullptr,
            flags,
            nullptr,
            0,
            D3D11_SDK_VERSION,
            m_device.put(),
            &featureLevel,
            m_context.put());

        if (FAILED(result))
        {
            winrt::check_hresult(::D3D11CreateDevice(
                nullptr,
                D3D_DRIVER_TYPE_WARP,
                nullptr,
                flags,
                nullptr,
                0,
                D3D11_SDK_VERSION,
                m_device.put(),
                &featureLevel,
                m_context.put()));
        }
    }

    void DocumentCanvasRenderer::CreateSwapChain(
        Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel)
    {
        auto dxgiDevice = m_device.as<IDXGIDevice>();

        winrt::com_ptr<IDXGIAdapter> adapter;
        winrt::check_hresult(dxgiDevice->GetAdapter(adapter.put()));

        winrt::com_ptr<IDXGIFactory2> factory;
        winrt::check_hresult(
            adapter->GetParent(__uuidof(IDXGIFactory2), factory.put_void()));

        DXGI_SWAP_CHAIN_DESC1 desc{};
        desc.Width = CanvasSize;
        desc.Height = CanvasSize;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.Stereo = FALSE;
        desc.SampleDesc.Count = 1;
        desc.SampleDesc.Quality = 0;
        desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
        desc.BufferCount = 2;
        desc.Scaling = DXGI_SCALING_STRETCH;
        desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL;
        desc.AlphaMode = DXGI_ALPHA_MODE_IGNORE;
        desc.Flags = 0;

        winrt::check_hresult(factory->CreateSwapChainForComposition(
            m_device.get(),
            &desc,
            nullptr,
            m_swapChain.put()));

        auto panelNative = panel.as<ISwapChainPanelNative>();
        winrt::check_hresult(panelNative->SetSwapChain(m_swapChain.get()));
    }

    void DocumentCanvasRenderer::PresentBgra(
        std::uint8_t const* pixels,
        std::uint32_t width,
        std::uint32_t height,
        std::uint32_t stride)
    {
        if (pixels == nullptr)
        {
            throw std::invalid_argument("tile pixels must not be null");
        }

        if (width != CanvasSize || height != CanvasSize || stride < width * 4)
        {
            throw std::invalid_argument(
                "P0 DocumentCanvas requires a 512x512 BGRA tile");
        }

        winrt::com_ptr<ID3D11Texture2D> backBuffer;
        winrt::check_hresult(m_swapChain->GetBuffer(
            0,
            __uuidof(ID3D11Texture2D),
            backBuffer.put_void()));

        m_context->UpdateSubresource(
            backBuffer.get(),
            0,
            nullptr,
            pixels,
            stride,
            0);

        winrt::check_hresult(m_swapChain->Present(1, 0));
    }
}
