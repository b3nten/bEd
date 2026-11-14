// Measure typing latency end to end on macOS: from a key event posted to an
// app until the window server has a frame of its window with the result.
//
//     swiftc -O tools/latency.swift -o target/latency && target/latency [options]
//
//     --app NAME       the app to type into (default: bed); its largest window
//     --count N        keystrokes to time (default: 60)
//     --interval S     seconds between them (default: 0.4: under the editor's
//                      0.7 s before the caret blinks again)
//     --debug          per key: frames, tiles changed, tiles changing on
//                      their own, and tiles back as they were two keys ago
//     --tile N         tile edge in points (default: 4: small enough that a
//                      glyph's tiles aren't the caret's, which may animate)
//
// Put the caret in a scratch buffer first: the probe types "x" and Backspace,
// alternately, into the app (posted to its process, not to whatever is in
// front). Frames come from ScreenCaptureKit, hashed in small tiles. A key's
// time is to the first frame that changes a tile it changed for good: still
// different once the screen settles, and settled exactly as two keys ago (the
// keys undo each other; animated ink never repeats), leaving out tiles that
// were changing on their own just before it. The display sends the frame on
// to the panel after this (about one more refresh), which isn't counted.
//
// The terminal running it needs Screen Recording and Accessibility (System
// Settings ▸ Privacy & Security).
import AppKit
import CoreMedia
import CoreVideo
import Foundation
import ScreenCaptureKit

var app = "bed"
var count = 60
var interval = 0.4
/// Edge of a tile, in points: each frame is kept as a hash per tile.
var tile = 4
var debug = false
var arguments = CommandLine.arguments.dropFirst().makeIterator()
while let argument = arguments.next() {
    switch argument {
    case "--app": app = arguments.next() ?? app
    case "--count": count = Int(arguments.next() ?? "") ?? count
    case "--interval": interval = Double(arguments.next() ?? "") ?? interval
    case "--debug": debug = true
    case "--tile": tile = max(1, Int(arguments.next() ?? "") ?? tile)
    default:
        FileHandle.standardError.write("unknown argument \(argument)\n".data(using: .utf8)!)
        exit(2)
    }
}

var timebase = mach_timebase_info_data_t()
mach_timebase_info(&timebase)
func ticks(_ seconds: Double) -> UInt64 {
    UInt64(seconds * 1e9 * Double(timebase.denom) / Double(timebase.numer))
}
/// How long before each key to look for tiles changing on their own: after
/// the caret's motion from the last key (0.11 s) has finished.
let quietBefore = 0.2
func milliseconds(_ ticks: UInt64) -> Double {
    Double(ticks) * Double(timebase.numer) / Double(timebase.denom) / 1_000_000
}


struct Frame {
    var time: UInt64
    var tiles: [UInt64]
}

/// Every new frame of the window: when it was displayed, and its tiles.
final class Frames: NSObject, SCStreamOutput {
    private let lock = NSLock()
    private var frames: [Frame] = []

    func stream(
        _ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard type == .screen,
            let attachments = CMSampleBufferGetSampleAttachmentsArray(
                buffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
            let info = attachments.first,
            let status = info[.status] as? Int, status == SCFrameStatus.complete.rawValue,
            let time = info[.displayTime] as? UInt64,
            let pixels = CMSampleBufferGetImageBuffer(buffer)
        else { return }
        let frame = Frame(time: time, tiles: Frames.tiles(pixels))
        lock.lock()
        frames.append(frame)
        lock.unlock()
    }

    static func tiles(_ pixels: CVPixelBuffer) -> [UInt64] {
        CVPixelBufferLockBaseAddress(pixels, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixels, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pixels) else { return [] }
        let row = CVPixelBufferGetBytesPerRow(pixels)
        let width = CVPixelBufferGetWidth(pixels)
        let height = CVPixelBufferGetHeight(pixels)
        let columns = (width + tile - 1) / tile
        var hashes = [UInt64](
            repeating: 0xcbf2_9ce4_8422_2325, count: columns * ((height + tile - 1) / tile))
        for y in 0..<height {
            let pixel = (base + y * row).assumingMemoryBound(to: UInt32.self)
            let first = (y / tile) * columns
            for x in 0..<width {
                let index = first + x / tile
                hashes[index] = (hashes[index] ^ UInt64(pixel[x])) &* 0x100_0000_01b3
            }
        }
        return hashes
    }

    /// The frames since the last call.
    func take() -> [Frame] {
        lock.lock()
        defer { lock.unlock() }
        let taken = frames
        frames.removeAll(keepingCapacity: true)
        return taken
    }
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

if !CGPreflightScreenCaptureAccess() {
    CGRequestScreenCaptureAccess()
    fail("Allow Screen Recording for this terminal, then run again.")
}
if !AXIsProcessTrusted() {
    fail("Allow Accessibility for this terminal (to post keys), then run again.")
}

let content = try await SCShareableContent.excludingDesktopWindows(
    true, onScreenWindowsOnly: true)
guard
    let window = content.windows
        .filter({
            $0.owningApplication?.applicationName.lowercased() == app.lowercased()
                && $0.windowLayer == 0
        })
        .max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }),
    let pid = window.owningApplication?.processID
else { fail("No window of \(app) on screen.") }

NSRunningApplication(processIdentifier: pid)?.activate()
try await Task.sleep(for: .seconds(0.5))

let configuration = SCStreamConfiguration()
// Points, not pixels: a typed character still changes them, at a quarter of
// the hashing.
configuration.width = Int(window.frame.width)
configuration.height = Int(window.frame.height)
configuration.minimumFrameInterval = CMTime(value: 1, timescale: 240)
configuration.queueDepth = 8
configuration.showsCursor = false
configuration.pixelFormat = kCVPixelFormatType_32BGRA
let frames = Frames()
let stream = SCStream(
    filter: SCContentFilter(desktopIndependentWindow: window), configuration: configuration,
    delegate: nil)
try stream.addStreamOutput(
    frames, type: .screen,
    sampleHandlerQueue: DispatchQueue(label: "frames", qos: .userInteractive))
try await withCheckedThrowingContinuation { (done: CheckedContinuation<Void, Error>) in
    stream.startCapture { error in
        if let error { done.resume(throwing: error) } else { done.resume() }
    }
}
try await Task.sleep(for: .seconds(1))

let source = CGEventSource(stateID: .hidSystemState)
func press(_ key: CGKeyCode) {
    CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: true)!.postToPid(pid)
    CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: false)!.postToPid(pid)
}
let keys: [(name: String, code: CGKeyCode)] = [("x", 7), ("Backspace", 51)]

print("Typing into \(app) (pid \(pid)), \(count) keys, \(interval) s apart…")
var samples: [String: [Double]] = [:]
var missed = 0
var noisiest = 0
var latest = frames.take()
/// The screen once each key had settled.
var settledAfter: [Frame] = []
// Two more, untimed, first: the caret may be mid-blink.
for index in 0..<(count + 2) {
    let key = keys[index % 2]
    // Tiles that changed on their own in the quiet just before the key
    // (animation) don't count for it. Not earlier: the last key's own change
    // is in the same tiles.
    latest += frames.take()
    guard let before = latest.last else { fail("No frames of the window.") }
    let quiet = mach_absolute_time() - ticks(quietBefore)
    var noisy = Set<Int>()
    for (earlier, later) in zip(latest, latest.dropFirst()) where later.time >= quiet {
        for t in earlier.tiles.indices where earlier.tiles[t] != later.tiles[t] {
            noisy.insert(t)
        }
    }
    noisiest = max(noisiest, noisy.count)
    let posted = mach_absolute_time()
    press(key.code)
    try await Task.sleep(for: .seconds(interval))
    let after = frames.take().filter { $0.time > posted }
    latest = after.isEmpty ? [before] : after
    let settled = latest.last!
    settledAfter.append(settled)
    guard index >= 2 else { continue }
    // What the key did: tiles it changed for good, back exactly as they
    // were two keys ago ("x" then Backspace undo each other). Animated ink
    // never repeats exactly, so it can't pass for the key.
    let twoAgo = settledAfter[index - 2]
    let changed = before.tiles.indices.filter {
        !noisy.contains($0) && settled.tiles[$0] != before.tiles[$0]
            && twoAgo.tiles.count == settled.tiles.count
            && settled.tiles[$0] == twoAgo.tiles[$0]
    }
    if debug {
        let differs = before.tiles.indices.filter { settled.tiles[$0] != before.tiles[$0] }
        let steady = differs.filter { !noisy.contains($0) }
        let columns = (Int(window.frame.width) + tile - 1) / tile
        let spots = steady.prefix(8).map { "(\($0 % columns * tile),\($0 / columns * tile))" }
        print(
            "\(key.name): \(after.count) frames, \(differs.count) tiles changed, "
                + "\(differs.count - steady.count) of them animating, \(changed.count) as two keys ago; "
                + "steady ones at \(spots.joined(separator: " "))")
    }
    if let shown = after.first(where: { frame in
        changed.contains { frame.tiles[$0] != before.tiles[$0] }
    }) {
        samples[key.name, default: []].append(milliseconds(shown.time - posted))
    } else {
        missed += 1
    }
}
stream.stopCapture { _ in }

func report(_ name: String, _ values: [Double]) {
    let sorted = values.sorted()
    guard !sorted.isEmpty else { return }
    let at = { (q: Double) in sorted[min(sorted.count - 1, Int(q * Double(sorted.count)))] }
    let mean = sorted.reduce(0, +) / Double(sorted.count)
    print(
        String(
            format: "%-10@ n=%3d  min %5.1f  median %5.1f  mean %5.1f  p90 %5.1f  max %5.1f ms",
            name as NSString, sorted.count, sorted[0], at(0.5), mean, at(0.9), sorted.last!))
}
for key in keys { report(key.name, samples[key.name] ?? []) }
let all = samples.values.flatMap { $0 }.sorted()
print("samples (ms):", all.map { String(format: "%.1f", $0) }.joined(separator: " "))
// Clusters, by display refresh: how many keys showed in each frame after.
let refresh = 1000.0 / Double(NSScreen.main?.maximumFramesPerSecond ?? 60)
var buckets: [Int: Int] = [:]
for value in all { buckets[Int(value / refresh), default: 0] += 1 }
print(
    String(format: "by %.1f ms refresh:", refresh),
    buckets.keys.sorted().map { "\($0)–\($0 + 1): \(buckets[$0]!)" }.joined(separator: "  "))
report("all", samples.values.flatMap { $0 })
if missed > 0 { print("\(missed) keys changed nothing on screen within \(interval) s") }
if noisiest > 0 { print("Ignored up to \(noisiest) \(tile)-point tiles that were changing on their own") }
