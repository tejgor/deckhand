// Builds assets/Deckhand.icns from the brand mark in assets/deckhand.png.
//
// The mark is full bleed, so it is drawn into the macOS app-icon template (an 824 px rounded square on a
// 1024 px canvas) at every iconset size; Finder and Notification Center then show it like any other app.
//
// Usage: swift scripts/make-icns.swift   (run from the repository root; needs iconutil)

import AppKit

let source = "assets/deckhand.png"
let output = "assets/Deckhand.icns"

guard let mark = NSImage(contentsOfFile: source),
      let markImage = mark.cgImage(forProposedRect: nil, context: nil, hints: nil)
else {
    FileHandle.standardError.write("cannot read \(source)\n".data(using: .utf8)!)
    exit(1)
}

func render(_ pixels: Int) -> Data {
    let scale = CGFloat(pixels) / 1024
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let context = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0, space: space,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.scaleBy(x: scale, y: scale)
    context.addPath(CGPath(roundedRect: body, cornerWidth: 185.4, cornerHeight: 185.4, transform: nil))
    context.clip()
    // Pixel art: keep the edges hard where the size allows it.
    context.interpolationQuality = pixels >= 512 ? .none : .high
    context.draw(markImage, in: body)
    let rep = NSBitmapImageRep(cgImage: context.makeImage()!)
    return rep.representation(using: .png, properties: [:])!
}

let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("Deckhand-\(getpid()).iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: iconset) }
for points in [16, 32, 128, 256, 512] {
    try render(points).write(to: iconset.appendingPathComponent("icon_\(points)x\(points).png"))
    try render(points * 2).write(to: iconset.appendingPathComponent("icon_\(points)x\(points)@2x.png"))
}

let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", output]
try iconutil.run()
iconutil.waitUntilExit()
exit(iconutil.terminationStatus)
