#pragma once

#include "MainWindow.g.h"
#include "DocumentCanvasRenderer.h"
#include "NativeCoreBridge.h"
#include <winrt/Microsoft.UI.Xaml.Input.h>
#include <winrt/Microsoft.UI.Input.h>
#include <winrt/Windows.System.h>
#include <winrt/Windows.UI.Input.h>

namespace winrt::PdfEditor::implementation
{
    struct MainWindow : MainWindowT<MainWindow>
    {
        MainWindow();

        void ValidateCore_Click(
            winrt::Windows::Foundation::IInspectable const& sender,
            Microsoft::UI::Xaml::RoutedEventArgs const& args);

        void Pan_Click(winrt::Windows::Foundation::IInspectable const& sender,
            Microsoft::UI::Xaml::RoutedEventArgs const& args);

        void Canvas_SizeChanged(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&);
        void Zoom_Click(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void GoTo_Click(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void Canvas_Wheel(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Input::PointerRoutedEventArgs const&);
        void Document_Scroll(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const&);
    private:
        void UpdateViewport();
        void ZoomAt(double factor, double x, double y);
        void PollTiles();
        Microsoft::UI::Xaml::DispatcherTimer m_pollTimer;
        PdfeditorDocumentViewport m_viewport{ 0, 0, 1024, 1024, 1, 1, 24, 0, 0 };
        PdfeditorLayoutSnapshot m_snapshot{};
        std::vector<PdfeditorPageLayout> m_pages;
        bool m_updatingScroll{};
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
