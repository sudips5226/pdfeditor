#include "pch.h"
#include "MainWindow.xaml.h"
#include <algorithm>
#include <chrono>
#include <cmath>
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

namespace winrt::PdfEditor::implementation
{
    MainWindow::MainWindow()
    {
        InitializeComponent();
        Title(L"pdfeditor");
        m_pollTimer.Interval(std::chrono::milliseconds(30));
        m_pollTimer.Tick([weak = get_weak()](auto const&, auto const&) {
            if (auto self = weak.get()) self->PollTiles();
        });
        Closed([weak = get_weak()](auto const&, auto const&) {
            if (auto self = weak.get()) self->m_pollTimer.Stop();
        });
    }
    void MainWindow::ValidateCore_Click(winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::RoutedEventArgs const&) { UpdateViewport(); }

    void MainWindow::Canvas_SizeChanged(winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::SizeChangedEventArgs const&)
    {
        if (m_viewport.generation != 0) UpdateViewport();
    }
    void MainWindow::Pan_Click(winrt::Windows::Foundation::IInspectable const& sender,
        Microsoft::UI::Xaml::RoutedEventArgs const&)
    {
        const auto direction = winrt::unbox_value<winrt::hstring>(sender.as<Microsoft::UI::Xaml::Controls::Button>().Tag());
        const auto step = 256.0 / (m_viewport.scale * m_viewport.device_pixel_ratio);
        if (direction == L"Left") m_viewport.origin_x -= step;
        if (direction == L"Right") m_viewport.origin_x += step;
        if (direction == L"Up") m_viewport.origin_y -= step;
        if (direction == L"Down") m_viewport.origin_y += step;
        UpdateViewport();
    }
    void MainWindow::ZoomAt(double factor, double x, double y)
    {
        const auto oldScale = m_viewport.scale * m_viewport.device_pixel_ratio;
        const auto next = std::clamp(m_viewport.scale * factor, 0.125, 8.0);
        const auto newScale = next * m_viewport.device_pixel_ratio;
        m_viewport.origin_x += x / oldScale - x / newScale;
        m_viewport.origin_y += y / oldScale - y / newScale;
        m_viewport.scale = next;
        UpdateViewport();
    }
    void MainWindow::Zoom_Click(winrt::Windows::Foundation::IInspectable const& sender,
        Microsoft::UI::Xaml::RoutedEventArgs const&)
    {
        const auto direction = winrt::unbox_value<winrt::hstring>(sender.as<Microsoft::UI::Xaml::Controls::Button>().Tag());
        ZoomAt(direction == L"In" ? 1.25 : 0.8, m_viewport.width / 2, m_viewport.height / 2);
    }
    void MainWindow::Canvas_Wheel(winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::Input::PointerRoutedEventArgs const& args)
    {
        const auto point = args.GetCurrentPoint(DocumentCanvas());
        const auto delta = point.Properties().MouseWheelDelta();
        const auto ctrl = (args.KeyModifiers() & winrt::Windows::System::VirtualKeyModifiers::Control) != winrt::Windows::System::VirtualKeyModifiers::None;
        if (ctrl) {
            const auto position = point.Position();
            const auto dpr = m_viewport.device_pixel_ratio;
            ZoomAt(std::pow(1.25, static_cast<double>(delta) / 120.0), position.X * dpr, position.Y * dpr);
        } else {
            const auto step = static_cast<double>(delta) * 2.0 / (m_viewport.scale * m_viewport.device_pixel_ratio);
            if (point.Properties().IsHorizontalMouseWheel()) m_viewport.origin_x += step;
            else m_viewport.origin_y -= step;
            UpdateViewport();
        }
        args.Handled(true);
    }
    void MainWindow::Document_Scroll(winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const& args)
    {
        if (m_updatingScroll) return;
        m_viewport.origin_y = args.NewValue();
        UpdateViewport();
    }
    void MainWindow::GoTo_Click(winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::RoutedEventArgs const&)
    {
        try {
            if (!m_core) UpdateViewport();
            const std::wstring text(PageInput().Text());
            std::size_t used{};
            const auto page = std::stoull(text, &used);
            if (used != text.size() || page == 0 || page > m_snapshot.page_count) throw std::runtime_error("Page number outside document");
            m_core->GoToPage(static_cast<std::uint32_t>(page - 1), m_viewport);
            UpdateViewport();
        } catch (std::exception const& error) { StatusText().Text(winrt::to_hstring(error.what())); }
    }
    void MainWindow::UpdateViewport()
    {
        try {
            if (!m_core) m_core = std::make_unique<NativeCoreBridge>();
            if (!m_renderer) m_renderer = std::make_unique<DocumentCanvasRenderer>(DocumentCanvas());
            m_core->OpenContinuousPdf(NativeCoreBridge::P4FixturePath());
            const double dpr = DocumentCanvas().XamlRoot().RasterizationScale();
            auto next = m_viewport;
            next.width = std::clamp(std::floor(DocumentCanvas().ActualWidth() * dpr), 1.0, 16384.0);
            next.height = std::clamp(std::floor(DocumentCanvas().ActualHeight() * dpr), 1.0, 16384.0);
            next.device_pixel_ratio = dpr;
            const std::uint16_t rotation = RotatePage().IsChecked().GetBoolean() ? 90 : 0;
            ++next.generation;
            if (next.rotation_degrees != rotation) {
                next.rotation_degrees = rotation;
                m_core->GoToPage(m_snapshot.current_page, next);
            }
            m_snapshot = m_core->UpdateContinuousViewport(next, m_pages);
            next.origin_x = m_snapshot.origin_x; next.origin_y = m_snapshot.origin_y;
            m_viewport = next;
            m_renderer->Resize(static_cast<std::uint32_t>(next.width), static_cast<std::uint32_t>(next.height));
            m_updatingScroll = true;
            DocumentScroll().Minimum(0);
            DocumentScroll().Maximum((std::max)(0.0, m_snapshot.extent_height - next.height / (next.scale * dpr)));
            DocumentScroll().ViewportSize(next.height / (next.scale * dpr));
            DocumentScroll().SmallChange(128.0 / (next.scale * dpr));
            DocumentScroll().LargeChange(next.height * 0.9 / (next.scale * dpr));
            DocumentScroll().Value(next.origin_y);
            m_updatingScroll = false;
            CurrentPageText().Text(L"Page " + std::to_wstring(m_snapshot.current_page + 1) + L" / " + std::to_wstring(m_snapshot.page_count));
            m_renderer->SetViewport(m_viewport, m_pages);
            m_renderer->ComposeViewport(DocumentCanvas().CompositionScaleX(), DocumentCanvas().CompositionScaleY());
            m_pollTimer.Start();
        } catch (std::exception const& error) {
            m_updatingScroll = false;
            StatusText().Text(winrt::to_hstring(std::string("P4 viewport failed: ") + error.what()));
        }
    }
    void MainWindow::PollTiles()
    {
        if (!m_core || !m_renderer || m_viewport.generation == 0) return;
        try {
            if (m_core->LayoutNeedsRefresh()) UpdateViewport();
            bool changed = false;
            for (int i = 0; i < 4; ++i) {
                if (!m_core->PollReady([&](PdfeditorReadyTile const& tile) {
                    if (tile.generation == m_viewport.generation) {
                        if (!m_renderer->CacheTile(tile)) throw std::runtime_error("GPU tile budget exhausted");
                        changed = true;
                    }
                })) break;
            }
            if (changed) m_renderer->ComposeViewport(DocumentCanvas().CompositionScaleX(), DocumentCanvas().CompositionScaleY());
            const auto m = m_core->Metrics();
            StatusText().Text(L"Document Y " + std::to_wstring(m_viewport.origin_y) + L", extent " + std::to_wstring(m_snapshot.extent_height) +
                L", zoom " + std::to_wstring(m_viewport.scale) + L", generation " + std::to_wstring(m_viewport.generation) +
                L" | visible pages/tiles " + std::to_wstring(m_snapshot.visible_pages) + L" / " + std::to_wstring(m_snapshot.visible_tiles) +
                L", known " + std::to_wstring(m_snapshot.known_pages) + L", metadata bytes " + std::to_wstring(m_snapshot.metadata_bytes) +
                L" | open/layout us " + std::to_wstring(m_snapshot.open_micros) + L" / " + std::to_wstring(m_snapshot.layout_init_micros) +
                L", geometry queries/us " + std::to_wstring(m_snapshot.geometry_queries) + L" / " + std::to_wstring(m_snapshot.geometry_micros) +
                L" | CPU/GPU bytes " + std::to_wstring(m.cpu_cache_bytes) + L" / " + std::to_wstring(m_renderer->GpuBytes()) +
                L", queue/ready " + std::to_wstring(m.queue_depth) + L" / " + std::to_wstring(m.completion_depth) +
                L", renders/hits/stale " + std::to_wstring(m.renders_performed) + L" / " + std::to_wstring(m.cache_hits) + L" / " + std::to_wstring(m.stale_renders_discarded) +
                L", GPU uploads/reuse " + std::to_wstring(m_renderer->GpuUploads()) + L" / " + std::to_wstring(m_renderer->GpuHits()));
        } catch (std::exception const& error) {
            m_pollTimer.Stop(); StatusText().Text(winrt::to_hstring(std::string("P4 completion failed: ") + error.what()));
        }
    }
}
