#include "pch.h"
#include "MainWindow.xaml.h"
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

#include "NativeCoreBridge.h"

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
            NativeCoreBridge core;
            const auto result = core.Validate();

            std::wstring message =
                L"ABI v" + std::to_wstring(result.abiVersion) +
                L" loaded successfully. Tile: " +
                std::to_wstring(result.width) + L" × " +
                std::to_wstring(result.height) +
                L", stride " + std::to_wstring(result.stride) +
                L", " + std::to_wstring(result.byteCount) + L" bytes.";

            StatusText().Text(message);
        }
        catch (std::exception const& error)
        {
            StatusText().Text(winrt::to_hstring(std::string("Validation failed: ") + error.what()));
        }
    }
}
