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

            const auto result = m_core->Validate(
                [this](PdfeditorTile const& tile)
                {
                    m_renderer->PresentBgra(
                        tile.data,
                        tile.width,
                        tile.height,
                        tile.stride);
                });

            std::wstring message =
                L"ABI v" + std::to_wstring(result.abiVersion) +
                L" → Rust tile " +
                std::to_wstring(result.width) + L" × " +
                std::to_wstring(result.height) +
                L" → Direct3D swap chain. " +
                std::to_wstring(result.byteCount) + L" bytes transferred.";

            StatusText().Text(message);
        }
        catch (std::exception const& error)
        {
            StatusText().Text(
                winrt::to_hstring(std::string("P0 render failed: ") + error.what()));
        }
    }
}
