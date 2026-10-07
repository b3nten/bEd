// Generate capture fixtures using the unchanged pinned upstream implementation.
#include "editor/services/highlight/tree_sitter.h"
#include "util/settings.h"
#include <iostream>
#include <vector>
std::string Settings::getAppResourcesPath() { return CMAKE_SOURCE_DIR; }
int main() {
    const std::vector<std::pair<std::string, std::string>> samples = {
        {"c", "// café\nint main(void) { return 42; }\n"},
        {"cpp", "namespace demo { class Widget { public: int method(int x) { return x + 1; } }; }\n"},
        {"js", "// Unicode π\nconst GREETING = `hi ${name}`; function run(x) { return Math.abs(x); }\n"},
        {"py", "@decorator\ndef run(x):\n    return f\"hello {x}\" # comment\n"},
        {"cs", "namespace Demo { public class Widget { public int Value { get; set; } } }\n"},
        {"html", "<div class=\"hello\">café &amp; hi</div>\n"},
        {"tsx", "interface Props { value: number }\nconst View = (p: Props) => <div>{p.value}</div>;\n"},
        {"css", "/* hello */\nbody { color: #ff00ff; margin: 2px; }\n"},
        {"java", "package demo; public class Widget { public int run(int x) { return x + 1; } }\n"},
        {"go", "package main\nfunc main() { println(\"hello\") }\n"},
        {"hcl", "resource \"aws_instance\" \"web\" {\n  ami = \"${var.ami}\"\n}\n"},
        {"json", "{\"greeting\": \"café\", \"count\": 42, \"enabled\": true, \"none\": null}\n"},
        {"kt", "package demo\nfun run(x: Int): String { return \"value $x\" }\n"},
        {"sh", "#!/bin/bash\nNAME=world\necho \"hello $NAME\" # comment\n"},
        {"rs", "// hello\nfn main() { let value: i32 = 42; println!(\"{}\", value); }\n"},
        {"toml", "# hello\n[package]\nname = \"demo\"\nversion = 42\n"},
        {"rb", "# hello\nclass Widget\n  def run(x)\n    puts \"value #{x}\"\n  end\nend\n"},
    };
    json output = json::array();
    for (const auto &[language, source] : samples) {
        const auto colors = TreeSitter::highlightSnippet(language, source);
        json rows = json::array();
        for (const auto &line : colors) {
            json spans = json::array();
            for (const auto &s : line) spans.push_back({s.start, s.end, static_cast<int>(s.slot)});
            rows.push_back(spans);
        }
        output.push_back({{"language", language}, {"source", source}, {"spans", rows}});
    }
    std::cout << output.dump(2) << '\n';
}
