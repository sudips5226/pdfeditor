#include "pch.h"
#include "ThumbnailPanel.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <robuffer.h>
#include <winrt/Windows.Storage.Streams.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.UI.h>
#include <winrt/Microsoft.UI.Xaml.Automation.h>

namespace winrt::PdfEditor::implementation
{
    namespace {
        bool SameKey(PdfeditorThumbnailKey const& a, PdfeditorThumbnailKey const& b) {
            return a.document_id == b.document_id && a.revision == b.revision && a.page_id == b.page_id &&
                a.dpr_bits == b.dpr_bits && a.width == b.width && a.height == b.height && a.flags == b.flags && a.rotation == b.rotation;
        }
    }
    ThumbnailPanel::ThumbnailPanel(Microsoft::UI::Xaml::Controls::Canvas const& canvas,
        Microsoft::UI::Xaml::Controls::Primitives::ScrollBar const& scroll, NativeCoreBridge& core,
        std::function<void(std::uint32_t)> navigate) : m_canvas(canvas), m_scroll(scroll), m_core(core), m_navigate(std::move(navigate)) {}

    std::unique_ptr<ThumbnailPanel::Slot> ThumbnailPanel::CreateSlot()
    {
        using namespace Microsoft::UI::Xaml;
        using namespace Microsoft::UI::Xaml::Controls;
        auto slot = std::make_unique<Slot>();
        slot->button.Width(m_view.width + 12); slot->button.Height(m_view.height + m_view.label_height);
        slot->button.Padding(Thickness{0}); slot->button.BorderThickness(Thickness{2});
        Grid grid;
        RowDefinition imageRow; imageRow.Height(GridLength{1, GridUnitType::Star});
        RowDefinition labelRow; labelRow.Height(GridLength{m_view.label_height, GridUnitType::Pixel});
        grid.RowDefinitions().Append(imageRow); grid.RowDefinitions().Append(labelRow);
        slot->image.Width(m_view.width); slot->image.Height(m_view.height - 4);
        slot->image.Stretch(Media::Stretch::Uniform);
        slot->placeholder.Text(L"Loading"); slot->placeholder.HorizontalAlignment(HorizontalAlignment::Center);
        slot->placeholder.VerticalAlignment(VerticalAlignment::Center);
        slot->label.HorizontalAlignment(HorizontalAlignment::Center);
        Grid::SetRow(slot->label, 1);
        grid.Children().Append(slot->image); grid.Children().Append(slot->placeholder); grid.Children().Append(slot->label);
        slot->button.Content(grid);
        auto raw = slot.get();
        slot->button.Click([this, raw](auto const&, auto const&) {
            // Resolve the current slot's logical index; no source-index assumption or rendering wait.
            m_navigate(raw->item.index);
        });
        m_canvas.Children().Append(slot->button);
        return slot;
    }
    void ThumbnailPanel::Clear(Slot& slot)
    {
        slot.image.Source(nullptr); m_bytes -= slot.bytes; slot.bytes = 0;
        slot.placeholder.Text(L"Loading"); slot.placeholder.Visibility(Microsoft::UI::Xaml::Visibility::Visible);
    }
    void ThumbnailPanel::Highlight()
    {
        using namespace Microsoft::UI::Xaml::Media;
        for (auto const& s : m_slots) {
            s->item.current = s->item.index == m_view.current;
            s->button.BorderBrush(SolidColorBrush(s->item.current ? winrt::Windows::UI::Color{255,0,100,220} : winrt::Windows::UI::Color{255,128,128,128}));
            s->button.Background(SolidColorBrush(s->item.current ? winrt::Windows::UI::Color{255,210,230,255} : winrt::Windows::UI::Color{255,245,245,245}));
        }
    }
    void ThumbnailPanel::Refresh(std::uint32_t current, double dpr, std::uint16_t rotation)
    {
        const auto extent = (std::max)(1.0, m_canvas.ActualHeight());
        const bool demandChanged = m_view.generation == 0 || m_view.extent != extent || m_view.dpr != dpr || m_view.rotation != rotation;
        if (m_view.current != current) m_core.SyncThumbnailCurrent(current);
        m_view.current = current; m_view.extent = extent; m_view.dpr = dpr; m_view.rotation = rotation;
        if (demandChanged) Update(); else Highlight();
    }
    void ThumbnailPanel::Update()
    {
        using namespace Microsoft::UI::Xaml::Controls;
        std::vector<PdfeditorThumbnailItem> items;
        auto next = m_view; ++next.generation;
        const auto snapshot = m_core.UpdateThumbnails(next, items);
        next.offset = snapshot.offset; m_view = next; m_snapshot = snapshot;
        std::vector<std::unique_ptr<Slot>> slots(items.size());
        // Preserve still-demanded image resources by identity first, recycle the remaining cards.
        for (std::size_t i = 0; i < items.size(); ++i) {
            for (auto& old : m_slots) if (old && SameKey(old->item.key, items[i].key) && old->item.recycle == items[i].recycle) {
                slots[i] = std::move(old); break;
            }
        }
        for (std::size_t i = 0; i < items.size(); ++i) {
            if (!slots[i]) {
                for (auto& old : m_slots) if (old) { slots[i] = std::move(old); Clear(*slots[i]); ++m_recycled; break; }
                if (!slots[i]) slots[i] = CreateSlot();
            }
            auto& slot = *slots[i]; slot.item = items[i];
            if (slot.item.visible == 0) Clear(slot); // only visible cards retain native bitmap payload
            slot.label.Text(std::to_wstring(slot.item.index + 1));
            Microsoft::UI::Xaml::Automation::AutomationProperties::SetName(slot.button, L"Thumbnail page " + std::to_wstring(slot.item.index + 1));
            Canvas::SetLeft(slot.button, m_view.padding);
            Canvas::SetTop(slot.button, slot.item.top - m_view.offset);
        }
        for (auto& old : m_slots) if (old) {
            Clear(*old); std::uint32_t index{};
            if (m_canvas.Children().IndexOf(old->button, index)) m_canvas.Children().RemoveAt(index);
        }
        m_slots = std::move(slots); Highlight();
        m_updating = true;
        m_scroll.Maximum((std::max)(0.0, snapshot.total - m_view.extent));
        m_scroll.ViewportSize(m_view.extent); m_scroll.SmallChange(100); m_scroll.LargeChange(m_view.extent * 0.9);
        m_scroll.Value(m_view.offset); m_updating = false;
    }
    void ThumbnailPanel::Scroll(double offset) { if (m_updating) return; m_view.offset = offset; Update(); }
    void ThumbnailPanel::Wheel(double delta) { Scroll((std::max)(0.0, m_view.offset - delta * 2)); }
    void ThumbnailPanel::ShowCurrent() { Scroll(m_core.ShowCurrentThumbnail(m_view)); }
    void ThumbnailPanel::Poll()
    {
        using namespace Microsoft::UI::Xaml;
        for (int i = 0; i < 2; ++i) if (!m_core.PollThumbnail([&](PdfeditorReadyThumbnail const& ready) {
            const auto match = std::find_if(m_slots.begin(), m_slots.end(), [&](auto const& s) {
                return s->item.visible && s->item.recycle == ready.recycle && SameKey(s->item.key, ready.key);
            });
            if (ready.generation != m_view.generation || match == m_slots.end()) { ++m_rejected; return; }
            auto& slot = **match;
            if (ready.status != 0) { Clear(slot); slot.placeholder.Text(L"Unavailable"); return; }
            if (slot.bytes != 0) return;
            if (ready.len > NativeBudget - m_bytes) { slot.placeholder.Text(L"Image budget"); return; }
            Media::Imaging::WriteableBitmap bitmap(static_cast<std::int32_t>(ready.key.width), static_cast<std::int32_t>(ready.key.height));
            std::uint8_t* data{};
            winrt::check_hresult(bitmap.PixelBuffer().as<::Windows::Storage::Streams::IBufferByteAccess>()->Buffer(&data));
            std::memcpy(data, ready.data, ready.len); bitmap.Invalidate();
            slot.image.Source(bitmap); slot.bytes = ready.len; m_bytes += ready.len; ++m_uploads;
            slot.placeholder.Visibility(Visibility::Collapsed);
        })) break;
    }
    std::wstring ThumbnailPanel::MetricsText() const
    {
        const auto m = m_core.ThumbnailMetrics();
        return L"Thumb visible/slots " + std::to_wstring(m_snapshot.visible) + L"/" + std::to_wstring(m_slots.size()) +
            L", queue " + std::to_wstring(m.queue) + L", hits/misses/renders " + std::to_wstring(m.hits) + L"/" + std::to_wstring(m.misses) + L"/" + std::to_wstring(m.renders) +
            L", cache/native bytes " + std::to_wstring(m.bytes) + L"/" + std::to_wstring(m_bytes) + L", uploads/recycle/stale " +
            std::to_wstring(m_uploads) + L"/" + std::to_wstring(m_recycled) + L"/" + std::to_wstring(m.stale + m_rejected) + L", current " + std::to_wstring(m_view.current + 1);
    }
}
