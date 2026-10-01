#pragma once

#include "MainWindow.g.h"
#include "DocumentCanvasRenderer.h"
#include "NativeCoreBridge.h"
#include "ThumbnailPanel.h"
#include <winrt/Microsoft.UI.Xaml.Media.h>
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
        void ShowCurrent_Click(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void Thumbnails_SizeChanged(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&);
        void Thumbnails_Scroll(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const&);
        void Thumbnails_Wheel(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Input::PointerRoutedEventArgs const&);
        void Edit_Click(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        winrt::fire_and_forget Output_Click(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&);
        void Editor_KeyDown(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Input::KeyRoutedEventArgs const&);
    private:
        void ApplyEdit(std::uint32_t command, std::int32_t argument = 0);
        void UpdateEditorControls();
        void UpdateViewport();
        void ZoomAt(double factor, double x, double y);
        void PollTiles();
        void TryCommitPresentation();
        void RefreshThumbnails();
        Microsoft::UI::Xaml::DispatcherTimer m_pollTimer;
        PdfeditorDocumentViewport m_viewport{ 0, 0, 1024, 1024, 1, 1, 24, 0, 0 };
        PdfeditorLayoutSnapshot m_snapshot{};
        std::vector<PdfeditorPageLayout> m_pages;
        bool m_updatingScroll{};
        Microsoft::UI::Xaml::XamlRoot m_root{nullptr};
        winrt::event_token m_rootChanged{};
        std::unique_ptr<NativeCoreBridge> m_core;
        std::filesystem::path m_documentPath;
        std::unique_ptr<DocumentCanvasRenderer> m_renderer;
        std::unique_ptr<ThumbnailPanel> m_thumbnails;
    };
}

namespace winrt::PdfEditor::factory_implementation
{
    struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow>
    {
    };
}
