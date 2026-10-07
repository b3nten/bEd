// State fixtures from unchanged pinned imgui-terminal core methods.
// Test-only access exposes state for measurement; it substitutes no algorithm.
// Attribution: resources/terminal/LICENSE and NOTICE.
#include <imgui.h>
#include <cstdio>
#include <cstdint>
#include <string>
#include <vector>
#include <wchar.h>
#include <sys/types.h>
#define private public
#include "terminal.h"
#undef private
#include "lib/json.hpp"
#include <cstdlib>
#include <fcntl.h>
#include <iostream>
#include <locale.h>
#include <unistd.h>
using json = nlohmann::json;

json snapshot(Terminal &terminal, int replyFd) {
    json rows = json::array();
    for (int row = 0; row < terminal.term.row; ++row) {
        json cells = json::array();
        for (int col = 0; col < terminal.term.col; ++col) {
            const auto &g = terminal.term.line[row][col];
            cells.push_back({g.u, g.mode, g.fg, g.bg});
        }
        rows.push_back(cells);
    }
    json colors = json::array();
    for (auto color : terminal.colors)
        colors.push_back({(color >> 0) & 255, (color >> 8) & 255, (color >> 16) & 255});
    char *selection = terminal.getsel();
    json selected = selection ? json(selection) : json(nullptr);
    std::free(selection);
    char replies[4096];
    const ssize_t count = read(replyFd, replies, sizeof(replies));
    json response = json::array();
    for (ssize_t i = 0; i < count; ++i) response.push_back(static_cast<unsigned char>(replies[i]));
    return {{"cells", rows}, {"cursor", {terminal.term.c.x, terminal.term.c.y}},
        {"cursor_shape", terminal.tw.cursor}, {"cursor_blinking", terminal.cursor_blinking},
        {"wrap_pending", (terminal.term.c.state & 1) != 0},
        {"win_mode", terminal.tw.mode}, {"term_mode", terminal.term.mode},
        {"title", terminal.title}, {"palette", colors}, {"selection", selected},
        {"replies", response}};
}
json feed(std::string bytes) { return {{"kind", "feed"}, {"text", bytes}}; }
json resize(int cols, int rows) { return {{"kind", "resize"}, {"cols", cols}, {"rows", rows}}; }
json start(int col, int row, int snap = 0) { return {{"kind", "select_start"}, {"col", col}, {"row", row}, {"snap", snap}}; }
json extend(int col, int row, int type = SEL_REGULAR, int done = 0) {
    return {{"kind", "select_extend"}, {"col", col}, {"row", row}, {"rectangular", type == SEL_RECTANGULAR}, {"done", done != 0}};
}
int main() {
    ImGui::CreateContext();
    setlocale(LC_CTYPE, "en_US.UTF-8");
    json cases = json::array({
        {{"name", "tabs_and_wrap"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("a\tb\r\n123456789012\tX"), feed("\x1b[2;1H\x1b[2ZQ")}}},
        {{"name", "alternate_screens_and_saved_attributes"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("main\x1b[31m\x1b" "7"), feed("\x1b[?47hALT"), feed("\x1b[?47lZ"), feed("\x1b[?1049h\x1b[32msecond"), feed("\x1b[?1049l\x1b" "8Q")}}},
        {{"name", "resize_truncates_both_grids"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("abcdefghij\r\nsecond\r\nthird\r\nfourth"), feed("\x1b[?47hALT\x1b[?47l"), resize(6,3), resize(14,6), feed("\x1b[?47hQ")}}},
        {{"name", "unicode_vs16_combining"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("中🙂é\u0301☕"), feed("\r\nA\uFE0F!\uFE0E"), feed("\r\n🖐\uFE0FZ")}}},
        {{"name", "one_column_wide_glyph"}, {"cols", 1}, {"rows", 4}, {"steps", {feed("中"), feed("A\r\n🙂"), resize(3,4), feed("Q")}}},
        {{"name", "erase_uses_current_colors"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("ABCDEFGHIJ\r\nsecond\r\nthird"), feed("\x1b[1;4H\x1b[31;44m\x1b[3X"), feed("\x1b[1K"), feed("\x1b[1J"), feed("\x1b[2J")}}},
        {{"name", "blink_attributes_and_modes"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("\x1b[1;2;3;4;5;7;8;9mX\x1b[0mY"), feed("\x1b[?5;9;1004;1034;2004h\x1b[2;12;20h"), feed("\x1b[?1002h\x1b[?1003l\x1b[?12h\x1b[7 q"), feed("\x1b[ q"), feed("\x1b" "c")}}},
        {{"name", "selection_wrap_word_line_rectangle"}, {"cols", 8}, {"rows", 5}, {"steps", {feed("one wordwrap next\r\nlast"), start(5,0,SNAP_WORD), start(2,1,SNAP_LINE), start(1,0), extend(4,2), extend(4,2,SEL_REGULAR,1), start(1,0), extend(4,2,SEL_RECTANGULAR), extend(4,2,SEL_RECTANGULAR,1)}}},
        {{"name", "selection_scroll_and_overwrite"}, {"cols", 8}, {"rows", 4}, {"steps", {feed("one\r\ntwo\r\nthree"), start(0,1,SNAP_LINE), feed("\x1b[4;1Hfour\r\nfive"), feed("\x1b[1;1HX")}}},
        {{"name", "scroll_origin_cursor_motion"}, {"cols", 12}, {"rows", 5}, {"steps", {feed("\x1b[2;4r\x1b[?6h\x1b[2;3HQ"), feed("\x1b[C\x1b[B\x1b[4GS"), feed("\x1b[?6l\x1b[5;1Hbottom\r\nnext"), feed("\x1b[3;1rX")}}},
        {{"name", "charset_and_repeat_after_controls"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("\x1b(0ABCDEFGq\x1b(BZ"), feed("A\x1b[3b\r\n\x1b[3bQ"), feed("\x1b" "7\x1b(0\x1b" "8A")}}},
        {{"name", "osc_palette_title_and_replies"}, {"cols", 12}, {"rows", 4}, {"steps", {feed("\x1b]4;1;Light Goldenrod Yellow\x07\x1b]10;#abc\x07\x1b]11;rgb:1234/5678/90ab\x1b\\"), feed("\x1b]1;  title ;ignored\x07\x1b]4;1;?\x07\x1b]10;?\x07"), feed("\x1b]104;1\x07\x1b]110\x07\x1b]111\x07"), feed("\x1b]10;#aébé\x07"), feed("\x1b]2;" + std::string(300,'T') + "\x07")}}},
    });
    for (auto &sample : cases) {
        Terminal terminal;
        terminal.tw.mode = MODE_VISIBLE | MODE_FOCUSED | MODE_NUMLOCK;
        terminal.tw.cursor = 2;
        terminal.cursor_blinking = false;
        terminal.load_colors();
        terminal.tnew(sample["cols"], sample["rows"]);
        terminal.selinit();
        int descriptors[2];
        if (pipe(descriptors) != 0) return 2;
        fcntl(descriptors[0], F_SETFL, O_NONBLOCK);
        terminal.cmdfd = descriptors[1]; terminal.iofd = -1;
        for (auto &step : sample["steps"]) {
            const std::string kind = step["kind"];
            if (kind == "feed") {
                const std::string bytes = step["text"];
                terminal.twrite(bytes.data(), static_cast<int>(bytes.size()), 0);
            } else if (kind == "resize") terminal.tresize(step["cols"], step["rows"]);
            else if (kind == "select_start") terminal.selstart(step["col"], step["row"], step["snap"]);
            else if (kind == "select_extend") terminal.selextend(step["col"], step["row"], step["rectangular"].get<bool>() ? SEL_RECTANGULAR : SEL_REGULAR, step["done"].get<bool>() ? 1 : 0);
            step["expected"] = snapshot(terminal, descriptors[0]);
        }
        close(descriptors[0]); close(descriptors[1]); terminal.cmdfd = -1;
    }
    std::cout << json({{"cases",cases}}).dump(2) << '\n';
    ImGui::DestroyContext();
}
