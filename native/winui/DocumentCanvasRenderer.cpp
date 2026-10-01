#include "pch.h"
#include "DocumentCanvasRenderer.h"

#include <microsoft.ui.xaml.media.dxinterop.h>
#include <cmath>
#include <algorithm>
#include <bit>
#include <d3dcompiler.h>

namespace winrt::PdfEditor::implementation
{
    DocumentCanvasRenderer::DocumentCanvasRenderer(
        Microsoft::UI::Xaml::Controls::SwapChainPanel const& panel,
        std::size_t gpuByteBudget, std::size_t gpuCountBudget)
        : m_gpuByteBudget(gpuByteBudget), m_gpuCountBudget(gpuCountBudget)
    {
        if (gpuByteBudget < 512ull * 512 * 4 || gpuCountBudget == 0)
            throw std::invalid_argument("GPU budget must admit at least one tile");
        CreateDevice();
        CreateSwapChain(panel);
        CreateCompositionPipeline();
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
        desc.Width = m_canvasWidth;
        desc.Height = m_canvasHeight;
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
        constexpr float background[4] = { 0.38f, 0.38f, 0.38f, 1.0f };
        m_context->ClearRenderTargetView(view.get(), background);
    }

    void DocumentCanvasRenderer::UploadBgra(
        std::uint8_t const* pixels, std::uint32_t width, std::uint32_t height,
        std::uint32_t stride, std::uint32_t canvasX, std::uint32_t canvasY)
    {
        if (!m_frameBuffer || pixels == nullptr || width != 512 || height != 512 ||
            stride < width * 4 || width > m_canvasWidth || height > m_canvasHeight || canvasX > m_canvasWidth - width || canvasY > m_canvasHeight - height)
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
    void DocumentCanvasRenderer::CreateCompositionPipeline()
    {
        constexpr char shader[] = R"(
            cbuffer Placement : register(b0) { float4 rect; };
            struct Vertex { float4 position : SV_POSITION; float2 uv : TEXCOORD0; };
            Vertex vs(uint id : SV_VertexID) {
                float2 coords[6] = { float2(0,0), float2(1,0), float2(0,1),
                                     float2(0,1), float2(1,0), float2(1,1) };
                Vertex v; v.uv = coords[id];
                v.position = float4(lerp(rect.xy, rect.zw, v.uv), 0, 1); return v;
            }
            Texture2D pixels : register(t0); SamplerState sampleState : register(s0);
            float4 ps(Vertex v) : SV_TARGET { return pixels.Sample(sampleState, v.uv); }
        )";
        winrt::com_ptr<ID3DBlob> vs, ps, errors;
        winrt::check_hresult(D3DCompile(shader, sizeof(shader), nullptr, nullptr, nullptr,
            "vs", "vs_4_0", D3DCOMPILE_ENABLE_STRICTNESS, 0, vs.put(), errors.put()));
        errors = nullptr;
        winrt::check_hresult(D3DCompile(shader, sizeof(shader), nullptr, nullptr, nullptr,
            "ps", "ps_4_0", D3DCOMPILE_ENABLE_STRICTNESS, 0, ps.put(), errors.put()));
        winrt::check_hresult(m_device->CreateVertexShader(vs->GetBufferPointer(), vs->GetBufferSize(), nullptr, m_vertexShader.put()));
        winrt::check_hresult(m_device->CreatePixelShader(ps->GetBufferPointer(), ps->GetBufferSize(), nullptr, m_pixelShader.put()));
        D3D11_BUFFER_DESC buffer{};
        buffer.ByteWidth = 16; buffer.Usage = D3D11_USAGE_DEFAULT;
        buffer.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
        winrt::check_hresult(m_device->CreateBuffer(&buffer, nullptr, m_constants.put()));
        D3D11_SAMPLER_DESC sampler{};
        sampler.Filter = D3D11_FILTER_MIN_MAG_MIP_POINT;
        sampler.AddressU = sampler.AddressV = sampler.AddressW = D3D11_TEXTURE_ADDRESS_CLAMP;
        sampler.MaxLOD = D3D11_FLOAT32_MAX;
        winrt::check_hresult(m_device->CreateSamplerState(&sampler, m_sampler.put()));
        D3D11_RASTERIZER_DESC rasterizer{};
        rasterizer.FillMode = D3D11_FILL_SOLID; rasterizer.CullMode = D3D11_CULL_NONE;
        rasterizer.DepthClipEnable = TRUE; rasterizer.ScissorEnable = TRUE;
        winrt::check_hresult(m_device->CreateRasterizerState(&rasterizer, m_rasterizer.put()));
    }

    void DocumentCanvasRenderer::Resize(std::uint32_t width, std::uint32_t height)
    {
        if (width == m_canvasWidth && height == m_canvasHeight) return;
        if (width == 0 || height == 0 || width > 16384 || height > 16384) throw std::invalid_argument("Invalid canvas size");
        m_context->OMSetRenderTargets(0, nullptr, nullptr);
        m_frameBuffer = nullptr;
        winrt::check_hresult(m_swapChain->ResizeBuffers(2, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, 0));
        m_canvasWidth = width; m_canvasHeight = height; m_hasContent = false;
    }

    void DocumentCanvasRenderer::SetViewport(PdfeditorDocumentViewport const& viewport, std::vector<PdfeditorPageLayout> const& pages)
    {
        m_viewport = viewport; m_pages = pages;
    }

    PdfeditorPageLayout const* DocumentCanvasRenderer::FindPage(std::uint64_t id) const
    {
        for (auto const& page : m_pages) if (page.page_id == id) return &page;
        return nullptr;
    }

    D3D11_RECT DocumentCanvasRenderer::PageClip(PdfeditorPageLayout const& p) const
    {
        const auto scale = m_viewport.scale * m_viewport.device_pixel_ratio;
        const auto x = (p.x - m_viewport.origin_x) * scale;
        const auto y = (p.y - m_viewport.origin_y) * scale;
        // Subtract document origin in double, clamp, then convert to native pixels.
        const auto bounded = [](double v, double max) { return std::clamp(v, 0.0, max); };
        return { static_cast<LONG>(std::ceil(bounded(x, m_canvasWidth))), static_cast<LONG>(std::ceil(bounded(y, m_canvasHeight))),
            static_cast<LONG>(std::ceil(bounded(x + p.width * scale, m_canvasWidth))),
            static_cast<LONG>(std::ceil(bounded(y + p.height * scale, m_canvasHeight))) };
    }

    bool DocumentCanvasRenderer::Intersects(PdfeditorTileKey const& key, bool currentOnly) const
    {
        const auto p = FindPage(key.page_id);
        if (!p || !p->geometry_known || key.rotation_degrees != m_viewport.rotation_degrees) return false;
        const auto scale = std::bit_cast<double>(key.physical_scale_bits);
        const auto targetScale = m_viewport.scale * m_viewport.device_pixel_ratio;
        if (currentOnly && scale != targetScale) return false;
        const auto ratio = targetScale / scale;
        const auto x = (p->x - m_viewport.origin_x) * targetScale + static_cast<double>(key.tile_x) * key.width * ratio;
        const auto y = (p->y - m_viewport.origin_y) * targetScale + static_cast<double>(key.tile_y) * key.height * ratio;
        const auto clip = PageClip(*p);
        return clip.right > clip.left && clip.bottom > clip.top && x < clip.right && y < clip.bottom &&
            x + key.width * ratio > clip.left && y + key.height * ratio > clip.top;
    }

    bool DocumentCanvasRenderer::CacheTile(PdfeditorReadyTile const& tile)
    {
        if (tile.generation != m_viewport.generation) return false;
        auto found = m_tiles.find(tile.key);
        if (found != m_tiles.end()) {
            found->second.touched = ++m_clock; ++m_gpuHits; return true;
        }
        const auto bytes = static_cast<std::size_t>(tile.width) * tile.height * 4;
        if (bytes > m_gpuByteBudget) return false;
        while (m_gpuBytes > m_gpuByteBudget - bytes || m_tiles.size() >= m_gpuCountBudget) {
            auto victim = m_tiles.end();
            for (auto it = m_tiles.begin(); it != m_tiles.end(); ++it) {
                if (Intersects(it->first, true)) continue; // Current visible pixels stay pinned.
                if (victim == m_tiles.end() || it->second.touched < victim->second.touched) victim = it;
            }
            if (victim == m_tiles.end()) return false;
            m_gpuBytes -= victim->second.bytes; m_tiles.erase(victim);
        }
        TextureEntry entry{};
        D3D11_TEXTURE2D_DESC desc{};
        desc.Width = tile.width; desc.Height = tile.height;
        desc.MipLevels = 1; desc.ArraySize = 1; desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 1; desc.Usage = D3D11_USAGE_IMMUTABLE;
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
        const D3D11_SUBRESOURCE_DATA data{ tile.data, tile.stride, 0 };
        winrt::check_hresult(m_device->CreateTexture2D(&desc, &data, entry.texture.put()));
        winrt::check_hresult(m_device->CreateShaderResourceView(entry.texture.get(), nullptr, entry.view.put()));
        entry.bytes = bytes; entry.touched = ++m_clock;
        m_tiles.emplace(tile.key, std::move(entry));
        m_gpuBytes += bytes; ++m_gpuUploads;
        return true;
    }

    void DocumentCanvasRenderer::DrawTile(PdfeditorTileKey const& key, TextureEntry const& entry)
    {
        const auto p = FindPage(key.page_id);
        if (!p) return;
        const auto scale = m_viewport.scale * m_viewport.device_pixel_ratio;
        const auto ratio = scale / std::bit_cast<double>(key.physical_scale_bits);
        const auto x = (p->x - m_viewport.origin_x) * scale + static_cast<double>(key.tile_x) * key.width * ratio;
        const auto y = (p->y - m_viewport.origin_y) * scale + static_cast<double>(key.tile_y) * key.height * ratio;
        const auto clip = PageClip(*p);
        m_context->RSSetScissorRects(1, &clip);
        const float rect[4] = {
            static_cast<float>(2 * x / m_canvasWidth - 1), static_cast<float>(1 - 2 * y / m_canvasHeight),
            static_cast<float>(2 * (x + key.width * ratio) / m_canvasWidth - 1),
            static_cast<float>(1 - 2 * (y + key.height * ratio) / m_canvasHeight) };
        m_context->UpdateSubresource(m_constants.get(), 0, nullptr, rect, 0, 0);
        auto resource = entry.view.get();
        m_context->PSSetShaderResources(0, 1, &resource);
        m_context->Draw(6, 0);
    }

    void DocumentCanvasRenderer::ComposeViewport(float compositionScaleX, float compositionScaleY)
    {
        if (m_viewport.generation == 0) return;
        std::size_t nativeCount{};
        for (auto const& item : m_tiles) if (Intersects(item.first, true)) ++nativeCount;
        std::vector<decltype(m_tiles)::iterator> fallback;
        for (auto it = m_tiles.begin(); it != m_tiles.end(); ++it)
            if (Intersects(it->first, false) && !Intersects(it->first, true)) fallback.push_back(it);
        // Keep the displayed frame for distant/rotation destinations until any
        // compatible content arrives. Scroll position and demand update now.
        bool visiblePage = false;
        for (auto const& p : m_pages) {
            const auto clip = PageClip(p);
            visiblePage = visiblePage || (clip.right > clip.left && clip.bottom > clip.top);
        }
        if (m_hasContent && visiblePage && nativeCount == 0 && fallback.empty()) return;
        BeginFrame(compositionScaleX, compositionScaleY);
        winrt::com_ptr<ID3D11RenderTargetView> view;
        winrt::check_hresult(m_device->CreateRenderTargetView(m_frameBuffer.get(), nullptr, view.put()));
        auto target = view.get(); m_context->OMSetRenderTargets(1, &target, nullptr);
        const D3D11_VIEWPORT viewport{ 0, 0, static_cast<float>(m_canvasWidth), static_cast<float>(m_canvasHeight), 0, 1 };
        m_context->RSSetViewports(1, &viewport); m_context->RSSetState(m_rasterizer.get());
        m_context->IASetInputLayout(nullptr);
        m_context->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        m_context->VSSetShader(m_vertexShader.get(), nullptr, 0);
        m_context->PSSetShader(m_pixelShader.get(), nullptr, 0);
        auto constants = m_constants.get(); m_context->VSSetConstantBuffers(0, 1, &constants);
        auto sampler = m_sampler.get(); m_context->PSSetSamplers(0, 1, &sampler);
        // White page rectangles distinguish page bounds from the neutral gap.
        auto context1 = m_context.as<ID3D11DeviceContext1>();
        constexpr float paper[4] = { 1, 1, 1, 1 };
        for (auto const& p : m_pages) {
            const auto clip = PageClip(p);
            if (clip.right > clip.left && clip.bottom > clip.top) context1->ClearView(view.get(), paper, &clip, 1);
        }
        std::sort(fallback.begin(), fallback.end(), [](auto a, auto b) { return a->second.touched < b->second.touched; });
        for (auto it : fallback) DrawTile(it->first, it->second);
        for (auto& item : m_tiles) {
            if (Intersects(item.first, true)) {
                DrawTile(item.first, item.second); item.second.touched = ++m_clock;
            }
        }
        ID3D11ShaderResourceView* none = nullptr;
        m_context->PSSetShaderResources(0, 1, &none);
        m_context->OMSetRenderTargets(0, nullptr, nullptr);
        EndFrame();
        m_hasContent = nativeCount > 0 || !fallback.empty();
    }

}
