#pragma once
#include "NativeCoreBridge.h"
#include <winrt/Microsoft.UI.Xaml.Media.h>
#include <winrt/Microsoft.UI.Xaml.Media.Imaging.h>

namespace winrt::PdfEditor::implementation
{
    // Explicit virtual scrollbar and clipped viewport: no tall canvas or per-page controls.
    class ThumbnailPanel final
    {
    public:
        ThumbnailPanel(Microsoft::UI::Xaml::Controls::Canvas const&, Microsoft::UI::Xaml::Controls::Primitives::ScrollBar const&,
            NativeCoreBridge&, std::function<void(std::uint32_t)>);
        void Refresh(std::uint32_t current, double dpr, std::uint16_t rotation);
        void Scroll(double offset);
        void Wheel(double delta);
        void ShowCurrent();
        void Poll();
        std::wstring MetricsText() const;
    private:
        struct Slot {
            PdfeditorThumbnailItem item{};
            Microsoft::UI::Xaml::Controls::Button button;
            Microsoft::UI::Xaml::Controls::Border frame;
            Microsoft::UI::Xaml::Controls::Image image;
            Microsoft::UI::Xaml::Controls::TextBlock label, placeholder;
            std::size_t bytes{};
        };
        void Update();
        void Highlight();
        std::unique_ptr<Slot> CreateSlot();
        void Clear(Slot&);
        Microsoft::UI::Xaml::Controls::Canvas m_canvas;
        Microsoft::UI::Xaml::Controls::Primitives::ScrollBar m_scroll;
        NativeCoreBridge& m_core;
        std::function<void(std::uint32_t)> m_navigate;
        PdfeditorThumbnailViewport m_view{0, 600, 144, 168, 24, 8, 8, 1, 0, 0, 2, 0};
        PdfeditorThumbnailSnapshot m_snapshot{};
        std::vector<std::unique_ptr<Slot>> m_slots;
        std::size_t m_bytes{};
        std::uint64_t m_uploads{}, m_recycled{}, m_rejected{};
        bool m_updating{};
        std::uint64_t m_editorRevision{}, m_selectionRevision{};
        static constexpr std::size_t NativeBudget = 32 * 1024 * 1024;
    };
}
