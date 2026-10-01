#pragma once
#include <cstddef>
#include <algorithm>

// Shared by the real Direct3D texture cache and deterministic native tests.
namespace pdfeditor {
    constexpr std::size_t presentationReserveBytes = 128ull * 1024 * 1024;
    constexpr std::size_t presentationReserveCount = 128;
    constexpr std::size_t maximumResidentCount = 512;
    inline std::size_t PresentationCountLimit(std::size_t budget) {
        return (std::min)(budget + presentationReserveCount, maximumResidentCount);
    }
    template<class Keys> bool FitsPresentation(Keys const& pending, Keys const& displayed, std::size_t bytes, std::size_t count) {
        auto pinned = displayed;
        pinned.insert(pending.begin(), pending.end());
        std::size_t total{};
        for (auto const& k : pinned) total += static_cast<std::size_t>(k.width) * k.height * 4;
        return total <= bytes + presentationReserveBytes && pinned.size() <= PresentationCountLimit(count);
    }
    template<class Cache, class Keys> void EvictUnpinned(Cache& cache, Keys const& pending, Keys const& displayed,
        std::size_t& residentBytes, std::size_t bytes, std::size_t count) {
        while (residentBytes > bytes || cache.size() > count) {
            auto victim = cache.end();
            for (auto it = cache.begin(); it != cache.end(); ++it) {
                if (pending.contains(it->first) || displayed.contains(it->first)) continue;
                if (victim == cache.end() || it->second.touched < victim->second.touched) victim = it;
            }
            if (victim == cache.end()) break;
            residentBytes -= victim->second.bytes; cache.erase(victim);
        }
    }
    // Predictive pins are a preference only: ordinary entries first, then
    // predictive entries, always protecting displayed/pending mandatory content.
    template<class Cache, class Keys> void EvictWithPrediction(Cache& cache, Keys const& pending, Keys const& displayed,
        Keys const& predictive, std::size_t& residentBytes, std::size_t bytes, std::size_t count) {
        while (residentBytes > bytes || cache.size() > count) {
            auto victim = cache.end();
            for (auto it = cache.begin(); it != cache.end(); ++it) {
                if (pending.contains(it->first) || displayed.contains(it->first)) continue;
                if (victim == cache.end() ||
                    (predictive.contains(victim->first) && !predictive.contains(it->first)) ||
                    (predictive.contains(victim->first) == predictive.contains(it->first) && it->second.touched < victim->second.touched)) victim = it;
            }
            if (victim == cache.end()) break;
            residentBytes -= victim->second.bytes; cache.erase(victim);
        }
    }
}
