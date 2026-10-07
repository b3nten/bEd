#!/usr/bin/env bash
# Generate geometry from unchanged pinned TextView/CaretView/GutterView code.
set -euo pipefail
bed_root="$(cd "$(dirname "$0")/../.." && pwd)"
python3 - "$bed_root" <<'PY'
import os
import pathlib
import subprocess
import sys
import tempfile

root = pathlib.Path(sys.argv[1])
ned = root / 'reference/ned'
expected = (root / 'UPSTREAM_REVISION').read_text().strip()
actual = subprocess.check_output(['git', '-C', str(ned), 'rev-parse', 'HEAD'], text=True).strip()
if actual != expected:
    sys.exit(f'Upstream revision mismatch: expected {expected}, found {actual}')
imgui_sha = (root / 'UPSTREAM_SUBMODULES').read_text().splitlines()[0].split()[2]
actual_imgui = subprocess.check_output(['git', '-C', str(ned / 'lib/imgui'), 'rev-parse', 'HEAD'], text=True).strip()
if actual_imgui != imgui_sha:
    sys.exit(f'ImGui revision mismatch: expected {imgui_sha}, found {actual_imgui}')
subprocess.run(['git', '-C', str(ned), 'diff', '--exit-code', '--quiet', '--',
                'editor', 'util'], check=True)
subprocess.run(['git', '-C', str(ned / 'lib/imgui'), 'diff', '--exit-code', '--quiet'], check=True)
build = root / 'target/upstream-model'
if not (build / 'tree-sitter.o').exists():
    subprocess.run([str(root / 'scripts/provenance/verify-upstream-model.sh'), '--with-highlight',
                    str(ned / 'lib/imgui'), str(root / 'reference/deps')], check=True)
command = [os.environ.get('CXX', 'clang++'), '-std=c++20', '-O0', '-Wno-deprecated-literal-operator',
           '-ffunction-sections', '-fdata-sections', '-DGLFW_INCLUDE_NONE',
           '-DNED_ENABLE_GIT=1', '-DNED_ENABLE_LSP=0',
           '-DIMGUI_USER_CONFIG="tests/ned_imgui_test_config.h"',
           f'-DCMAKE_SOURCE_DIR="{ned}"']
for directory in [ned, ned / 'editor', ned / 'tests', ned / 'lib/imgui',
                  root / 'reference/deps', ned / 'lib/treesitter/tree-sitter/lib/include',
                  ned / 'lib/libgit2/include']:
    command.extend(['-I', str(directory)])
sources = [
    root / 'tests/fixtures/upstream_views.cpp',
    ned / 'editor/buffer/text_buffer.cpp', ned / 'editor/editor_state.cpp',
    ned / 'editor/editor_view_state.cpp', ned / 'editor/editor_operations.cpp',
    ned / 'editor/editor_api.cpp',
    ned / 'editor/services/highlight/tree_sitter.cpp',
    ned / 'editor/services/highlight/span_map.cpp',
    ned / 'editor/services/highlight/highlight_service.cpp',
    ned / 'editor/services/diagnostics/diagnostics_store.cpp',
    ned / 'editor/services/git/git_service.cpp',
    ned / 'editor/services/git/git_repo.cpp', ned / 'editor/services/git/line_diff.cpp',
    ned / 'editor/views/text_view.cpp', ned / 'editor/views/caret_view.cpp',
    ned / 'editor/views/gutter_view.cpp',
    ned / 'editor/views/hover_tooltip.cpp', ned / 'editor/views/hover_markdown.cpp',
    ned / 'util/settings.cpp', ned / 'util/keybinds.cpp',
]
sources.extend(ned / 'lib/imgui' / name for name in
               ['imgui.cpp', 'imgui_draw.cpp', 'imgui_tables.cpp', 'imgui_widgets.cpp'])
command.extend(str(path) for path in sources)
command.extend(str(path) for path in sorted(build.glob('*.o')))
for pattern in ['target/debug/build/libgit2-sys-*/out/build/libgit2.a',
                'target/debug/build/libz-sys-*/out/lib/libz.a']:
    archives = list(root.glob(pattern))
    if not archives:
        sys.exit(f'Missing native archive {pattern}; first run cargo test -p bed-session --offline --test git')
    command.append(str(max(archives, key=lambda path: path.stat().st_mtime)))
if sys.platform == 'darwin':
    command.extend(['-liconv', '-framework', 'Security', '-framework', 'CoreFoundation'])
else:
    command.extend(['-lpthread', '-ldl'])
command.extend(['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections',
                '-o', str(build / 'ned-view-fixtures')])
subprocess.run(command, check=True)
fixture = root / 'tests/fixtures/views.json'
environment = os.environ.copy()
for name in ['HOME', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH']:
    environment.pop(name, None)
with tempfile.TemporaryDirectory(prefix='view-repositories-', dir=build) as directory:
    environment['BED_VIEW_FIXTURE_ROOT'] = directory
    with fixture.open('wb') as stream:
        subprocess.run([str(build / 'ned-view-fixtures')], stdout=stream, check=True,
                       cwd=ned, env=environment)
print(f'Generated {fixture.relative_to(root)} from pinned original view implementations')
PY
