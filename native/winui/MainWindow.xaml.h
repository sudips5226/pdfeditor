#pragma once

#include "MainWindow.g.h"
#include "DocumentCanvasRenderer.h"
#include "NativeCoreBridge.h"

namespace winrt::PdfEditor::implementation
{
    struct MainWindow : MainWindowT<MainWindow>
    {
        MainWindow();

        void ValidateCore_Click(
            winrt::Windows::Foundation::IInspectable const& sender,
            Microsoft::UI::Xaml::RoutedEventArgs const& args);

    private:
        std::unique_ptr<NativeCoreBridge> m_core;
        std::unique_ptr<DocumentCanvasRenderer> m_renderer;
    };
}

namespace winrt::PdfEditor::factory_implementation
{
    struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow>
    {
    };
}
