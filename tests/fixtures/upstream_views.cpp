// Draw/measurement fixtures from the unchanged ned sources at UPSTREAM_REVISION.
// The test apparatus sets font metrics and view inputs; it replaces no editor
// algorithm. Upstream attribution: LICENSE and NOTICE.
#include "editor/editor_operations.h"
#include "editor/editor_state.h"
#include "editor/editor_view_state.h"
#include "editor/services/highlight/highlight_service.h"
#include "editor/services/diagnostics/diagnostics_store.h"
#include "editor/services/git/git_service.h"
#include "editor/util/editor_utils.h"
#include "editor/views/caret_view.h"
#include "editor/views/gutter_view.h"
#include "editor/views/text_view.h"
#include "editor/views/view_layout.h"
#include "util/settings.h"
#include "imgui_internal.h"
#include <git2.h>

#include <iostream>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

// Winit replaces GLFW in Bed. Use the active host frame clock for the original
// rainbow algorithm too; no windowing library is needed by this headless test.
extern "C" double glfwGetTime() { return ImGui::GetTime(); }

struct Sample {
    const char *name;
    std::string bytes;
    std::vector<Selection> selections;
    int primary = 0;
    ImVec2 origin{40.25f, 50.5f};
    ImVec2 windowSize{600.0f, 400.0f};
    float scrollY = 0.0f;
    bool highlight = false;
    bool blocked = false;
    bool rainbow = false;
    bool diagnosticsEnabled = false;
    std::vector<DiagnosticItem> diagnostics;
    bool gutter = false;
    std::optional<std::string> gitBaseline;
    bool measureLines = true;
    float topMargin = 0.0f;
};

DiagnosticItem diagnostic(int startLine, int startCharacter, int endLine,
                          int endCharacter, int severity) {
    DiagnosticItem result;
    result.startLine = startLine;
    result.startCharacter = startCharacter;
    result.endLine = endLine;
    result.endCharacter = endCharacter;
    result.severity = severity;
    result.message = "fixture diagnostic";
    result.source = "fixture";
    return result;
}

void gitCheck(int result) {
    if (result < 0) {
        const git_error *error = git_error_last();
        throw std::runtime_error(error ? error->message : "libgit2 fixture failure");
    }
}

// Real libgit2 repository and HEAD blob, rather than substituted dirty markers.
std::string createRepository(const Sample &sample) {
    const char *base = std::getenv("BED_VIEW_FIXTURE_ROOT");
    if (!base) throw std::runtime_error("BED_VIEW_FIXTURE_ROOT missing");
    const auto root = std::filesystem::path(base) / sample.name;
    std::filesystem::create_directories(root);
    gitCheck(git_libgit2_init());
    git_repository *repo;
    gitCheck(git_repository_init(&repo, root.string().c_str(), 0));
    git_oid blobOid, treeOid, commitOid;
    gitCheck(git_blob_create_frombuffer(&blobOid, repo, sample.gitBaseline->data(),
                                       sample.gitBaseline->size()));
    git_treebuilder *builder;
    gitCheck(git_treebuilder_new(&builder, repo, nullptr));
    gitCheck(git_treebuilder_insert(nullptr, builder, "fixture.rs", &blobOid,
                                   GIT_FILEMODE_BLOB));
    gitCheck(git_treebuilder_write(&treeOid, builder));
    git_tree *tree;
    gitCheck(git_tree_lookup(&tree, repo, &treeOid));
    git_signature *signature;
    gitCheck(git_signature_new(&signature, "bed-fixture", "fixture@example.com", 1, 0));
    gitCheck(git_commit_create(&commitOid, repo, "HEAD", signature, signature,
                              nullptr, "fixture", tree, 0, nullptr));
    git_signature_free(signature);
    git_tree_free(tree);
    git_treebuilder_free(builder);
    git_repository_free(repo);
    git_libgit2_shutdown();
    std::ofstream(root / "fixture.rs", std::ios::binary) << sample.bytes;
    return root.string();
}

json vertices(const ImDrawList &draw, int first) {
    json output = json::array();
    for (int i = first; i < draw.VtxBuffer.Size; ++i) {
        const auto &v = draw.VtxBuffer[i];
        output.push_back({v.pos.x, v.pos.y, v.col});
    }
    return output;
}

json indices(const ImDrawList &draw, int first, int vertexBase) {
    json output = json::array();
    for (int i = first; i < draw.IdxBuffer.Size; ++i)
        output.push_back(static_cast<int>(draw.IdxBuffer[i]) - vertexBase);
    return output;
}

int main() {
    ImGui::CreateContext();
    ImGuiIO &io = ImGui::GetIO();
    io.IniFilename = nullptr;
    io.DisplaySize = ImVec2(640.0f, 480.0f);
    io.DeltaTime = 1.0f / 60.0f;
    unsigned char *pixels;
    int width, height;
    io.Fonts->GetTexDataAsRGBA32(&pixels, &width, &height);

    std::string many;
    for (int i = 0; i < 35; ++i) many += "row " + std::to_string(i) + "\n";
    std::vector<Sample> samples = {
        {"rainbow_initial_phase", "AA", {{0, 1, 0, 1}}, 0, {40.25f, 50.5f}, {600.0f, 400.0f}, 0.0f, false, false, true},
        {"selected_run", "ABCDE\nlast", {{0, 4, 0, 1}}},
        {"fractional_tabs_multicaret", "AAAA\tAAAA\n\té🙂", {{0, 3, 0, 1}, {1, 7, 1, 7}}, 1},
        {"guides_and_syntax", "        fn main() {\n\t\tlet answer = 42; // café\n}", {{1, 18, 0, 10}}, 0, {40.25f, 50.5f}, {600.0f, 400.0f}, 0.0f, true},
        {"bounded_malformed_bytes", std::string("\x80" "A\x80\tB\0C\xf0\x9f\x82\x80" "D", 13), {{0, 12, 0, 1}}},
        {"horizontal_clip", "AAAA ABCDEFGHIJKLMNOPQRSTUVWXYZ\tAAAA", {{0, 18, 0, 2}}, 0, {-17.25f, 50.5f}, {160.0f, 160.0f}},
        {"vertical_clip", many, {{7, 3, 5, 1}, {20, 2, 20, 2}}, 0, {40.25f, 5.5f}, {160.0f, 96.0f}, 45.0f},
        {"blocked_caret", "é🙂\tAA", {{0, 6, 0, 0}}, 0, {40.25f, 50.5f}, {600.0f, 400.0f}, 0.0f, false, true},
        {"rainbow_carets", "AAAA\té", {{0, 3, 0, 3}, {0, 7, 0, 7}}, 0, {40.25f, 50.5f}, {600.0f, 400.0f}, 0.0f, false, false, true},
    };
    Sample utf16{"diagnostics_utf16", "aé🙂\tAAAA\nsecond\n", {{0, 0, 0, 0}}};
    utf16.diagnosticsEnabled = true;
    utf16.diagnostics = {diagnostic(0, 2, 0, 4, 1), diagnostic(0, 4, 0, 5, 2),
                         diagnostic(0, 5, 0, 7, 3), diagnostic(1, 1, 1, 1, 4)};
    utf16.gutter = true;
    utf16.topMargin = 3.75f;
    samples.push_back(utf16);
    Sample multiline{"diagnostics_multiline_reversed", "AA🙂\n\n\téAA\nlast", {{2, 4, 0, 1}}};
    multiline.diagnosticsEnabled = true;
    multiline.diagnostics = {diagnostic(0, 2, 2, 3, 2), diagnostic(2, 4, 2, 1, 1),
                             diagnostic(-2, -1, 0, 1, 3), diagnostic(3, 500, 3, 500, 4)};
    multiline.gutter = true;
    samples.push_back(multiline);
    Sample clipping{"diagnostics_horizontal_clip", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", {{0, 1, 0, 1}}, 0,
                    {-17.25f, 50.5f}, {160.0f, 160.0f}};
    clipping.diagnosticsEnabled = true;
    clipping.diagnostics = {diagnostic(0, 0, 0, 30, 1), diagnostic(0, 2, 0, 12, 2)};
    samples.push_back(clipping);
    Sample git{"gutter_git_and_diagnostics", "original\nchanged\ninserted\nunchanged\nlast\n", {{0, 0, 0, 0}}};
    git.gitBaseline = "original\nsame\nunchanged\nlast\n";
    git.diagnosticsEnabled = true;
    git.diagnostics = {diagnostic(1, 0, 1, 3, 2), diagnostic(2, 1, 3, 2, 1)};
    git.gutter = true;
    git.topMargin = 5.25f;
    samples.push_back(git);
    std::string thousand;
    for (int i = 0; i < 1005; ++i) thousand += "A\n";
    Sample digits{"gutter_four_digits_scrolled_rainbow", thousand, {{1001, 1, 999, 0}, {1003, 1, 1002, 0}},
                  0, {40.25f, -12900.0f}, {160.0f, 96.0f}, 12935.0f, false, false, true};
    digits.diagnosticsEnabled = true; // Empty stores still reserve the column.
    digits.gutter = true;
    digits.measureLines = false;
    digits.topMargin = 2.5f;
    samples.push_back(digits);
    // Full original Settings construction, with HOME unset by the runner, takes
    // upstream's bundled-profile branch without modifying ned configuration.
    Settings settings;
    settings.settings["git_changed_lines"] = true;
    json cases = json::array();
    for (const auto &sample : samples) {
        EditorState state;
        state.setFromString(sample.bytes);
        std::string projectRoot;
        if (sample.gitBaseline) projectRoot = createRepository(sample);
        if (sample.highlight || sample.diagnosticsEnabled || sample.gutter)
            state.path = projectRoot.empty() ? "/fixture.rs" : projectRoot + "/fixture.rs";
        EditorGit git(state, projectRoot, settings);
        if (sample.gitBaseline) git.init();
        LSPDiagnostics diagnostics;
        if (sample.diagnosticsEnabled) diagnostics.replace(state.path, sample.diagnostics, 7);
        EditorOperations operations(state);
        EditorViewState view(state);
        view.selections = sample.selections;
        view.primaryIndex = sample.primary;
        view.syncPrimaryMirrors();
        view.blockInput = sample.blocked;
        view.cursorBlinkTime = 0.37f;
        view.setScrollPosition(ImVec2(0.0f, sample.scrollY));
        EditorHighlight highlight(state, operations);
        highlight.resetForDocument(state.lineCount());
        if (sample.highlight) {
            state.path = "/fixture.rs";
            state.languageId = "rs";
            // Prime the original query cache so the complete original service
            // performs its small-document parse synchronously.
            TreeSitter::highlightSnippet("rs", sample.bytes);
            highlight.highlightContent();
            highlight.poll();
        }

        ImGui::NewFrame();
        ImGui::SetNextWindowPos(ImVec2(20.0f, 20.0f), ImGuiCond_Always);
        ImGui::SetNextWindowSize(sample.windowSize, ImGuiCond_Always);
        ImGui::Begin("View fixture", nullptr, ImGuiWindowFlags_NoTitleBar);
        ImFontBaked *baked = ImGui::GetFontBaked();
        // Fixed fractional advance in BOTH fixture runners. This exercises run
        // pixel snapping independent of differences between font rasterizers.
        ImFontGlyph *glyph = const_cast<ImFontGlyph *>(baked->FindGlyph('A'));
        glyph->AdvanceX = 7.25f;
        baked->IndexAdvanceX['A'] = 7.25f;
        ImGui::GetCurrentWindow()->Scroll.y = sample.scrollY;
        ViewLayout layout;
        layout.textPos = sample.origin;
        layout.lineHeight = ImGui::GetTextLineHeight();
        layout.size = sample.windowSize;
        layout.rainbowMode = sample.rainbow;
        layout.editorTopMargin = sample.topMargin;
        ImDrawList &draw = *ImGui::GetWindowDrawList();
        const int firstTextVertex = draw.VtxBuffer.Size;
        const int firstTextIndex = draw.IdxBuffer.Size;
        TextView text(state, view, highlight, layout);
        if (sample.diagnosticsEnabled) text.setDiagnostics(&diagnostics);
        text.draw();
        json textVertices = vertices(draw, firstTextVertex);
        json textIndices = indices(draw, firstTextIndex, firstTextVertex);
        const int firstCaretVertex = draw.VtxBuffer.Size;
        const int firstCaretIndex = draw.IdxBuffer.Size;
        CaretView caret(view, layout);
        caret.draw();
        const json caretVertices = vertices(draw, firstCaretVertex);
        const json caretIndices = indices(draw, firstCaretIndex, firstCaretVertex);
        json gutterVertices = json::array(), gutterIndices = json::array();
        float gutterWidth = 0.0f;
        ImVec2 gutterPos{};
        if (sample.gutter) {
            GutterView gutter(state, view, git, layout);
            if (sample.diagnosticsEnabled) gutter.setDiagnostics(&diagnostics);
            gutterPos = gutter.createLineNumbersPanel();
            gutterWidth = gutter.lineNumberWidth;
            const int firstGutterVertex = draw.VtxBuffer.Size;
            const int firstGutterIndex = draw.IdxBuffer.Size;
            gutter.renderLineNumbers();
            gutterVertices = vertices(draw, firstGutterVertex);
            gutterIndices = indices(draw, firstGutterIndex, firstGutterVertex);
            ImGui::EndGroup();
        }

        json measurements = json::array();
        for (const auto &line : state.lines()) {
            if (!sample.measureLines) break;
            json columns = json::array();
            for (int column = -1; column <= static_cast<int>(line.size()) + 1; ++column)
                columns.push_back(EditorUtils::LineColumnX(line, column, sample.origin.x));
            json hits = json::array();
            for (float x : {-10.0f, 0.0f, 1.0f, 7.1f, 15.0f, 22.75f, 35.0f, 200.0f})
                hits.push_back(EditorUtils::ColumnAtX(line, x));
            measurements.push_back({{"columns", columns}, {"hits", hits},
                {"raw_advance", EditorUtils::GlyphAdvance(line.data(), line.data() + line.size())}});
        }
        json selections = json::array();
        for (const auto &s : view.selections)
            selections.push_back({s.headRow, s.headColumn, s.anchorRow, s.anchorColumn});
        std::vector<unsigned int> bytes(sample.bytes.begin(), sample.bytes.end());
        for (auto &byte : bytes) byte &= 255;
        json diagnosticItems = json::array();
        for (const auto &d : sample.diagnostics)
            diagnosticItems.push_back({d.startLine, d.startCharacter, d.endLine, d.endCharacter, d.severity});
        cases.push_back({{"name", sample.name}, {"bytes", bytes},
            {"selections", selections}, {"primary", sample.primary},
            {"origin", {sample.origin.x, sample.origin.y}},
            {"window_size", {sample.windowSize.x, sample.windowSize.y}},
            {"scroll_y", sample.scrollY}, {"highlight", sample.highlight},
            {"blocked", sample.blocked}, {"line_height", layout.lineHeight},
            {"rainbow", sample.rainbow},
            {"diagnostics_enabled", sample.diagnosticsEnabled}, {"diagnostics", diagnosticItems},
            {"gutter", sample.gutter}, {"gutter_width", gutterWidth},
            {"gutter_pos", {gutterPos.x, gutterPos.y}}, {"top_margin", sample.topMargin},
            {"git_baseline", sample.gitBaseline ? json(*sample.gitBaseline) : json(nullptr)},
            {"git_changes", git.currentGitChanges},
            {"text_vertices", textVertices}, {"text_indices", textIndices},
            {"caret_vertices", caretVertices}, {"caret_indices", caretIndices},
            {"gutter_vertices", gutterVertices}, {"gutter_indices", gutterIndices},
            {"measurements", measurements}});
        ImGui::End();
        ImGui::Render();
    }
    std::cout << json({{"imgui_version", IMGUI_VERSION}, {"cases", cases}}).dump(2) << '\n';
    ImGui::DestroyContext();
}
