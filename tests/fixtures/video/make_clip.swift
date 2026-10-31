// Writes a short H.264 test clip: a moving gradient with a bouncing square.
// Usage: swift make_clip.swift out.mp4
import AVFoundation
import CoreVideo

let output = URL(fileURLWithPath: CommandLine.arguments[1])
try? FileManager.default.removeItem(at: output)
let (width, height, fps, seconds) = (320, 180, 30, 2)
let writer = try! AVAssetWriter(outputURL: output, fileType: .mp4)
let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
    AVVideoCodecKey: AVVideoCodecType.h264,
    AVVideoWidthKey: width,
    AVVideoHeightKey: height,
])
let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input, sourcePixelBufferAttributes: [
    kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
    kCVPixelBufferWidthKey as String: width,
    kCVPixelBufferHeightKey as String: height,
])
writer.add(input)
writer.startWriting()
writer.startSession(atSourceTime: .zero)
for frame in 0..<(fps * seconds) {
    while !input.isReadyForMoreMediaData { usleep(1000) }
    var buffer: CVPixelBuffer?
    CVPixelBufferPoolCreatePixelBuffer(nil, adaptor.pixelBufferPool!, &buffer)
    CVPixelBufferLockBaseAddress(buffer!, [])
    let base = CVPixelBufferGetBaseAddress(buffer!)!.assumingMemoryBound(to: UInt8.self)
    let stride = CVPixelBufferGetBytesPerRow(buffer!)
    let t = Double(frame) / Double(fps * seconds)
    let square = Int(t * Double(width - 40))
    for y in 0..<height {
        for x in 0..<width {
            let p = base + y * stride + x * 4
            let inside = x >= square && x < square + 40 && y >= 70 && y < 110
            p[0] = inside ? 255 : UInt8(80 + 120 * x / width)   // B
            p[1] = inside ? 255 : UInt8(40 + 60 * y / height)   // G
            p[2] = inside ? 255 : UInt8(160 * (1 - t))           // R
            p[3] = 255
        }
    }
    CVPixelBufferUnlockBaseAddress(buffer!, [])
    adaptor.append(buffer!, withPresentationTime: CMTime(value: CMTimeValue(frame), timescale: CMTimeScale(fps)))
}
input.markAsFinished()
let done = DispatchSemaphore(value: 0)
writer.finishWriting { done.signal() }
done.wait()
