#!/usr/bin/env bash
# Rebuild unchanged upstream tests, then regenerate exact capture fixtures.
set -euo pipefail
bed_root="$(cd "$(dirname "$0")/../.." && pwd)"
"$bed_root/scripts/provenance/verify-upstream-model.sh" --with-highlight \
    "$bed_root/reference/ned/lib/imgui" "$bed_root/reference/deps"
python3 - "$bed_root" <<'PY'
import pathlib
import subprocess
import sys

root = pathlib.Path(sys.argv[1])
ned = root / 'reference/ned'
build = root / 'target/upstream-model'
command = [
    'clang++', '-std=c++20', '-O0', '-DGLFW_INCLUDE_NONE',
    '-DIMGUI_USER_CONFIG="tests/ned_imgui_test_config.h"',
    f'-DCMAKE_SOURCE_DIR="{ned}"',
]
for directory in [ned, ned / 'editor', ned / 'tests', ned / 'lib/imgui',
                  root / 'reference/deps', ned / 'lib/treesitter/tree-sitter/lib/include']:
    command.extend(['-I', str(directory)])
command.extend(str(path) for path in [
    root / 'tests/fixtures/upstream_highlight.cpp',
    ned / 'editor/services/highlight/tree_sitter.cpp',
    ned / 'editor/buffer/text_buffer.cpp', ned / 'editor/editor_state.cpp',
])
command.extend(str(path) for path in sorted(build.glob('*.o')))
command.extend(['-o', str(build / 'ned-highlight-fixtures')])
subprocess.run(command, check=True)
fixture = root / 'tests/fixtures/highlight.json'
with fixture.open('wb') as stream:
    subprocess.run([str(build / 'ned-highlight-fixtures')], stdout=stream, check=True)
print(f'Generated {fixture.relative_to(root)} from pinned upstream implementation')
PY
