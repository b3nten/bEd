"""Exercise archive/resource assembly without native builds or installation."""
import base64
import json
import os
import plistlib
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
import zlib
from pathlib import Path

from test_remote_helpers import HELPERS, bundle, elf


SCRIPTS = Path(__file__).resolve().parents[1]
RUST_BINDING_NOTICES = [
    f"vendor/{binding}/{license}"
    for binding in ("dear-imgui-sys", "dear-imgui-winit", "dear-imgui-wgpu")
    for license in ("LICENSE-MIT", "LICENSE-APACHE")
]


def png(size):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    rows = (b"\0" + b"\x40\xc0\x80\xff" * size) * size
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


def png_dimensions(data):
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("Expected PNG image")
    return struct.unpack_from(">II", data, 16)


class DesktopPackageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="bed-packaging-")
        self.addCleanup(temporary.cleanup)
        # Spaces exercise quoting in entry points, resources and helper paths.
        self.root = Path(temporary.name) / "fixture repository"
        (self.root / "scripts").mkdir(parents=True)
        shutil.copytree(SCRIPTS / "lib", self.root / "scripts/lib")
        for name in ("package-linux.sh", "package-macos.sh"):
            shutil.copy2(SCRIPTS / name, self.root / "scripts" / name)
        files = {
            "Cargo.toml": f'[package]\nname = "bed"\nversion = "{HELPERS.package_version()}"\n',
            "Cargo.lock": "fixture lockfile\n",
            "crates/bed-remote/src/protocol.rs": f"pub const PROTOCOL_VERSION: u32 = {HELPERS.protocol_version()};\n",
            "LICENSE": "Project license\n",
            "NOTICE": "Required attribution\n",
            "LICENSES/tree-sitter-sources.json": "[]\n",
            "LICENSES/musl-COPYRIGHT.txt": "Retained musl license\n",
            "LICENSES/terminal-adapter-BSL-1.1.txt": "Retained terminal license\n",
            "vendor/freetype-sys/freetype2/docs/FTL.TXT": "Retained font license\n",
            "resources/config/bed.json": "{}\n",
            "resources/fonts/SourceCodePro-Regular.ttf": "source font\n",
            "resources/fonts/Emoji.ttf": "emoji font\n",
            "resources/icons/file.svg": "<svg xmlns='http://www.w3.org/2000/svg'/>\n",
            "assets/bEd.icon/icon.json": "{}\n",
            "resources/queries/rs.scm": "highlight query\n",
            "target/release/bed": "#!/bin/sh\nexit 0\n",
            "target/release/bed-headless": "#!/bin/sh\nexit 0\n",
        }
        files.update({name: f"Retained Rust binding notice: {name}\n" for name in RUST_BINDING_NOTICES})
        for name, contents in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")
        for binary in ("bed", "bed-headless"):
            (self.root / "target/release" / binary).chmod(0o755)
        (self.root / "assets/bEd-iOS-Default-1024@1x.png").write_bytes(png(1024))
        helpers = self.root / "prebuilt helper bundles"
        helpers.mkdir()
        for target in HELPERS.TARGETS:
            bundle(helpers, target)
        self.bin = self.root / "tools"
        self.bin.mkdir()
        # Only host tools are adapted. Resource copying, license collection,
        # helper validation and archive creation run their real implementations.
        self.tool("cargo", "print('{\"packages\": [], \"workspace_members\": []}')")
        self.tool("plutil", "import plistlib, sys; plistlib.loads(open(sys.argv[-1], 'rb').read())")
        self.tool("otool", "import sys; print(sys.argv[-1] + ':')")
        self.tool("codesign", "pass")
        images = {size: base64.b64encode(png(size)).decode() for size in (16, 24, 32, 48, 64, 128, 256, 512, 1024)}
        self.resize_tool = f"""import base64, pathlib, sys
images = {images!r}
if '-resize' in sys.argv:
    size = int(sys.argv[sys.argv.index('-resize') + 1].split('x')[0])
    source, output = sys.argv[1], sys.argv[-1]
else:
    index = sys.argv.index('--resampleHeightWidth')
    size = int(sys.argv[index + 1])
    assert int(sys.argv[index + 2]) == size
    source, output = sys.argv[index + 3], sys.argv[-1]
assert pathlib.Path(source).read_bytes() == base64.b64decode(images[1024])
pathlib.Path(output).write_bytes(base64.b64decode(images[size]))
"""
        self.tool("sips", self.resize_tool)
        self.tool("magick", self.resize_tool)
        self.tool("iconutil", """import pathlib, struct, sys
iconset = pathlib.Path(sys.argv[-1])
output = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
types = {'16x16': b'icp4', '16x16@2x': b'ic11', '32x32': b'icp5', '32x32@2x': b'ic12',
         '128x128': b'ic07', '128x128@2x': b'ic13', '256x256': b'ic08', '256x256@2x': b'ic14',
         '512x512': b'ic09', '512x512@2x': b'ic10'}
chunks = []
for name, kind in types.items():
    data = (iconset / ('icon_' + name + '.png')).read_bytes()
    chunks.append(kind + struct.pack('>I', len(data) + 8) + data)
data = b''.join(chunks)
output.write_bytes(b'icns' + struct.pack('>I', len(data) + 8) + data)
""")
        self.tool("ditto", """import pathlib, shutil, sys
app, output = map(pathlib.Path, sys.argv[-2:])
shutil.make_archive(str(output.with_suffix('')), 'zip', root_dir=app.parent, base_dir=app.name)
""")
        self.environment = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
                                BED_HELPERS_DIR=str(helpers))
        self.environment.pop("BED_BINARY", None)
        self.environment.pop("BED_DIST_DIR", None)
        self.environment.pop("BASH_ENV", None)

    def tool(self, name, body):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n{body}\n", encoding="utf-8")
        path.chmod(0o755)

    def run_package(self, platform):
        system, architecture = ("Darwin", "arm64") if platform == "macos" else ("Linux", "x86_64")
        self.tool("uname", f"import sys; print({system!r} if sys.argv[-1] == '-s' else {architecture!r})")
        return subprocess.run(
            ["bash", str(self.root / "scripts" / f"package-{platform}.sh"), "--skip-build"],
            cwd=self.root.parent, env=self.environment, text=True, capture_output=True, check=False)

    def test_linux_archive_keeps_runtime_resources_helpers_and_notices(self):
        result = self.run_package("linux")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        archive, = (self.root / "target/dist").glob("*.tar.gz")
        with tarfile.open(archive) as package:
            prefix = "usr/share/Bed/"
            self.assertEqual(package.extractfile(prefix + "NOTICE").read().splitlines()[0], b"Required attribution")
            self.assertEqual(package.extractfile(prefix + "LICENSES/dependencies/vendor/freetype-sys/freetype2/docs/FTL.TXT").read(), b"Retained font license\n")
            self.assertIn("usr/share/applications/bed.desktop", package.getnames())
            self.assertIn(prefix + "resources/queries/rs.scm", package.getnames())
            self.assertFalse(any(name.startswith(prefix + "queries/") for name in package.getnames()))
            self.assertEqual(package.extractfile(prefix + "resources/icons/bed.png").read(),
                             (self.root / "assets/bEd-iOS-Default-1024@1x.png").read_bytes())
            for size in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
                icon = package.extractfile(f"usr/share/icons/hicolor/{size}x{size}/apps/bed.png").read()
                self.assertEqual(png_dimensions(icon), (size, size))
            self.assertFalse(any("/assets/" in name for name in package.getnames()))
            for notice in RUST_BINDING_NOTICES:
                self.assertEqual(package.extractfile(prefix + "LICENSES/dependencies/" + notice).read(),
                                 (self.root / notice).read_bytes())
            for target in HELPERS.TARGETS:
                binary = prefix + f"remote-helpers/{target}/bed-headless"
                self.assertTrue(package.getmember(binary).mode & 0o111)
                manifest = json.load(package.extractfile(prefix + f"remote-helpers/{target}/manifest.json"))
                self.assertEqual(manifest["target"], target)

    def test_mac_archive_has_bundle_metadata_and_both_validated_helpers(self):
        result = self.run_package("macos")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        archive, = (self.root / "target/dist").glob("*.zip")
        with zipfile.ZipFile(archive) as package:
            info = plistlib.loads(package.read("bEd.app/Contents/Info.plist"))
            self.assertEqual(info["CFBundleExecutable"], "bed")
            self.assertEqual(info["CFBundleName"], "bEd")
            self.assertEqual(info["CFBundleIconFile"], "bed.icns")
            resources = "bEd.app/Contents/Resources/"
            self.assertEqual(package.read(resources + "resources/icons/bed.png"),
                             (self.root / "assets/bEd-iOS-Default-1024@1x.png").read_bytes())
            self.assertIn(resources + "resources/queries/rs.scm", package.namelist())
            self.assertFalse(any(name.startswith(resources + "queries/") for name in package.namelist()))
            icon = package.read(resources + "bed.icns")
            self.assertEqual(icon[:4], b"icns")
            self.assertEqual(struct.unpack_from(">I", icon, 4)[0], len(icon))
            sizes = []
            offset = 8
            while offset < len(icon):
                length = struct.unpack_from(">I", icon, offset + 4)[0]
                sizes.append(png_dimensions(icon[offset + 8:offset + length]))
                offset += length
            self.assertEqual(len(sizes), 10)
            self.assertEqual(set(sizes), {(size, size) for size in (16, 32, 64, 128, 256, 512, 1024)})
            self.assertFalse(any("/assets/" in name for name in package.namelist()))
            self.assertEqual(package.read(resources + "LICENSE"), b"Project license\n")
            self.assertIn(resources + "LICENSES/dependencies/index.json", package.namelist())
            for notice in RUST_BINDING_NOTICES:
                self.assertEqual(package.read(resources + "LICENSES/dependencies/" + notice),
                                 (self.root / notice).read_bytes())
            for target in HELPERS.TARGETS:
                self.assertIn(resources + f"remote-helpers/{target}/bed-headless", package.namelist())

    def test_damaged_helper_prevents_publishing_a_desktop_package(self):
        helper = Path(self.environment["BED_HELPERS_DIR"]) / next(iter(HELPERS.TARGETS)) / "bed-headless"
        helper.write_bytes(helper.read_bytes() + b"damaged")
        result = self.run_package("linux")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sha256", result.stderr)
        self.assertFalse((self.root / "target/dist").exists())

    def previous_outputs(self):
        dist = self.root / "target/dist"
        app_marker = dist / "bEd.app/Contents/Resources/previous.txt"
        app_marker.parent.mkdir(parents=True)
        app_marker.write_bytes(b"previous app")
        outputs = {app_marker: b"previous app"}
        for suffix in ("arm64.zip", "x86_64-linux.tar.gz"):
            path = dist / f"bEd-{HELPERS.package_version()}-{suffix}"
            path.write_bytes(b"previous archive")
            outputs[path] = b"previous archive"
        return outputs

    def test_missing_icon_source_keeps_existing_packages_intact(self):
        outputs = self.previous_outputs()
        (self.root / "assets/bEd-iOS-Default-1024@1x.png").unlink()
        for platform in ("macos", "linux"):
            with self.subTest(platform=platform):
                result = self.run_package(platform)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Missing application icon export", result.stderr)
                for path, contents in outputs.items():
                    self.assertEqual(path.read_bytes(), contents)
                self.assertFalse(list((self.root / "target/dist").glob("bed-package.*")))

    def test_icon_resize_failure_keeps_existing_packages_intact(self):
        outputs = self.previous_outputs()
        for platform, tool in (("macos", "sips"), ("linux", "magick")):
            with self.subTest(platform=platform):
                self.tool(tool, "import sys; sys.exit('Icon resize failed')")
                result = self.run_package(platform)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Icon resize failed", result.stderr)
                for path, contents in outputs.items():
                    self.assertEqual(path.read_bytes(), contents)
                self.assertFalse(list((self.root / "target/dist").glob("bed-package.*")))

    def test_linux_accepts_imagemagick_convert_when_magick_is_unavailable(self):
        (self.bin / "magick").unlink()
        self.tool("convert", self.resize_tool)
        # Hide a host ImageMagick 7 install so this always exercises the v6 path.
        shell_setup = self.root / "without-magick.bash"
        shell_setup.write_text(
            'command() {\n'
            '    if [ "$1" = -v ] && [ "$2" = magick ]; then return 1; fi\n'
            '    builtin command "$@"\n'
            '}\n', encoding="utf-8")
        self.environment["BASH_ENV"] = str(shell_setup)
        result = self.run_package("linux")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        archive, = (self.root / "target/dist").glob("*.tar.gz")
        with tarfile.open(archive) as package:
            icon = package.extractfile("usr/share/icons/hicolor/256x256/apps/bed.png").read()
            self.assertEqual(png_dimensions(icon), (256, 256))

    def test_icns_compilation_failure_keeps_existing_packages_intact(self):
        outputs = self.previous_outputs()
        self.tool("iconutil", "import sys; sys.exit('ICNS compilation failed')")
        result = self.run_package("macos")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ICNS compilation failed", result.stderr)
        for path, contents in outputs.items():
            self.assertEqual(path.read_bytes(), contents)
        self.assertFalse(list((self.root / "target/dist").glob("bed-package.*")))

    def test_missing_rust_binding_notice_prevents_publishing_a_desktop_package(self):
        (self.root / RUST_BINDING_NOTICES[0]).unlink()
        result = self.run_package("linux")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Missing Rust binding notice", result.stderr)
        self.assertEqual(list((self.root / "target/dist").iterdir()), [])

    def test_standalone_helper_archive_keeps_binary_and_attribution(self):
        output = self.root / "custom output/helper"
        result = subprocess.run(
            [sys.executable, str(self.root / "scripts/lib/remote-helpers.py"), "standalone",
             "--output", str(output)], env=self.environment,
            text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        with zipfile.ZipFile(output.with_suffix(".zip")) as package:
            self.assertEqual(package.read("helper/bed-headless"), b"#!/bin/sh\nexit 0\n")
            self.assertEqual(package.read("helper/LICENSE"), b"Project license\n")
            self.assertIn("helper/NOTICE", package.namelist())
            self.assertIn("helper/LICENSES/dependencies/index.json", package.namelist())

    def test_remote_helper_staging_retains_product_build_provenance(self):
        target = "aarch64-unknown-linux-musl"
        binary = self.root / "static-helper"
        binary.write_bytes(elf(HELPERS.TARGETS[target]))
        output = self.root / "staged helpers"
        source_revision = "1234abcd" * 5
        result = subprocess.run(
            [sys.executable, str(self.root / "scripts/lib/remote-helpers.py"), "stage",
             "--target", target, "--binary", str(binary), "--output", str(output)],
            env=dict(self.environment, GITHUB_SHA=source_revision),
            text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        manifest = json.loads((output / target / "manifest.json").read_text())
        self.assertEqual(manifest["source_revision"], source_revision)
        self.assertEqual(manifest["cargo_lock_sha256"], HELPERS.digest(self.root / "Cargo.lock"))
        self.assertEqual(manifest["sha256"], HELPERS.digest(binary))
        self.assertNotIn("upstream_revision", manifest)


if __name__ == "__main__":
    unittest.main()
