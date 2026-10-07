#!/usr/bin/env bash
# Run the unchanged, pinned upstream model tests without ned's desktop stack.
# Optional UTF-8/layer tests require the pinned lib/imgui submodule and GLFW's
# official 3.4 header: https://raw.githubusercontent.com/glfw/glfw/3.4/include/GLFW/glfw3.h
# The header only satisfies unrelated inline GUI declarations; no GLFW code is
# linked and no editor/test behavior is stubbed or replaced.
set -euo pipefail
bed_root="$(cd "$(dirname "$0")/../.." && pwd)"
ned_root="$bed_root/reference/ned"
expected_revision="$(tr -d '\r\n' < "$bed_root/UPSTREAM_REVISION")"
actual_revision="$(git -C "$ned_root" rev-parse HEAD)"
if [[ "$actual_revision" != "$expected_revision" ]]; then
    printf 'Upstream revision mismatch: expected %s, found %s\n' "$expected_revision" "$actual_revision" >&2
    exit 1
fi
compiler="${CXX:-clang++}"
build_dir="$bed_root/target/upstream-model"
mkdir -p "$build_dir"
include_flags=(-I "$ned_root" -I "$ned_root/editor" -I "$ned_root/tests")
sources=(
    "$ned_root/tests/main.cpp"
    "$ned_root/editor/buffer/text_buffer.cpp"
    "$ned_root/editor/editor_state.cpp"
    "$ned_root/editor/editor_operations.cpp"
    "$ned_root/tests/monaco/text_model.cpp"
    "$ned_root/tests/editor/text_buffer_test.cpp"
    "$ned_root/tests/editor/editor_operations_test.cpp"
    "$ned_root/tests/editor/text_model_data_test.cpp"
    "$ned_root/tests/editor/text_model_test.cpp"
    "$ned_root/tests/editor/model_edit_operation_test.cpp"
    "$ned_root/tests/editor/editable_text_model_test.cpp"
)
if [[ "$#" != 0 ]]; then
    if [[ "$#" != 3 || ( "$1" != '--with-utf8' && "$1" != '--with-layer' && "$1" != '--with-highlight' ) ]]; then
        printf 'Usage: %s [--with-utf8|--with-layer|--with-highlight IMGUI_INCLUDE_DIR GLFW_INCLUDE_DIR]\n' "$0" >&2
        exit 2
    fi
    [[ -f "$2/imgui.h" && -f "$3/GLFW/glfw3.h" ]]
    include_flags+=(-I "$2" -I "$3" -DGLFW_INCLUDE_NONE)
    sources+=("$ned_root/tests/editor/utf8_utils_test.cpp")
    if [[ "$1" == '--with-layer' || "$1" == '--with-highlight' ]]; then
        # Use the actual upstream headless config: wchar32, no FreeType backend.
        include_flags+=('-DIMGUI_USER_CONFIG="tests/ned_imgui_test_config.h"')
        sources+=(
            "$ned_root/editor/editor_view_state.cpp"
            "$ned_root/editor/editor_commands.cpp"
            "$ned_root/editor/editor_events.cpp"
            "$ned_root/editor/services/save_service.cpp"
            "$ned_root/util/project_undo.cpp"
            "$2/imgui.cpp" "$2/imgui_draw.cpp" "$2/imgui_tables.cpp" "$2/imgui_widgets.cpp"
            "$ned_root/tests/editor/editor_commands_test.cpp"
            "$ned_root/tests/editor/multi_cursor_test.cpp"
            "$ned_root/tests/editor/undo_service_test.cpp"
            "$ned_root/tests/editor/save_service_test.cpp"
        )
    fi
    if [[ "$1" == '--with-highlight' ]]; then
        runtime="$ned_root/lib/treesitter/tree-sitter/lib"
        include_flags+=(-I "$runtime/include" "-DCMAKE_SOURCE_DIR=\"$ned_root\"")
        c_compiler="${CC:-clang}"
        "$c_compiler" -std=c11 -O0 -D_POSIX_C_SOURCE=200112L -D_DEFAULT_SOURCE \
            -I "$runtime/include" -I "$runtime/src" -I "$runtime/src/wasm" \
            -c "$runtime/src/lib.c" -o "$build_dir/tree-sitter.o"
        sources+=("$build_dir/tree-sitter.o")
        for grammar in c cpp javascript python csharp html typescript css java go hcl json kotlin bash rust toml ruby; do
            grammar_src="$ned_root/lib/treesitter/tree-sitter-$grammar/src"
            if [[ "$grammar" == typescript ]]; then
                grammar_src="$ned_root/lib/treesitter/tree-sitter-typescript/tsx/src"
            fi
            for native_source in parser scanner; do
                if [[ -f "$grammar_src/$native_source.c" ]]; then
                    native_object="$build_dir/$grammar-$native_source.o"
                    "$c_compiler" -std=c11 -O0 -w -I "$grammar_src" \
                        -c "$grammar_src/$native_source.c" -o "$native_object"
                    sources+=("$native_object")
                fi
            done
        done
        sources+=(
            "$ned_root/editor/services/highlight/tree_sitter.cpp"
            "$ned_root/editor/services/highlight/span_map.cpp"
            "$ned_root/editor/services/highlight/highlight_service.cpp"
            "$ned_root/tests/editor/capture_map_test.cpp"
            "$ned_root/tests/editor/highlight_queries_test.cpp"
            "$ned_root/tests/editor/highlight_service_test.cpp"
        )
    fi
fi
"$compiler" -std=c++20 -O0 "${include_flags[@]}" "${sources[@]}" -o "$build_dir/ned-model-tests"
"$build_dir/ned-model-tests"
