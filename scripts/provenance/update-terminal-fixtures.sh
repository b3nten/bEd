#!/usr/bin/env bash
# Serialize the unchanged pinned terminal core; no font/GPU/PTY substitutions.
set -euo pipefail
bed_root="$(cd "$(dirname "$0")/../.." && pwd)"
python3 - "$bed_root" <<'PY'
import hashlib
import os
import pathlib
import subprocess
import sys
root = pathlib.Path(sys.argv[1])
ned = root / 'reference/ned'
terminal = ned / 'lib/imgui-terminal'
expected = '1a6733ba9e5cd777acc86cbb111c1c9b4f7500a4'
actual = subprocess.check_output(['git','-C',str(terminal),'rev-parse','HEAD'],text=True).strip()
if expected != actual: sys.exit(f'ImGui-Terminal revision mismatch: {actual}')
subprocess.run(['git','-C',str(terminal),'diff','--quiet','--exit-code'],check=True)
fontconfig = root / 'reference/deps/fontconfig'
header = fontconfig / 'fontconfig.h.in'
if not header.exists():
    sys.exit('Fetch the genuine Fontconfig 2.17.1 fontconfig/fontconfig.h.in template and COPYING into reference/deps/fontconfig; see tests/fixtures/terminal_core/README.md')
if hashlib.sha256(header.read_bytes()).hexdigest() != '5e5e2494b98b4ca691165dd166732e96beb6a25d757a9251553561d2fc4fd321':
    sys.exit('Fontconfig 2.17.1 header template hash mismatch')
(fontconfig / 'fontconfig.h').write_bytes(header.read_bytes().replace(b'@CACHE_VERSION@',b'9'))
build = root / 'target/upstream-terminal'
build.mkdir(parents=True,exist_ok=True)
command = [os.environ.get('CXX','clang++'),'-std=c++20','-O0','-ffunction-sections','-fdata-sections',
           '-Wno-deprecated-literal-operator','-DIMGUI_USER_CONFIG="tests/ned_imgui_test_config.h"']
for path in [ned,terminal,ned/'lib/imgui',ned/'lib/imgui/misc/freetype',root/'reference/deps']:
    command += ['-I',str(path)]
command += [str(root/'tests/fixtures/upstream_terminal.cpp'),str(terminal/'terminal.cpp')]
command += [str(ned/'lib/imgui'/name) for name in ['imgui.cpp','imgui_draw.cpp','imgui_tables.cpp','imgui_widgets.cpp']]
command += ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections','-o',str(build/'ned-terminal-fixtures')]
if sys.platform != 'darwin': command += ['-lutil','-lpthread']
subprocess.run(command,check=True)
with (root/'tests/fixtures/terminal_core/state.json').open('wb') as stream:
    subprocess.run([str(build/'ned-terminal-fixtures')],stdout=stream,cwd=terminal,check=True)
print('Generated terminal_core/state.json from pinned original terminal methods')
PY
