#include "pch.h"
#include "DocumentCanvasRenderer.h"

#include <microsoft.ui.xaml.media.dxinterop.h>
#include <cmath>

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
        BeginFrame();
        UploadBgra(pixels, width, height, stride, 0, 0);
        EndFrame();
    }

    void DocumentCanvasRenderer::BeginFrame(float compositionScaleX, float compositionScaleY)
    {
        if (!std::isfinite(compositionScaleX) || !std::isfinite(compositionScaleY) ||
            compositionScaleX <= 0.0f || compositionScaleY <= 0.0f)
        {
            throw std::invalid_argument("Invalid canvas composition scale");
        }
        // The swap chain stores physical pixels; XAML composes in DIPs.
        const DXGI_MATRIX_3X2_F inverseScale{
            1.0f / compositionScaleX, 0.0f, 0.0f, 1.0f / compositionScaleY, 0.0f, 0.0f };
        winrt::check_hresult(m_swapChain.as<IDXGISwapChain2>()->SetMatrixTransform(&inverseScale));
        m_frameBuffer = nullptr;
        winrt::check_hresult(m_swapChain->GetBuffer(
            0, __uuidof(ID3D11Texture2D), m_frameBuffer.put_void()));
        winrt::com_ptr<ID3D11RenderTargetView> view;
        winrt::check_hresult(m_device->CreateRenderTargetView(m_frameBuffer.get(), nullptr, view.put()));
        constexpr float background[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
        m_context->ClearRenderTargetView(view.get(), background);
    }

    void DocumentCanvasRenderer::UploadBgra(
        std::uint8_t const* pixels, std::uint32_t width, std::uint32_t height,
        std::uint32_t stride, std::uint32_t canvasX, std::uint32_t canvasY)
    {
        if (!m_frameBuffer || pixels == nullptr || width != 512 || height != 512 ||
            stride < width * 4 || canvasX > CanvasSize - width || canvasY > CanvasSize - height)
        {
            throw std::invalid_argument("Invalid tile or canvas placement");
        }
        const D3D11_BOX region{ canvasX, canvasY, 0, canvasX + width, canvasY + height, 1 };
        m_context->UpdateSubresource(m_frameBuffer.get(), 0, &region, pixels, stride, 0);
    }

    void DocumentCanvasRenderer::EndFrame()
    {
        if (!m_frameBuffer)
        {
            throw std::logic_error("BeginFrame must precede EndFrame");
        }
        m_frameBuffer = nullptr;
        winrt::check_hresult(m_swapChain->Present(1, 0));
    }
}
