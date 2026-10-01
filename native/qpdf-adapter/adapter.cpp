// Private structural boundary. No editing, paths policy, or finalization here.
#include <qpdf/QPDF.hh>
#include <qpdf/QPDFWriter.hh>
#include <qpdf/QPDFPageDocumentHelper.hh>
#include <qpdf/QPDFAcroFormDocumentHelper.hh>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <memory>
#include <stdexcept>
#include <vector>

struct OutputPage { uint32_t source; uint32_t index; uint16_t rotation; };
static_assert(sizeof(OutputPage) == 12);
using Progress = void (__cdecl*)(void*, uint32_t, uint32_t);
static void error_text(char* out, size_t cap, char const* message) noexcept {
    if (out && cap) { auto n = (std::min)(cap - 1, std::strlen(message)); std::memcpy(out, message, n); out[n] = 0; }
}
extern "C" __declspec(dllexport) uint32_t __cdecl pdfeditor_qpdf_adapter_version() { return 1; }
extern "C" __declspec(dllexport) int __cdecl pdfeditor_qpdf_write(
    char const* const* paths, size_t source_count, OutputPage const* pages, size_t page_count,
    char const* temp, Progress progress, void* context, char* error, size_t capacity) noexcept {
    try {
        if (!paths || !pages || !temp || !page_count) throw std::runtime_error("Invalid structural snapshot");
        QPDF destination; destination.emptyPDF();
        QPDFPageDocumentHelper output(destination);
        QPDFAcroFormDocumentHelper forms(destination);
        // All QPDF sources remain alive through writer completion; streams are lazy.
        std::vector<std::unique_ptr<QPDF>> sources;
        std::vector<std::vector<QPDFPageObjectHelper>> source_pages;
        std::vector<std::unique_ptr<QPDFAcroFormDocumentHelper>> source_forms;
        if (progress) progress(context, 2, 0);
        for (size_t i = 0; i < source_count; ++i) {
            auto source = std::make_unique<QPDF>();
            source->setSuppressWarnings(true); source->setAttemptRecovery(false);
            source->processFile(paths[i]);
            if (source->isEncrypted()) throw std::runtime_error("Encrypted sources are not supported for P5C output");
            QPDFPageDocumentHelper helper(*source);
            helper.pushInheritedAttributesToPage();
            source_pages.push_back(helper.getAllPages());
            source_forms.push_back(std::make_unique<QPDFAcroFormDocumentHelper>(*source));
            if (source->anyWarnings()) throw std::runtime_error("Source PDF has qpdf structural warnings");
            sources.push_back(std::move(source));
        }
        if (progress) progress(context, 3, 0);
        for (size_t i = 0; i < page_count; ++i) {
            auto const& p = pages[i];
            if (p.source >= sources.size() || p.index >= source_pages[p.source].size() ||
                p.rotation >= 360 || p.rotation % 90) throw std::runtime_error("Invalid source-page reference");
            auto original = source_pages[p.source][p.index];
            // Supported copy API creates a distinct page object for every occurrence.
            auto copy = original.shallowCopyPage();
            copy.rotatePage(p.rotation, true);
            output.addPage(copy, false);
            // Retrieve the actual foreign copy without copying the full page vector per page.
            auto added = QPDFPageObjectHelper(destination.getAllPages().back());
            forms.fixCopiedAnnotations(added.getObjectHandle(), original.getObjectHandle(), *source_forms[p.source]);
        }
        if (progress) progress(context, 4, 0);
        {
            QPDFWriter writer(destination, temp);
            writer.setPreserveEncryption(false);
            writer.setObjectStreamMode(qpdf_o_generate);
            writer.registerProgressReporter(std::make_shared<QPDFWriter::FunctionProgressReporter>(
                [=](int percent) { if (progress) progress(context, 4, static_cast<uint32_t>(percent)); }));
            writer.write();
        }
        if (destination.anyWarnings()) throw std::runtime_error("Output construction generated qpdf warnings");
        return 0;
    } catch (std::exception const& e) { error_text(error, capacity, e.what()); return 1; }
      catch (...) { error_text(error, capacity, "Unknown libqpdf failure"); return 1; }
}
extern "C" __declspec(dllexport) int __cdecl pdfeditor_qpdf_verify(
    char const* path, size_t expected, char* error, size_t capacity) noexcept {
    try {
        QPDF pdf; pdf.setSuppressWarnings(true); pdf.setAttemptRecovery(false); pdf.processFile(path);
        auto pages = QPDFPageDocumentHelper(pdf).getAllPages();
        if (pages.size() != expected || !expected) throw std::runtime_error("Output page count does not match snapshot");
        // Resolve the entire page tree and validate lightweight boxes; never render.
        for (auto& page : pages) {
            auto box = page.getMediaBox();
            if (!box.isArray() || box.getArrayNItems() != 4) throw std::runtime_error("Output page has invalid MediaBox");
            double v[4];
            for (int i = 0; i < 4; ++i) {
                auto n = box.getArrayItem(i);
                if (!n.isNumber()) throw std::runtime_error("Output page has nonnumeric geometry");
                v[i] = n.getNumericValue();
                if (!std::isfinite(v[i])) throw std::runtime_error("Output page has nonfinite geometry");
            }
            if (v[2] <= v[0] || v[3] <= v[1]) throw std::runtime_error("Output page has empty geometry");
        }
        if (pdf.anyWarnings()) throw std::runtime_error("Output verification generated qpdf warnings");
        return 0;
    } catch (std::exception const& e) { error_text(error, capacity, e.what()); return 1; }
      catch (...) { error_text(error, capacity, "Unknown verification failure"); return 1; }
}
