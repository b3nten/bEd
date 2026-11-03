"""Viewers: files open in the viewer made for them, filling the panel, with
floating controls and right-click menus."""
import math
import shutil
import struct
import tempfile
import wave
from pathlib import Path
from bed import Bed


def until(bed, check, what):
    for _ in range(300):
        if check():
            return
        bed.wait(5)
    raise AssertionError(f"never: {what}")


def row(bed, name):
    """The tree's row (tabs with the same name are further right)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def chirp(path, seconds=4.0, rate=22050):
    """A rising tone with beats: something to see in every view."""
    frames = bytearray()
    phase = 0.0
    for i in range(int(seconds * rate)):
        t = i / rate
        frequency = 200 * (40 ** (t / seconds))
        phase += 2 * math.pi * frequency / rate
        beat = 0.6 + 0.4 * math.sin(2 * math.pi * 2 * t)
        sample = (math.sin(phase) * 0.5 + math.sin(phase * 3) * 0.12) * beat
        frames += struct.pack("<h", int(sample * 32000))
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(bytes(frames))


def center(widget):
    x0, y0, x1, y1 = widget["rect"]
    return ((x0 + x1) / 2, (y0 + y1) / 2)


with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    fixtures = Path(__file__).resolve().parents[1] / "fixtures"
    shutil.copy(fixtures / "images" / "sample.png", root / "sample.png")
    shutil.copy(fixtures / "gltf" / "LittlestTokyo.glb", root / "tokyo.glb")
    shutil.copy(fixtures / "fonts" / "DejaVuSans.ttf", root / "DejaVuSans.ttf")
    shutil.copy(fixtures / "video" / "clip.mp4", root / "clip.mp4")
    (root / "logo.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 32">'
        '<rect width="64" height="32" rx="6" fill="#e0607e"/>'
        '<circle cx="16" cy="16" r="9" fill="#ffffff" fill-opacity="0.7"/></svg>'
    )
    chirp(root / "chirp.wav")
    people = [("Ada", 36, "London"), ("Linus", 28, "Helsinki"), ("Grace", 85, "New York"),
              ("Alan", 41, "Wilmslow"), ("Barbara", 69, "Boston"), ("Ken", 77, "Murray Hill")]
    (root / "people.csv").write_text(
        "name,age,city\n" + "".join(f"{n},{a},{c}\n" for n, a, c in people))
    (root / "data.bin").write_bytes(bytes(range(256)) * 8 + b"needle in the bytes" + bytes(64))

    with Bed(folder) as bed:
        until(bed, lambda: bed.exists("data.bin"), "the root is listed")

        # Binary data opens in hex; Cmd+F finds text in it.
        bed.click(row(bed, "data.bin"))
        until(bed, lambda: bed.exists(contains="0x0"), "data.bin shows in hex")
        bed.click(center(row(bed, "data.bin")))
        tab = max(bed.find_all("data.bin"), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = tab["rect"]
        bed.click((x0 + 200, y1 + 200))
        bed.key("Primary+F")
        until(bed, lambda: any(w["role"] == "TextInput" for w in bed.widgets()), "the find box opens")
        bed.type("needle")
        bed.key("Enter")
        until(bed, lambda: bed.exists(contains="0x800 · 6 bytes"), "needle is found and selected")
        bed.screenshot("viewers-hex")
        assert not bed.status()["errors"], bed.status()["errors"]

        # An image fills its panel; the floating bar zooms; SVG isn't text.
        bed.click(row(bed, "sample.png"))
        until(bed, lambda: bed.exists(contains="· PNG"), "sample.png shows as an image")
        bed.click(row(bed, "logo.svg"))
        until(bed, lambda: bed.exists("64 × 32 · SVG"), "logo.svg shows as an image")
        tab = max(bed.find_all("logo.svg"), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = tab["rect"]
        bed.move(x0 + 300, y1 + 300)
        bed.wait(10)
        zoom = lambda: bed.find(contains="%")["value"]
        before = zoom()
        bed.click(bed.find("plus"))
        until(bed, lambda: zoom() != before,
              "zoom in changes the zoom")
        bed.screenshot("viewers-image")
        assert not bed.status()["errors"], bed.status()["errors"]

        # A model fills its panel, with floating controls.
        bed.click(row(bed, "tokyo.glb"))
        until(bed, lambda: bed.exists("info"), "tokyo.glb shows as a model")
        until(bed, lambda: not bed.exists(contains="Loading model"), "the model loads")
        tab = max(bed.find_all("tokyo.glb"), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = tab["rect"]
        bed.drag((x0 + 300, y1 + 300), (x0 + 380, y1 + 280))
        bed.wait(30)
        bed.screenshot("viewers-model")
        assert not bed.status()["errors"], bed.status()["errors"]

        # A font: its preview, then every glyph; one is inspected.
        bed.click(row(bed, "DejaVuSans.ttf"))
        until(bed, lambda: bed.exists("DejaVu Sans"), "the font shows")
        bed.screenshot("viewers-font")
        tab = max(bed.find_all("DejaVuSans.ttf"), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = tab["rect"]
        bed.move(x0 + 300, y1 + 300)
        bed.wait(10)
        bed.click(bed.find("grid"))
        until(bed, lambda: bed.exists("0041"), "the glyphs show")
        bed.click(bed.find("0041"))
        until(bed, lambda: bed.exists(contains="U+0041"), "A is inspected")
        bed.screenshot("viewers-glyph")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Audio: a waveform to click along; the spectrogram. (Nothing plays.)
        bed.click(row(bed, "chirp.wav"))
        until(bed, lambda: bed.exists(contains="0:00 / 0:04"), "chirp.wav shows")
        tab = max(bed.find_all("chirp.wav"), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = tab["rect"]
        width = bed.status()["window"]["width"]
        bed.click((x0 + 20 + (width - 20 - x0 - 40) * 0.6, y1 + 250))
        until(bed, lambda: bed.exists(contains="0:02 / 0:04"), "a click seeks")
        bed.screenshot("viewers-waveform")
        bed.click(bed.find("layers"))
        bed.wait(10)
        bed.screenshot("viewers-spectrogram")
        bed.click(bed.find("spectrum"))
        bed.wait(10)
        bed.screenshot("viewers-spectrum")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Video (a silent clip): it shows, and plays.
        bed.click(row(bed, "clip.mp4"))
        until(bed, lambda: bed.exists(contains="0:00 / 0:02"), "clip.mp4 shows")
        bed.wait(20)
        bed.screenshot("viewers-video-paused")
        bed.click(bed.find("play"))
        until(bed, lambda: bed.exists(contains="0:01 / 0:02"), "it plays")
        bed.screenshot("viewers-video")
        assert not bed.status()["errors"], bed.status()["errors"]

        # A table: sort by a column, edit a cell and save, filter.
        bed.click(row(bed, "people.csv"))
        until(bed, lambda: bed.exists("Helsinki"), "people.csv shows as a table")
        y = lambda label: bed.find(label)["rect"][1]
        assert y("Ada") < y("Linus"), "in file order"
        bed.click(bed.find("age"))
        until(bed, lambda: y("Linus") < y("Ada"), "sorted by age")
        bed.input({"move": {"x": center(bed.find("Helsinki"))[0], "y": center(bed.find("Helsinki"))[1]}},
                  {"press": "Left"}, {"release": "Left"}, {"press": "Left"}, {"release": "Left"})
        until(bed, lambda: any(w["role"] == "TextInput" and w.get("value") == "Helsinki" for w in bed.widgets()),
              "the cell is edited")
        bed.key("Primary+A")
        bed.type("Espoo")
        bed.key("Enter")
        until(bed, lambda: bed.exists("Espoo"), "the cell changes")
        bed.key("Primary+S")
        until(bed, lambda: "Linus,28,Espoo" in (root / "people.csv").read_text(), "it's saved")
        x0, y0, x1, y1 = bed.find("Espoo")["rect"]
        bed.move(x1, y1)
        bed.wait(10)
        bed.click(bed.find("filter"))
        bed.type("york")
        until(bed, lambda: bed.exists(contains="1 of 6 rows"), "the filter shows one row")
        bed.screenshot("viewers-table")
        assert not bed.status()["errors"], bed.status()["errors"]
        bed.click([w for w in bed.find_all("close") if w["rect"][0] > 800][0])

        # The table is a view of the text: the editor, in its own panel,
        # shows the same document, and the table follows its edits.
        bed.click(row(bed, "people.csv"), button="Right")
        bed.click(bed.find("Open With ⏵"))
        bed.click(bed.find("Text Editor"))
        until(bed, lambda: len(bed.find_all("people.csv")) >= 3, "people.csv opens in the editor too")
        tabs = sorted(bed.find_all("people.csv"), key=lambda widget: widget["rect"][0])
        x0, y0, x1, y1 = tabs[-1]["rect"]
        bed.click((x0 + 40, y1 + 120))
        bed.key("Primary+A")
        bed.type("name,age,city")
        bed.key("Enter")
        bed.type("Zed,1,Oslo")
        bed.click(tabs[1])
        until(bed, lambda: bed.exists("Oslo"), "the table shows the editor's text")
        assert not bed.exists("Helsinki"), "the old rows are gone"
        bed.screenshot("viewers-table-text")
        assert not bed.status()["errors"], bed.status()["errors"]
