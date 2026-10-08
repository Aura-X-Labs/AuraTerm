#!/usr/bin/env swift
// Draws the AuraTerm icon and writes every raster copy of it:
//
//   src-tauri/icons/icon.icns   macOS app icon
//   src-tauri/icons/icon.png    Linux bundle, README, MSIX logo source
//   src-tauri/icons/icon.ico    Windows executable, installer and shortcut
//   src/logo.png                titlebar and About dialog
//
//   swift scripts/make_icons.swift [repo-root]
//
// macOS 26 clips every app icon with its own rounded shape. The plate drawn
// here is that shape exactly, and the silver ring is a constant-width inset of
// it, so the system clip lands on the plate edge and the ring stays even.

import AppKit
import CoreText
import ImageIO
import UniformTypeIdentifiers

// Design space: 1024 x 1024, origin top-left, y down.
let canvas: CGFloat = 1024
let plate = CGRect(x: 100, y: 100, width: 824, height: 824)
// Measured from the mask macOS 26 applies; Big Sur through Sequoia used 185.4.
let cornerRadius: CGFloat = 214
let ringWidth: CGFloat = 48
let outlineWidth: CGFloat = 13

enum Style {
    // Plate on the macOS icon grid with a drop shadow. No outline: the system
    // clip would cut into it, and macOS draws its own edge.
    case macOS
    // Plate plus a dark outline filling the whole canvas, for everywhere the
    // icon is shown as-is on an arbitrary background.
    case fullBleed
}

func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: a)
}

let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

func gradient(_ stops: [(CGFloat, CGColor)]) -> CGGradient {
    let last = stops[stops.count - 1].0
    return CGGradient(
        colorsSpace: sRGB,
        colors: stops.map { $0.1 } as CFArray,
        locations: stops.map { $0.0 / last }
    )!
}

// Rounded rect with continuous-curvature corners (the UIBezierPath / app icon
// shape) rather than circular arcs.
func continuousRoundedRect(_ rect: CGRect, radius r: CGFloat) -> CGPath {
    let path = CGMutablePath()
    // One corner, as offsets from the corner point in units of the radius:
    // `along` runs back down the incoming edge, `across` into the shape.
    let segments: [[(CGFloat, CGFloat)]] = [
        [(1.08849323, 0), (0.86840689, 0), (0.66993427, 0.06549569)],
        [(0.37282392, 0.16905883), (0.16905883, 0.37282392), (0.07491100, 0.63149399)],
        [(0, 0.86840689), (0, 1.08849323), (0, 1.52866483)],
    ]
    let joint: (CGFloat, CGFloat) = (0.63149399, 0.07491100)
    // Corners clockwise from top-right: corner point, incoming edge direction,
    // and the direction pointing into the shape.
    let corners: [(CGPoint, CGVector, CGVector)] = [
        (CGPoint(x: rect.maxX, y: rect.minY), CGVector(dx: 1, dy: 0), CGVector(dx: 0, dy: 1)),
        (CGPoint(x: rect.maxX, y: rect.maxY), CGVector(dx: 0, dy: 1), CGVector(dx: -1, dy: 0)),
        (CGPoint(x: rect.minX, y: rect.maxY), CGVector(dx: -1, dy: 0), CGVector(dx: 0, dy: -1)),
        (CGPoint(x: rect.minX, y: rect.minY), CGVector(dx: 0, dy: -1), CGVector(dx: 1, dy: 0)),
    ]
    for (index, (corner, edge, inward)) in corners.enumerated() {
        func point(_ offset: (CGFloat, CGFloat)) -> CGPoint {
            CGPoint(
                x: corner.x - edge.dx * offset.0 * r + inward.dx * offset.1 * r,
                y: corner.y - edge.dy * offset.0 * r + inward.dy * offset.1 * r
            )
        }
        let start = point((1.52866483, 0))
        if index == 0 { path.move(to: start) } else { path.addLine(to: start) }
        path.addCurve(to: point(segments[0][2]), control1: point(segments[0][0]), control2: point(segments[0][1]))
        path.addLine(to: point(joint))
        path.addCurve(to: point(segments[1][2]), control1: point(segments[1][0]), control2: point(segments[1][1]))
        path.addCurve(to: point(segments[2][2]), control1: point(segments[2][0]), control2: point(segments[2][1]))
    }
    path.closeSubpath()
    return path
}

func drawIcon(in ctx: CGContext, pixels: Int, style: Style) {
    let extent = style == .macOS
        ? CGRect(x: 0, y: 0, width: canvas, height: canvas)
        : plate.insetBy(dx: -outlineWidth, dy: -outlineWidth)
    let scale = CGFloat(pixels) / extent.width
    ctx.translateBy(x: 0, y: CGFloat(pixels))
    ctx.scaleBy(x: scale, y: -scale)
    ctx.translateBy(x: -extent.minX, y: -extent.minY)

    let shape = continuousRoundedRect(plate, radius: cornerRadius)

    switch style {
    case .macOS:
        // Drop shadow for systems that do not add their own (shadow parameters
        // are in device space, so they are scaled by hand).
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -16 * scale), blur: 32 * scale, color: rgb(0, 0, 0, 0.45))
        ctx.addPath(shape)
        ctx.setFillColor(rgb(22, 22, 22))
        ctx.fillPath()
        ctx.restoreGState()
    case .fullBleed:
        // The outer half of this stroke is the outline; the plate covers the rest.
        ctx.addPath(shape)
        ctx.setLineWidth(outlineWidth * 2)
        ctx.setLineJoin(.round)
        ctx.setStrokeColor(rgb(13, 13, 13))
        ctx.strokePath()
    }

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()

    // Screen: dark panel lit from the upper left.
    let panel = gradient([
        (0, rgb(73, 73, 75)), (95, rgb(73, 73, 75)), (315, rgb(48, 48, 51)),
        (473, rgb(34, 34, 37)), (599, rgb(23, 23, 24)), (652, rgb(22, 22, 22)),
    ])
    ctx.drawRadialGradient(
        panel,
        startCenter: CGPoint(x: 373, y: 184), startRadius: 0,
        endCenter: CGPoint(x: 373, y: 184), endRadius: 652,
        options: [.drawsAfterEndLocation]
    )

    // Bezel: the inner half of a stroke along the plate edge, which keeps the
    // ring the same width along the straights and around the corners.
    ctx.saveGState()
    ctx.addPath(shape)
    ctx.setLineWidth(ringWidth * 2)
    ctx.setLineJoin(.round)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    // Brushed-metal bands running along the top-left to bottom-right diagonal;
    // stop positions are x + y.
    let metalFrom: CGFloat = 263
    let metal = gradient([
        (0, rgb(197, 218, 226)), (357, rgb(140, 150, 158)), (830, rgb(230, 238, 242)),
        (1324, rgb(110, 120, 126)), (1493, rgb(180, 194, 201)),
    ])
    ctx.drawLinearGradient(
        metal,
        start: CGPoint(x: metalFrom / 2, y: metalFrom / 2),
        end: CGPoint(x: (metalFrom + 1493) / 2, y: (metalFrom + 1493) / 2),
        options: [.drawsBeforeStartLocation, .drawsAfterEndLocation]
    )
    ctx.restoreGState()

    // Prompt: ">_"
    ctx.setStrokeColor(rgb(225, 237, 246))
    ctx.setLineWidth(36)
    ctx.setLineCap(.round)
    ctx.setLineJoin(.round)
    ctx.move(to: CGPoint(x: 300, y: 342))
    ctx.addLine(to: CGPoint(x: 402, y: 404.5))
    ctx.addLine(to: CGPoint(x: 300, y: 467))
    ctx.move(to: CGPoint(x: 438.5, y: 467))
    ctx.addLine(to: CGPoint(x: 566.5, y: 467))
    ctx.strokePath()

    // Protocol list
    let font = CTFontCreateWithName("HelveticaNeue-Medium" as CFString, 80, nil)
    let attributes: [NSAttributedString.Key: Any] = [
        NSAttributedString.Key(kCTFontAttributeName as String): font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String): rgb(206, 214, 219),
        NSAttributedString.Key(kCTKernAttributeName as String): 1.3,
    ]
    ctx.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
    for (text, baseline) in [("ssh", 625.0), ("telnet", 714.5), ("serial", 804.0)] {
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
        ctx.textPosition = CGPoint(x: 289, y: baseline)
        CTLineDraw(line, ctx)
    }

    ctx.restoreGState()
}

func render(pixels: Int, style: Style) throws -> CGContext {
    guard let ctx = CGContext(
        data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0,
        space: sRGB, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { throw CocoaError(.fileWriteUnknown) }
    ctx.interpolationQuality = .high
    ctx.saveGState()
    drawIcon(in: ctx, pixels: pixels, style: style)
    ctx.restoreGState()
    return ctx
}

func pngData(_ ctx: CGContext) throws -> Data {
    let data = NSMutableData()
    guard let image = ctx.makeImage(),
          let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)
    else { throw CocoaError(.fileWriteUnknown) }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
    return data as Data
}

extension Data {
    mutating func append<T: FixedWidthInteger>(littleEndian value: T) {
        Swift.withUnsafeBytes(of: value.littleEndian) { append(contentsOf: $0) }
    }
}

// One .ico frame as a 32-bit bitmap: header, bottom-up BGRA rows with straight
// alpha, then the 1-bit transparency mask.
func icoBitmap(_ ctx: CGContext) -> Data {
    let size = ctx.width
    let pixels = ctx.data!.assumingMemoryBound(to: UInt8.self)
    let maskRowBytes = (size + 31) / 32 * 4
    var colour = Data(capacity: size * size * 4)
    var mask = Data(capacity: maskRowBytes * size)
    for y in stride(from: size - 1, through: 0, by: -1) {
        var maskRow = [UInt8](repeating: 0, count: maskRowBytes)
        for x in 0..<size {
            let p = pixels + y * ctx.bytesPerRow + x * 4
            let alpha = Int(p[3])
            if alpha == 0 {
                colour.append(contentsOf: [0, 0, 0, 0])
                maskRow[x / 8] |= 0x80 >> UInt8(x % 8)
            } else {
                let straight = { (c: UInt8) in UInt8(min(255, (Int(c) * 255 + alpha / 2) / alpha)) }
                colour.append(contentsOf: [straight(p[2]), straight(p[1]), straight(p[0]), UInt8(alpha)])
            }
        }
        mask.append(contentsOf: maskRow)
    }
    var data = Data()
    data.append(littleEndian: UInt32(40))
    data.append(littleEndian: Int32(size))
    data.append(littleEndian: Int32(size * 2))
    data.append(littleEndian: UInt16(1))
    data.append(littleEndian: UInt16(32))
    data.append(littleEndian: UInt32(0))
    data.append(littleEndian: UInt32(colour.count + mask.count))
    for _ in 0..<4 { data.append(littleEndian: UInt32(0)) }
    return data + colour + mask
}

func icoData(sizes: [Int], style: Style) throws -> Data {
    // 256 px is stored as PNG, smaller frames as bitmaps, as Windows expects.
    let frames = try sizes.map { size -> Data in
        let ctx = try render(pixels: size, style: style)
        return size >= 256 ? try pngData(ctx) : icoBitmap(ctx)
    }
    var data = Data()
    data.append(littleEndian: UInt16(0))
    data.append(littleEndian: UInt16(1))
    data.append(littleEndian: UInt16(sizes.count))
    var offset = 6 + 16 * sizes.count
    for (size, frame) in zip(sizes, frames) {
        data.append(contentsOf: [UInt8(size % 256), UInt8(size % 256), 0, 0])
        data.append(littleEndian: UInt16(1))
        data.append(littleEndian: UInt16(32))
        data.append(littleEndian: UInt32(frame.count))
        data.append(littleEndian: UInt32(offset))
        offset += frame.count
    }
    return frames.reduce(data, +)
}

// Defaults to the repository this script lives in, wherever it is run from.
let root = CommandLine.arguments.count > 1
    ? URL(fileURLWithPath: CommandLine.arguments[1])
    : URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("..").standardizedFileURL
let icons = root.appendingPathComponent("src-tauri/icons")

func write(_ data: Data, to url: URL) throws {
    try data.write(to: url)
    print("wrote \(url.path)")
}

let iconset = FileManager.default.temporaryDirectory
    .appendingPathComponent("auraterm-\(ProcessInfo.processInfo.processIdentifier).iconset")
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: iconset) }

for points in [16, 32, 128, 256, 512] {
    try pngData(render(pixels: points, style: .macOS))
        .write(to: iconset.appendingPathComponent("icon_\(points)x\(points).png"))
    try pngData(render(pixels: points * 2, style: .macOS))
        .write(to: iconset.appendingPathComponent("icon_\(points)x\(points)@2x.png"))
}

let icns = icons.appendingPathComponent("icon.icns")
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", icns.path]
try iconutil.run()
iconutil.waitUntilExit()
guard iconutil.terminationStatus == 0 else { exit(iconutil.terminationStatus) }
print("wrote \(icns.path)")

try write(pngData(render(pixels: 512, style: .fullBleed)), to: icons.appendingPathComponent("icon.png"))
try write(
    icoData(sizes: [16, 24, 32, 48, 64, 72, 80, 96, 128, 256], style: .fullBleed),
    to: icons.appendingPathComponent("icon.ico")
)
try write(pngData(render(pixels: 512, style: .fullBleed)), to: root.appendingPathComponent("src/logo.png"))
