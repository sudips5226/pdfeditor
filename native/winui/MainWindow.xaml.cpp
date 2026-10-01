#include "pch.h"
#include "MainWindow.xaml.h"
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

namespace winrt::PdfEditor::implementation
{
    MainWindow::MainWindow()
    {
        InitializeComponent();
        Title(L"pdfeditor");
    }

    void MainWindow::ValidateCore_Click(
        winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::RoutedEventArgs const&)
    {
        try
        {
            if (!m_core)
            {
                m_core = std::make_unique<NativeCoreBridge>();
            }

            if (!m_renderer)
            {
                m_renderer = std::make_unique<DocumentCanvasRenderer>(DocumentCanvas());
            }

            const auto geometry = m_core->OpenPdf(NativeCoreBridge::P2FixturePath());
            const double dpr = DocumentCanvas().XamlRoot().RasterizationScale();
            const double zoom = HighZoom().IsChecked().GetBoolean() ? 2.0 : 1.0;
            const std::uint16_t rotation = RotatePage().IsChecked().GetBoolean() ? 90 : 0;
            // Keep the fixed 1024-physical-pixel canvas at 1:1 on this display.
            DocumentCanvas().Width(1024.0 / dpr);
            DocumentCanvas().Height(1024.0 / dpr);
            m_renderer->BeginFrame(DocumentCanvas().CompositionScaleX(), DocumentCanvas().CompositionScaleY());
            for (std::int32_t y = 0; y < 2; ++y)
            {
                for (std::int32_t x = 0; x < 2; ++x)
                {
                    const PdfeditorTileRequest request{
                        geometry.page_id, x, y, zoom, dpr, rotation, 512, 512 };
                    const auto result = m_core->RenderTile(request,
                        [this, x, y](PdfeditorTile const& tile)
                        {
                            m_renderer->UploadBgra(tile.data, tile.width, tile.height, tile.stride,
                                static_cast<std::uint32_t>(x) * 512,
                                static_cast<std::uint32_t>(y) * 512);
                        });
                    (void)result;
                }
            }
            m_renderer->EndFrame();
            std::wstring message = L"ABI v3: four independent 512 x 512 regions. PageId " +
                std::to_wstring(geometry.page_id) + L", zoom " + std::to_wstring(zoom) +
                L", DPR " + std::to_wstring(dpr) + L", rotation " + std::to_wstring(rotation) +
                L" degrees. Grid: (0,0), (1,0), (0,1), (1,1).";

            StatusText().Text(message);
        }
        catch (std::exception const& error)
        {
            StatusText().Text(
                winrt::to_hstring(std::string("P2 render failed: ") + error.what()));
        }
    }
}
