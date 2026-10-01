#include "pch.h"
#include "MainWindow.xaml.h"
#include <algorithm>
#include <chrono>
#include <cmath>
#include <fstream>
#include <winrt/Windows.UI.Core.h>
#include <winrt/Microsoft.Windows.Storage.Pickers.h>
#include <winrt/Microsoft.UI.Windowing.h>
#include <winrt/Windows.Foundation.Collections.h>
#if __has_include("MainWindow.g.cpp")
#include "MainWindow.g.cpp"
#endif

namespace winrt::PdfEditor::implementation
{
    MainWindow::MainWindow()
    {
        InitializeComponent();
        Title(L"pdfeditor");
#ifdef _DEBUG
        wchar_t mode[32]{};
        ::GetEnvironmentVariableW(L"PDFEDITOR_PREDICTION_MODE",mode,32);
        m_predictionEnabled=std::wstring(mode)!=L"baseline";
#endif
        auto queued=std::make_shared<std::atomic_bool>(false);
        m_predictionGate=std::make_shared<std::atomic_bool>(m_predictionEnabled);
        auto queue=DocumentCanvas().DispatcherQueue();
        m_schedulePump=[weak=get_weak(),queue,queued,gate=m_predictionGate] {
            if(!gate->load()) return;
            if(queued->exchange(true)) return;
            if(!queue.TryEnqueue(Microsoft::UI::Dispatching::DispatcherQueuePriority::Low,[weak,queued] {
                queued->store(false);
                if(auto self=weak.get()) self->PollTiles();
            })) queued->store(false);
        };
        m_pollTimer.Interval(std::chrono::milliseconds(30));
        m_pollTimer.Tick([weak = get_weak()](auto const&, auto const&) {
            if (auto self = weak.get()) self->PollTiles();
        });
        Closed([weak = get_weak()](auto const&, auto const&) {
            if (auto self = weak.get()) self->m_pollTimer.Stop();
            if (auto self = weak.get()) self->m_benchmarkTimer.Stop();
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
        if (m_core) m_core->NavigationInput(direction == L"Down" ? 1 : direction == L"Up" ? -1 : 0,m_viewport.origin_y);
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
        if (m_core) m_core->NavigationDirection(0);
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
            if (m_core) m_core->NavigationInput(point.Properties().IsHorizontalMouseWheel() ? 0 : delta < 0 ? 1 : -1,m_viewport.origin_y);
            UpdateViewport();
        }
        args.Handled(true);
    }
    void MainWindow::Document_Scroll(winrt::Windows::Foundation::IInspectable const&,
        Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const& args)
    {
        if (m_updatingScroll) return;
        if (m_core) m_core->NavigationInput(args.NewValue() > m_viewport.origin_y ? 1 : -1,args.NewValue());
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
            m_core->NavigationInput(page - 1 > m_snapshot.current_page ? 1 : page - 1 < m_snapshot.current_page ? -1 : 0,m_viewport.origin_y,true);
            UpdateViewport();
        } catch (std::exception const& error) { StatusText().Text(winrt::to_hstring(error.what())); }
    }
    void MainWindow::RefreshThumbnails()
    {
        if (!m_core || m_viewport.generation == 0) return;
        Microsoft::UI::Xaml::Media::RectangleGeometry clip;
        clip.Rect(winrt::Windows::Foundation::Rect{0, 0, static_cast<float>(ThumbnailClip().ActualWidth()), static_cast<float>(ThumbnailClip().ActualHeight())});
        ThumbnailClip().Clip(clip);
        if (!m_thumbnails) m_thumbnails = std::make_unique<ThumbnailPanel>(ThumbnailCanvas(), ThumbnailScroll(), *m_core,
            [weak = get_weak()](std::uint32_t index) { if (auto self = weak.get()) {
                const auto direction=index > self->m_snapshot.current_page ? 1 : index < self->m_snapshot.current_page ? -1 : 0;
                self->m_core->GoToPage(index, self->m_viewport);
                self->m_core->NavigationInput(direction,self->m_viewport.origin_y,true);
                self->UpdateViewport();
            }});
        m_thumbnails->Refresh(m_snapshot.current_page, m_viewport.device_pixel_ratio, m_viewport.rotation_degrees);
    }
    void MainWindow::ShowCurrent_Click(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::RoutedEventArgs const&)
    { if (m_thumbnails) m_thumbnails->ShowCurrent(); }
    void MainWindow::Thumbnails_SizeChanged(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::SizeChangedEventArgs const&)
    { RefreshThumbnails(); }
    void MainWindow::Thumbnails_Scroll(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Controls::Primitives::RangeBaseValueChangedEventArgs const& args)
    { if (m_thumbnails) m_thumbnails->Scroll(args.NewValue()); }
    void MainWindow::Thumbnails_Wheel(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Input::PointerRoutedEventArgs const& args)
    { if (m_thumbnails) { m_thumbnails->Wheel(args.GetCurrentPoint(ThumbnailClip()).Properties().MouseWheelDelta()); args.Handled(true); } }
    void MainWindow::UpdateViewport()
    {
        try {
            if (!m_core) {
                m_core = std::make_unique<NativeCoreBridge>();
                m_core->SetReadyCallback(m_schedulePump);
            }
            if (!m_renderer) m_renderer = std::make_unique<DocumentCanvasRenderer>(DocumentCanvas());
            if (m_documentPath.empty()) m_documentPath = NativeCoreBridge::P4FixturePath();
            m_core->OpenContinuousPdf(m_documentPath);
            if (!m_root) {
                m_root = DocumentCanvas().XamlRoot();
                m_rootChanged = m_root.Changed([weak = get_weak()](auto const&, auto const&) {
                    if (auto self = weak.get(); self && self->m_viewport.generation != 0 &&
                        self->m_viewport.device_pixel_ratio != self->m_root.RasterizationScale()) self->UpdateViewport();
                });
            }
            const double dpr = m_root.RasterizationScale();
            auto next = m_viewport;
            next.width = std::clamp(std::floor(DocumentCanvas().ActualWidth() * dpr), 1.0, 16384.0);
            next.height = std::clamp(std::floor(DocumentCanvas().ActualHeight() * dpr), 1.0, 16384.0);
            next.device_pixel_ratio = dpr;
            const std::uint16_t rotation = RotatePage().IsChecked().GetBoolean() ? 90 : 0;
            ++next.generation;
            if (next.rotation_degrees != rotation) {
                m_core->NavigationDirection(0);
                next.rotation_degrees = rotation;
                m_core->GoToPage(m_snapshot.current_page, next);
            }
            m_core->GpuResidency(m_renderer->ResidentKeys());
            m_core->PredictionConfigure(m_predictionEnabled,(std::min)(128ull*1024*1024, (std::max)(1ull, static_cast<unsigned long long>(m_renderer->PredictionBudget()))),256);
            m_snapshot = m_core->UpdateContinuousViewport(next, m_pages);
            next.origin_x = m_snapshot.origin_x; next.origin_y = m_snapshot.origin_y;
            m_viewport = next;
            m_updatingScroll = true;
            DocumentScroll().Minimum(0);
            DocumentScroll().Maximum((std::max)(0.0, m_snapshot.extent_height - next.height / (next.scale * dpr)));
            DocumentScroll().ViewportSize(next.height / (next.scale * dpr));
            DocumentScroll().SmallChange(128.0 / (next.scale * dpr));
            DocumentScroll().LargeChange(next.height * 0.9 / (next.scale * dpr));
            DocumentScroll().Value(next.origin_y);
            m_updatingScroll = false;
            CurrentPageText().Text(L"Page " + std::to_wstring(m_snapshot.current_page + 1) + L" / " + std::to_wstring(m_snapshot.page_count));
            std::vector<PdfeditorTileKey> required;
            m_core->Presentation(required);
            std::vector<PdfeditorTileKey> predictive;
            m_core->Prediction(predictive);
            m_renderer->SetViewport(m_viewport, m_pages, required,predictive);
            LogPrediction("request");
            TryCommitPresentation();
            if(m_predictionEnabled) m_schedulePump();
            RefreshThumbnails();
            UpdateEditorControls();
            m_pollTimer.Start();
        } catch (std::exception const& error) {
            m_updatingScroll = false;
            StatusText().Text(winrt::to_hstring(std::string("P4 viewport failed: ") + error.what()));
        }
    }
    void MainWindow::UpdateEditorControls() {
        const auto e = m_core->EditorStatus();
        SelectionText().Text(std::to_wstring(e.selected_count) + L" pages selected | " + (e.structural_dirty ? L"STRUCTURAL_DIRTY" : L"CLEAN"));
        DuplicatePages().IsEnabled(e.selected_count != 0);
        ExtractPages().IsEnabled(e.selected_count != 0);
        DeletePages().IsEnabled(e.selected_count != 0 && e.selected_count < e.page_count);
        MovePages().IsEnabled(e.selected_count != 0); RotateLeft().IsEnabled(e.selected_count != 0); RotateRight().IsEnabled(e.selected_count != 0);
        UndoEdit().IsEnabled(e.undo_depth != 0); RedoEdit().IsEnabled(e.redo_depth != 0);
    }
    void MainWindow::ApplyEdit(std::uint32_t command, std::int32_t argument) {
        try {
            if (!m_core || m_viewport.generation == 0) return;
            const auto before = m_core->EditorStatus();
            double offset = 0;
            for (auto const& p : m_pages) if (p.page_id == before.current_page_id) offset = m_viewport.origin_y - p.y;
            const auto after = m_core->Edit(command, argument);
            EditMessage().Text(L"");
            if (after.revision != before.revision) {
                m_renderer->InvalidateFrame();
                m_core->GoToPage(after.current_index, m_viewport);
                m_viewport.origin_y += offset;
            }
            UpdateViewport();
        } catch (std::exception const& error) { EditMessage().Text(winrt::to_hstring(error.what())); }
    }
    void MainWindow::Edit_Click(winrt::Windows::Foundation::IInspectable const& sender, Microsoft::UI::Xaml::RoutedEventArgs const&) {
        const auto tag = winrt::unbox_value<winrt::hstring>(sender.as<Microsoft::UI::Xaml::Controls::Button>().Tag());
        if (tag == L"Duplicate") ApplyEdit(7);
        if (tag == L"Delete") ApplyEdit(1);
        if (tag == L"RotateLeft") ApplyEdit(3, -90);
        if (tag == L"RotateRight") ApplyEdit(3, 90);
        if (tag == L"Undo") ApplyEdit(4);
        if (tag == L"Redo") ApplyEdit(5);
        if (tag == L"Move") {
            try {
                const std::wstring text(MoveInput().Text()); std::size_t used{};
                const auto page = std::stoull(text, &used);
                if (used != text.size() || page == 0 || page > m_snapshot.page_count + 1ull) throw std::runtime_error("Move Before expects 1 through page count + 1 (end)");
                ApplyEdit(2, static_cast<std::int32_t>(page - 1));
            } catch (std::exception const& error) { EditMessage().Text(winrt::to_hstring(error.what())); }
        }
    }
    winrt::fire_and_forget MainWindow::Output_Click(winrt::Windows::Foundation::IInspectable const& sender, Microsoft::UI::Xaml::RoutedEventArgs const&) {
        auto lifetime = get_strong();
        try {
            if (!m_core || m_viewport.generation == 0) UpdateViewport();
            EditMessage().Text(L"");
            const auto tag = winrt::unbox_value<winrt::hstring>(sender.as<Microsoft::UI::Xaml::Controls::Button>().Tag());
            if (tag == L"Cancel") { m_core->CancelOutput(); co_return; }
            using namespace Microsoft::Windows::Storage::Pickers;
            if (tag == L"Insert" || tag == L"Open") {
                FileOpenPicker picker(AppWindow().Id());
                picker.FileTypeFilter().Append(L".pdf");
                auto file = co_await picker.PickSingleFileAsync();
                if (!file) co_return;
                if (tag == L"Open") {
                    m_core->OpenContinuousPdf(std::filesystem::path(file.Path().c_str()));
                    m_documentPath = std::filesystem::path(file.Path().c_str());
                    // Revision/selection counters can match across different documents.
                    // Rebuild demand and cards so no previous document's pixels survive.
                    ThumbnailCanvas().Children().Clear();
                    m_thumbnails.reset();
                    m_viewport.origin_x = 0; m_viewport.origin_y = 0;
                    m_renderer->InvalidateFrame(); UpdateViewport(); co_return;
                }
                const auto after = m_core->InsertPdf(std::filesystem::path(file.Path().c_str()), m_core->EditorStatus().current_index + 1);
                m_renderer->InvalidateFrame(); m_core->GoToPage(after.current_index, m_viewport);
                UpdateViewport(); OutputText().Text(L"PDF pages inserted after current page"); co_return;
            }
            FileSavePicker picker(AppWindow().Id());
            picker.SuggestedFileName(tag == L"Extract" ? L"extracted.pdf" : L"edited.pdf");
            picker.FileTypeChoices().Insert(L"PDF document", winrt::single_threaded_vector<winrt::hstring>({L".pdf"}));
            auto file = co_await picker.PickSaveFileAsync();
            if (!file) co_return;
            const std::filesystem::path path(file.Path().c_str());
            bool overwrite = false;
            if (std::filesystem::exists(path)) {
                Microsoft::UI::Xaml::Controls::ContentDialog dialog;
                dialog.XamlRoot(DocumentCanvas().XamlRoot()); dialog.Title(winrt::box_value(L"Replace existing PDF?"));
                dialog.Content(winrt::box_value(L"The existing destination will be replaced only after the new PDF passes verification."));
                dialog.PrimaryButtonText(L"Replace"); dialog.CloseButtonText(L"Cancel");
                if (co_await dialog.ShowAsync() != Microsoft::UI::Xaml::Controls::ContentDialogResult::Primary) co_return;
                overwrite = true;
            }
            m_core->StartOutput(path, tag == L"Extract" ? 2u : 1u, overwrite);
        } catch (winrt::hresult_error const& error) { EditMessage().Text(error.message()); }
          catch (std::exception const& error) { EditMessage().Text(winrt::to_hstring(error.what())); }
    }
    void MainWindow::Editor_KeyDown(winrt::Windows::Foundation::IInspectable const&, Microsoft::UI::Xaml::Input::KeyRoutedEventArgs const& args) {
        using winrt::Windows::System::VirtualKey;
        using winrt::Windows::UI::Core::CoreVirtualKeyStates;
        if (!m_core) return;
        // Preserve native text editing shortcuts while either numeric box has focus.
        const auto focus = Microsoft::UI::Xaml::Input::FocusManager::GetFocusedElement(DocumentCanvas().XamlRoot());
        if (focus && focus.try_as<Microsoft::UI::Xaml::Controls::TextBox>()) return;
        const auto ctrl = (Microsoft::UI::Input::InputKeyboardSource::GetKeyStateForCurrentThread(VirtualKey::Control) & CoreVirtualKeyStates::Down) != CoreVirtualKeyStates::None;
        if (args.Key() == VirtualKey::Delete && !ctrl) { ApplyEdit(1); args.Handled(true); }
        if (ctrl && args.Key() == VirtualKey::Z) { ApplyEdit(4); args.Handled(true); }
        if (ctrl && args.Key() == VirtualKey::Y) { ApplyEdit(5); args.Handled(true); }
        if (ctrl && args.Key() == VirtualKey::A) { ApplyEdit(6); args.Handled(true); }
#ifdef _DEBUG
        if(ctrl && args.Key()==VirtualKey::P) {
            m_predictionEnabled=!m_predictionEnabled; m_predictionGate->store(m_predictionEnabled);
            m_core->NavigationDirection(0); UpdateViewport(); args.Handled(true);
        }
        if(ctrl && args.Key()==VirtualKey::B) { StartNavigationBenchmark(); args.Handled(true); }
        // Exact-scale developer acceptance controls, outside editable text boxes.
        if (ctrl && (args.Key() == VirtualKey::Number1 || args.Key() == VirtualKey::Number2 || args.Key() == VirtualKey::Number4)) {
            const auto scale = args.Key() == VirtualKey::Number1 ? 1.0 : args.Key() == VirtualKey::Number2 ? 2.0 : 4.0;
            ZoomAt(scale / m_viewport.scale, m_viewport.width / 2, m_viewport.height / 2);
            args.Handled(true);
        }
#endif
    }
    void MainWindow::PollTiles()
    {
        if (!m_core || !m_renderer || m_viewport.generation == 0) return;
        try {
            if (m_core->LayoutNeedsRefresh()) UpdateViewport();
            const auto uploadStarted = std::chrono::steady_clock::now();
            // Drain mandatory uploads promptly, with bounded work per UI tick.
            for (int i = 0; i < 16 && std::chrono::steady_clock::now() - uploadStarted < std::chrono::milliseconds(8); ++i) {
                if (!m_core->PollReady([&](PdfeditorReadyTile const& tile) {
                    if (tile.generation == m_viewport.generation) {
                        m_renderer->CacheTile(tile);
                    }
                })) break;
            }
            TryCommitPresentation();
            // A separate queue/poll makes mandatory upload preemption explicit.
            const auto predictiveStarted=std::chrono::steady_clock::now();
            for(int i=0;m_predictionEnabled && i<4 && std::chrono::steady_clock::now()-predictiveStarted<std::chrono::milliseconds(2) && std::chrono::steady_clock::now()-uploadStarted<std::chrono::milliseconds(8);++i) {
                if(!m_core->PollReady([&](PdfeditorReadyTile const& tile) { m_renderer->CacheTile(tile); },true)) break;
            }
            const auto uploadUs=static_cast<std::uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now()-uploadStarted).count());
            m_uploadMaxUs=(std::max)(m_uploadMaxUs,uploadUs);
            TryCommitPresentation();
            LogPrediction("pump",uploadUs);
            if (m_thumbnails) m_thumbnails->Poll();
            const auto output = m_core->OutputStatus();
            static wchar_t const* phases[] = { L"Idle", L"Snapshotting", L"Opening sources", L"Building document", L"Writing", L"Verifying", L"Finalizing", L"Succeeded", L"Failed", L"Cancel requested", L"Cancelled" };
            OutputText().Text(std::wstring(phases[(std::min)(output.phase, 10u)]) + L" " + std::to_wstring(output.percent) + L"% | " + std::to_wstring(output.page_count) + L" pages, " + std::to_wstring(output.registered_sources) + L" sources | " + winrt::to_hstring(output.error_utf8));
            const bool busy = (output.phase >= 1 && output.phase <= 6) || output.phase == 9;
            OpenDocument().IsEnabled(!busy);
            SaveDocument().IsEnabled(!busy); CancelOutput().IsEnabled(busy);
            UpdateEditorControls();
            ExtractPages().IsEnabled(!busy && m_core->EditorStatus().selected_count != 0);
            const auto m = m_core->Metrics();
            m_cpuPeak=(std::max)(m_cpuPeak,static_cast<std::uint64_t>(m.cpu_cache_bytes));
            const auto e = m_core->EditorStatus();
            std::vector<PdfeditorTileKey> keys;
            const auto p = m_core->Presentation(keys);
#ifdef _DEBUG
            std::vector<PdfeditorTileKey> predicted;
            const auto prediction=m_core->Prediction(predicted);
#endif
            StatusText().Text(L"Document Y " + std::to_wstring(m_viewport.origin_y) + L", extent " + std::to_wstring(m_snapshot.extent_height) +
                L", zoom " + std::to_wstring(m_viewport.scale) + L", generation " + std::to_wstring(m_viewport.generation) +
                L" | visible pages/tiles " + std::to_wstring(m_snapshot.visible_pages) + L" / " + std::to_wstring(m_snapshot.visible_tiles) +
                L", known " + std::to_wstring(m_snapshot.known_pages) + L", metadata bytes " + std::to_wstring(m_snapshot.metadata_bytes) +
                L" | open/layout us " + std::to_wstring(m_snapshot.open_micros) + L" / " + std::to_wstring(m_snapshot.layout_init_micros) +
                L", geometry queries/us " + std::to_wstring(m_snapshot.geometry_queries) + L" / " + std::to_wstring(m_snapshot.geometry_micros) +
                L" | CPU/GPU bytes " + std::to_wstring(m.cpu_cache_bytes) + L" / " + std::to_wstring(m_renderer->GpuBytes()) +
                L", queue/ready " + std::to_wstring(m.queue_depth) + L" / " + std::to_wstring(m.completion_depth) +
                L", renders/hits/stale " + std::to_wstring(m.renders_performed) + L" / " + std::to_wstring(m.cache_hits) + L" / " + std::to_wstring(m.stale_renders_discarded) +
                L", GPU uploads/reuse " + std::to_wstring(m_renderer->GpuUploads()) + L" / " + std::to_wstring(m_renderer->GpuHits()) +
                L" | plan rev " + std::to_wstring(e.revision) + L", current ID/index " + std::to_wstring(e.current_page_id) + L"/" + std::to_wstring(e.current_index) +
                L", undo/redo " + std::to_wstring(e.undo_depth) + L"/" + std::to_wstring(e.redo_depth) + L", history bytes " + std::to_wstring(e.history_bytes) +
                L"\nAtomic requested page/Y/scale/gen " + std::to_wstring(p.requested_page + 1) + L"/" + std::to_wstring(p.requested.origin_y) + L"/" + std::to_wstring(p.requested.scale) + L"/" + std::to_wstring(p.requested.generation) +
                L"; displayed " + std::to_wstring(p.displayed_page + 1) + L"/" + std::to_wstring(p.displayed.origin_y) + L"/" + std::to_wstring(p.displayed.scale) + L"/" + std::to_wstring(p.displayed.generation) +
                L" | required/CPU/GPU/missing " + std::to_wstring(p.required_count) + L"/" + std::to_wstring(p.cpu_count) + L"/" + std::to_wstring(p.gpu_count) + L"/" + std::to_wstring(p.missing_count) +
                L", commits/stale/coalesced/partial " + std::to_wstring(p.commit_count) + L"/" + std::to_wstring(p.stale_destinations) + L"/" + std::to_wstring(p.coalesced_requests) + L"/" + std::to_wstring(p.partial_presentations) +
                L", request-to-start/CPU/GPU/commit us " + std::to_wstring(p.render_started_at ? p.render_started_at - p.requested_at : 0) + L"/" + std::to_wstring(p.cpu_ready_at ? p.cpu_ready_at - p.requested_at : 0) + L"/" + std::to_wstring(p.gpu_ready_at ? p.gpu_ready_at - p.requested_at : 0) + L"/" + std::to_wstring(p.committed_at ? p.committed_at - p.requested_at : 0) +
                L", hold us " + std::to_wstring(p.hold_micros) + (m_renderer->MemoryBlocked() ? L" | destination exceeds fixed GPU reserve; request a smaller viewport" : L"") +
#ifdef _DEBUG
                L"\nPredict " + std::wstring(m_predictionEnabled ? L"adaptive " : L"baseline ")+L"direction/velocity/latency us/lead/depth " + std::to_wstring(prediction.direction)+L"/"+std::to_wstring(prediction.velocity)+L"/"+std::to_wstring(prediction.preparation_us)+L"/"+std::to_wstring(prediction.lead_distance)+L"/"+std::to_wstring(prediction.depth)+
                L" | tiles/CPU/GPU " + std::to_wstring(prediction.tile_count)+L"/"+std::to_wstring(prediction.cpu_ready)+L"/"+std::to_wstring(prediction.gpu_ready)+
                L" | entry GPU/required/predicted " + std::to_wstring(prediction.entry_gpu)+L"/"+std::to_wstring(prediction.entry_required)+L"/"+std::to_wstring(prediction.entry_predictive_gpu)+
                L" | prediction CPU/GPU used " + std::to_wstring(prediction.cpu_used)+L"/"+std::to_wstring(prediction.gpu_used)+L", waste " + std::to_wstring(prediction.cpu_wasted)+L"/"+std::to_wstring(prediction.gpu_wasted)+
                L" | CPU/GPU useful % " + std::to_wstring(prediction.cpu_completed ? 100.0*prediction.cpu_used/prediction.cpu_completed : 0.0)+L"/"+std::to_wstring(prediction.gpu_uploaded ? 100.0*prediction.gpu_used/prediction.gpu_uploaded : 0.0)+
                L", GPU waste % " + std::to_wstring(prediction.gpu_uploaded ? 100.0*prediction.gpu_wasted/prediction.gpu_uploaded : 0.0)+
                L" | predictive bytes " + std::to_wstring(m_renderer->PredictiveBytes())+L", upload max us " + std::to_wstring(m_uploadMaxUs)+
#endif
                L"\n" + (m_thumbnails ? m_thumbnails->MetricsText() : L""));
            if(m_predictionEnabled && !m_renderer->MemoryBlocked() && m.completion_depth!=0 &&
                (p.state==0 || (p.cpu_count>p.gpu_count && p.gpu_count<p.required_count))) m_schedulePump();
        } catch (std::exception const& error) {
            m_pollTimer.Stop(); StatusText().Text(winrt::to_hstring(std::string("P4 completion failed: ") + error.what()));
        }
    }
    void MainWindow::TryCommitPresentation()
    {
        m_core->GpuResidency(m_renderer->ResidentKeys());
        std::vector<PdfeditorTileKey> required;
        auto p = m_core->Presentation(required);
        if (p.state != 2 || m_renderer->MemoryBlocked()) return;
        if (!m_renderer->ComposeViewport(DocumentCanvas().CompositionScaleX(), DocumentCanvas().CompositionScaleY())) return;
        m_core->CommitPresentation(p.requested.generation);
        p = m_core->Presentation(required);
        LogPrediction("commit");
        // Developer-only optional CSV records every successful native commit.
        wchar_t path[32768]{};
        const auto length = ::GetEnvironmentVariableW(L"PDFEDITOR_PRESENTATION_LOG", path, 32768);
        bool record = length > 0 && length < 32768;
#ifdef _DEBUG
        if (!record) {
            const auto exeLength = ::GetModuleFileNameW(nullptr, path, 32768);
            if (exeLength > 0 && exeLength < 32768) {
                auto file = std::filesystem::path(path).parent_path() / L"pdfeditor-presentation.csv";
                const auto text = file.wstring();
                if (text.size() < 32768) { std::copy(text.begin(), text.end(), path); path[text.size()] = 0; record = true; }
            }
        }
#endif
        if (record) {
            std::ofstream log(std::filesystem::path(path), std::ios::app);
            log << p.requested.generation << ',' << p.requested_page << ',' << p.requested.scale << ','
                << p.requested_at << ',' << p.render_started_at << ',' << p.cpu_ready_at << ','
                << p.gpu_ready_at << ',' << p.committed_at << ',' << p.hold_micros << ','
                << p.required_count << ',' << p.partial_presentations << ','
                << p.requested.origin_y << ',' << p.requested.rotation_degrees << ','
                << p.coalesced_requests << ',' << (required.empty() ? 0 : required.front().document_id) << '\n';
        }
    }
    void MainWindow::LogPrediction(char const* event,std::uint64_t uploadUs) {
#ifdef _DEBUG
        if(!m_core || !m_renderer) return;
        std::vector<PdfeditorTileKey> keys;
        const auto p=m_core->Presentation(keys);
        const auto q=m_core->Prediction(keys);
        const auto m=m_core->Metrics();
        m_cpuPeak=(std::max)(m_cpuPeak,static_cast<std::uint64_t>(m.cpu_cache_bytes));
        wchar_t exe[32768]{};
        const auto length=::GetModuleFileNameW(nullptr,exe,32768);
        if(!length || length>=32768) return;
        const auto path=std::filesystem::path(exe).parent_path()/L"prediction-events.csv";
        const bool header=!std::filesystem::exists(path) || std::filesystem::file_size(path)==0;
        std::ofstream log(path,std::ios::app);
        if(header) log<<"event,input,generation,page,y,scale,direction,velocity,estimate_us,lead,depth,required,entry_cpu,entry_gpu,entry_predictive_cpu,entry_predictive_gpu,mandatory_rendered,mandatory_uploaded,latency_us,hold_us,partial,predictive_tiles,predictive_cpu,predictive_gpu,cpu_completed,gpu_uploaded,cpu_used,gpu_used,cpu_wasted,gpu_wasted,invalidated,renders,uploads,cpu_peak,gpu_peak,pump_us,gpu_bytes,predictive_bytes,predicted_at,predicted_cpu_at,predicted_gpu_at,requested_at,wall_us,benchmark_step,mode\n";
        const auto wall=std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now().time_since_epoch()).count();
        log<<event<<','<<q.input_sequence<<','<<p.requested.generation<<','<<p.requested_page<<','<<p.requested.origin_y<<','<<p.requested.scale<<','
            <<q.direction<<','<<q.velocity<<','<<q.preparation_us<<','<<q.lead_distance<<','<<q.depth<<','<<q.entry_required<<','<<q.entry_cpu<<','<<q.entry_gpu<<','
            <<q.entry_predictive_cpu<<','<<q.entry_predictive_gpu<<','<<q.mandatory_rendered<<','<<q.mandatory_uploaded<<','
            <<(p.committed_at ? p.committed_at-p.requested_at : 0)<<','<<p.hold_micros<<','<<p.partial_presentations<<','
            <<q.tile_count<<','<<q.cpu_ready<<','<<q.gpu_ready<<','<<q.cpu_completed<<','<<q.gpu_uploaded<<','<<q.cpu_used<<','<<q.gpu_used<<','
            <<q.cpu_wasted<<','<<q.gpu_wasted<<','<<q.invalidated<<','<<m.renders_performed<<','<<m_renderer->GpuUploads()<<','<<m_cpuPeak<<','<<m_renderer->GpuPeak()<<','
            <<uploadUs<<','<<m_renderer->GpuBytes()<<','<<m_renderer->PredictiveBytes()<<','<<q.predicted_at<<','<<q.predicted_cpu_at<<','<<q.predicted_gpu_at<<','
            <<q.requested_at<<','<<wall<<','<<m_benchmarkStep<<','<<(m_predictionEnabled ? "adaptive" : "baseline")<<'\n';
#else
        (void)event; (void)uploadUs;
#endif
    }
    void MainWindow::StartNavigationBenchmark() {
#ifdef _DEBUG
        if(!m_core || m_snapshot.page_count==0 || m_benchmarkStep>=0) return;
        // Repeatable native render/upload/Present comparison; no simulated textures.
        // Actual mouse-wheel checks remain separate from this developer trace.
        m_core->GoToPage((std::min)(199u,m_snapshot.page_count-1),m_viewport);
        m_core->NavigationInput(0,m_viewport.origin_y,true); UpdateViewport();
        m_benchmarkStepY=240.0/(m_viewport.scale*m_viewport.device_pixel_ratio);
        m_benchmarkStep=0;
        // A fresh timer prevents repeated runs from accumulating Tick handlers.
        m_benchmarkTimer=Microsoft::UI::Xaml::DispatcherTimer();
        m_benchmarkTimer.Interval(std::chrono::milliseconds(500));
        m_benchmarkTimer.Tick([weak=get_weak()](auto const&,auto const&) {
            if(auto self=weak.get()) {
                const auto step=self->m_benchmarkStep;
                if(step<0) return;
                if(step>=192) {self->m_benchmarkTimer.Stop(); self->LogPrediction("benchmark_done"); self->m_benchmarkStep=-1; return;}
                const int phase=step/24;
                const int direction=phase==1 || phase==6 ? -1 : 1;
                // cold forward, immediate reverse, warm forward, slow, medium,
                // fast, reversal, then a distant coalesced jump and forward motion.
                const int cadence=phase==3 ? 450 : phase==5 ? 70 : 180;
                self->m_benchmarkTimer.Interval(std::chrono::milliseconds(cadence));
                if(phase==7 && step%24==0) {
                    self->m_core->GoToPage((std::min)(749u,self->m_snapshot.page_count-1),self->m_viewport);
                    self->m_core->NavigationInput(1,self->m_viewport.origin_y,true);
                } else {
                    self->m_viewport.origin_y+=direction*self->m_benchmarkStepY;
                    self->m_core->NavigationInput(direction,self->m_viewport.origin_y);
                }
                self->UpdateViewport(); ++self->m_benchmarkStep;
            }
        });
        m_benchmarkTimer.Start();
#endif
    }
}
