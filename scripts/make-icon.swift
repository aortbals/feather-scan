// Draws the Feather Scan app icon from the menu bar feather (FeatherIcon.swift)
// and writes macos/Resources/AppIcon.icns, docs/icon.png and docs/feather.svg.
// Run via scripts/make-icon.sh, which compiles this together with FeatherIcon.swift.
import AppKit

@main
enum MakeIcon {
    static let canvas: CGFloat = 1024
    static let rgb = CGColorSpaceCreateDeviceRGB()

    static func main() {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        writeIcns(to: root.appendingPathComponent("macos/Resources/AppIcon.icns"))
        writeSVG(to: root.appendingPathComponent("docs/feather.svg"))
        // README header image.
        try! render(size: 256).representation(using: .png, properties: [:])!
            .write(to: root.appendingPathComponent("docs/icon.png"))
        if CommandLine.arguments.count > 2 {
            let png = render(size: 1024).representation(using: .png, properties: [:])!
            try! png.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
        }
    }

    // MARK: Icon

    static func render(size px: Int) -> NSBitmapImageRep {
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        let ctx = NSGraphicsContext.current!.cgContext
        let k = CGFloat(px) / canvas
        ctx.scaleBy(x: k, y: k)
        drawIcon(ctx, shadowScale: k)
        NSGraphicsContext.restoreGraphicsState()
        return rep
    }

    static func color(_ r: Double, _ g: Double, _ b: Double, _ a: Double = 1) -> CGColor {
        NSColor(srgbRed: r, green: g, blue: b, alpha: a).cgColor
    }

    static func gradient(_ colors: [CGColor], _ locations: [CGFloat]) -> CGGradient {
        CGGradient(colorsSpace: rgb, colors: colors as CFArray, locations: locations)!
    }

    /// Shadow offsets and blur are in device pixels, so they scale with the icon size.
    static func drawIcon(_ ctx: CGContext, shadowScale k: CGFloat) {
        // macOS icon grid: an 824 pt rounded square centered on the 1024 canvas.
        let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
        let tileShape = CGPath(roundedRect: tile, cornerWidth: 186, cornerHeight: 186, transform: nil)

        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -12 * k), blur: 28 * k, color: color(0, 0, 0, 0.35))
        ctx.addPath(tileShape)
        ctx.setFillColor(color(0, 0, 0))
        ctx.fillPath()
        ctx.restoreGState()

        ctx.saveGState()
        ctx.addPath(tileShape)
        ctx.clip()
        // Dark tile: charcoal, a touch lighter at the top.
        ctx.drawLinearGradient(
            gradient([color(0.035, 0.04, 0.055), color(0.10, 0.11, 0.14)], [0, 1]),
            start: CGPoint(x: 0, y: 100), end: CGPoint(x: 0, y: 924), options: [])
        // Faint teal glow behind the feather, like light on a peacock's plumage.
        ctx.drawRadialGradient(
            gradient([color(0.05, 0.45, 0.50, 0.28), color(0.05, 0.45, 0.50, 0)], [0, 1]),
            startCenter: CGPoint(x: 560, y: 560), startRadius: 0,
            endCenter: CGPoint(x: 560, y: 560), endRadius: 470, options: [])
        // Bevel: a thin light rim along the top edge of the tile.
        ctx.addPath(tileShape)
        ctx.setLineWidth(6)
        ctx.replacePathWithStrokedPath()
        ctx.clip()
        ctx.drawLinearGradient(
            gradient([color(1, 1, 1, 0.18), color(1, 1, 1, 0)], [0, 1]),
            start: CGPoint(x: 0, y: 924), end: CGPoint(x: 0, y: 600), options: [])
        ctx.restoreGState()

        // The menu bar feather, tilted like a pen, colored like a peacock feather:
        // indigo at the quill through cobalt, teal and emerald to gold at the tip.
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -10 * k), blur: 26 * k, color: color(0, 0, 0, 0.6))
        ctx.beginTransparencyLayer(auxiliaryInfo: nil)
        ctx.translateBy(x: 512, y: 512)
        ctx.rotate(by: FeatherIcon.tilt)
        ctx.scaleBy(x: 650, y: 650)
        ctx.translateBy(x: 0, y: -0.5)
        FeatherIcon.drawFeather(in: ctx)
        ctx.setBlendMode(.sourceIn)
        ctx.drawLinearGradient(
            gradient(
                [
                    color(0.27, 0.18, 0.72),  // indigo
                    color(0.12, 0.38, 0.92),  // cobalt
                    color(0.02, 0.68, 0.74),  // teal
                    color(0.18, 0.80, 0.42),  // emerald
                    color(0.93, 0.77, 0.28),  // gold
                ],
                [0, 0.3, 0.55, 0.78, 1]),
            start: CGPoint(x: 0, y: 0.05), end: CGPoint(x: 0, y: 1.0),
            options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
        // A soft sheen across the vane for an iridescent feel.
        ctx.setBlendMode(.sourceAtop)
        ctx.drawLinearGradient(
            gradient([color(1, 1, 1, 0), color(1, 1, 1, 0.18), color(1, 1, 1, 0)], [0, 0.5, 1]),
            start: CGPoint(x: -0.25, y: 0.35), end: CGPoint(x: 0.2, y: 0.75), options: [])
        ctx.endTransparencyLayer()
        ctx.restoreGState()
    }

    static func writeIcns(to out: URL) {
        let iconset = FileManager.default.temporaryDirectory.appendingPathComponent("AppIcon.iconset")
        try? FileManager.default.removeItem(at: iconset)
        try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
        for base in [16, 32, 128, 256, 512] {
            for scale in [1, 2] {
                let name = scale == 1 ? "icon_\(base)x\(base).png" : "icon_\(base)x\(base)@2x.png"
                try! render(size: base * scale).representation(using: .png, properties: [:])!
                    .write(to: iconset.appendingPathComponent(name))
            }
        }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
        p.arguments = ["-c", "icns", iconset.path, "-o", out.path]
        try! p.run()
        p.waitUntilExit()
        print(p.terminationStatus == 0 ? "wrote \(out.path)" : "iconutil failed")
    }

    // MARK: SVG

    /// The menu bar feather as a single-color SVG (shaft cut out with a mask).
    static func writeSVG(to out: URL) {
        let size = 100.0
        // Unit space → SVG space: tilt about the center, y down.
        let t = CGAffineTransform(translationX: size / 2, y: size / 2)
            .scaledBy(x: 1, y: -1)
            .rotated(by: FeatherIcon.tilt)
            .scaledBy(x: 88, y: 88)
            .translatedBy(x: 0, y: -0.5)
        let f = { (v: CGFloat) in String(format: "%.2f", v) }
        func svgPath(_ path: CGPath) -> String {
            var d = ""
            path.applyWithBlock { el in
                let e = el.pointee
                let pt = { (i: Int) in e.points[i].applying(t) }
                switch e.type {
                case .moveToPoint: d += "M\(f(pt(0).x)) \(f(pt(0).y))"
                case .addLineToPoint: d += "L\(f(pt(0).x)) \(f(pt(0).y))"
                case .addQuadCurveToPoint: d += "Q\(f(pt(0).x)) \(f(pt(0).y)) \(f(pt(1).x)) \(f(pt(1).y))"
                case .addCurveToPoint:
                    d += "C\(f(pt(0).x)) \(f(pt(0).y)) \(f(pt(1).x)) \(f(pt(1).y)) \(f(pt(2).x)) \(f(pt(2).y))"
                case .closeSubpath: d += "Z"
                @unknown default: break
                }
            }
            return d
        }
        func capsule(_ seg: (from: CGPoint, to: CGPoint, width: Double)) -> CGPath {
            let r = seg.width / 2
            return CGPath(
                roundedRect: CGRect(x: seg.from.x - r, y: seg.from.y - r, width: seg.width, height: seg.to.y - seg.from.y + seg.width),
                cornerWidth: r, cornerHeight: r, transform: nil)
        }
        // One outline: vane plus quill, minus the shaft capsule.
        let outline = FeatherIcon.vanePath
            .union(capsule(FeatherIcon.quill))
            .subtracting(capsule(FeatherIcon.shaft))
        let svg = """
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100">
          <title>Feather Scan</title>
          <path d="\(svgPath(outline))" fill="currentColor"/>
        </svg>

        """
        try? FileManager.default.createDirectory(at: out.deletingLastPathComponent(), withIntermediateDirectories: true)
        try! svg.write(to: out, atomically: true, encoding: .utf8)
        print("wrote \(out.path)")
    }
}
