#include "../native/winui/GpuPresentationPolicy.h"
#include <cassert>
#include <map>
#include <memory>
#include <set>
#include <iostream>
struct Key {
    int id; unsigned width = 512, height = 512;
    bool operator<(Key const& b) const { return id < b.id; }
};
struct Texture { std::size_t bytes, touched; std::shared_ptr<int> resource; };
int main() {
    constexpr std::size_t tileBytes = 512 * 512 * 4;
    std::map<Key, Texture> cache;
    std::set<Key> displayed{{0}}, pending{{1}, {2}};
    for (int i = 0; i != 4; ++i) cache.emplace(Key{i}, Texture{tileBytes, static_cast<std::size_t>(i), std::make_shared<int>(i)});
    auto old = std::weak_ptr<int>(cache.at(Key{0}).resource);
    auto unrelated = std::weak_ptr<int>(cache.at(Key{3}).resource);
    std::size_t bytes = 4 * tileBytes;
    assert(pdfeditor::FitsPresentation(pending, displayed, tileBytes, 1));
    pdfeditor::EvictUnpinned(cache, pending, displayed, bytes, tileBytes, 1);
    assert(cache.size() == 3 && bytes == 3 * tileBytes);
    assert(!old.expired() && unrelated.expired()); // old + all pending pinned, unrelated evicted first
    assert(bytes <= tileBytes + pdfeditor::presentationReserveBytes);
    displayed = pending; // successful commit releases old pins
    pdfeditor::EvictUnpinned(cache, pending, displayed, bytes, tileBytes, 1);
    assert(old.expired() && cache.size() == 2); // no dangling cache resource references
    pending.clear(); for (int i = 0; i != 130; ++i) pending.insert(Key{i});
    assert(!pdfeditor::FitsPresentation(pending, displayed, tileBytes, 1)); // deterministic hard cap
    pending.clear(); pending.insert(Key{200, 8192, 8192});
    assert(!pdfeditor::FitsPresentation(pending, displayed, tileBytes, 1));
    std::cout << "GPU pinning, LRU, reserve bounds, oversized fallback and release passed\n";
}
