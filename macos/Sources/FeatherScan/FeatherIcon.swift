import AppKit

/// The menu bar feather, drawn in code as a template image so it follows the
/// menu bar's light/dark appearance.
enum FeatherIcon {
    static let size = NSSize(width: 18, height: 18)

    /// Resting feather. `dimmed` is used while offline.
    static func still(dimmed: Bool = false) -> NSImage {
        image { _ in dimmed ? 0.4 : 1 }
    }

    /// Frames of a shimmer for while a scan runs: the feather dims a little and
    /// a soft band of light sweeps from the quill to the tip, then rests.
    static let scanningFrames: [NSImage] = {
        let sweep = 14, rest = 6
        let base = 0.45
        return (0..<(sweep + rest)).map { i in
            guard i < sweep else { return image { _ in base } }
            // Band center travels from below the quill to past the tip.
            let center = -0.3 + 1.6 * Double(i) / Double(sweep - 1)
            return image { y in base + (1 - base) * exp(-pow((y - center) / 0.2, 2)) }
        }
    }()

    /// Draws the feather with opacity given per point along its length
    /// (`opacity(y)`, y = 0 at the quill, 1 at the tip).
    private static func image(opacity: @escaping (Double) -> Double) -> NSImage {
        let img = NSImage(size: size, flipped: false) { rect in
            guard let ctx = NSGraphicsContext.current?.cgContext else { return false }
            ctx.translateBy(x: rect.midX, y: rect.midY)
            // The quill leans to the right, like a pen.
            ctx.rotate(by: -38 * .pi / 180)
            ctx.scaleBy(x: 18, y: 18)
            ctx.translateBy(x: 0, y: -0.5)
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)
            drawFeather(in: ctx)
            // Recolor the drawn feather with an opacity gradient along its length.
            let steps = 32
            var colors: [CGColor] = []
            var locations: [CGFloat] = []
            for k in 0...steps {
                let y = Double(k) / Double(steps)
                colors.append(NSColor(white: 0, alpha: min(1, max(0, opacity(y)))).cgColor)
                locations.append(CGFloat(y))
            }
            if let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceGray(), colors: colors as CFArray, locations: locations) {
                ctx.setBlendMode(.sourceIn)
                ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: 0), end: CGPoint(x: 0, y: 1),
                                       options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
            }
            ctx.endTransparencyLayer()
            return true
        }
        img.isTemplate = true
        img.accessibilityDescription = "Feather Scan"
        return img
    }

    /// Feather in a unit box: base of the quill at y = 0, tip at y = 1.
    /// Kept bold and simple so it reads at 18 pt: a solid vane, one notch, a shaft line.
    private static func drawFeather(in ctx: CGContext) {
        let vane = CGMutablePath()
        vane.move(to: CGPoint(x: 0.0, y: 0.18))
        vane.addCurve(to: CGPoint(x: -0.23, y: 0.55), control1: CGPoint(x: -0.18, y: 0.24), control2: CGPoint(x: -0.24, y: 0.40))
        vane.addCurve(to: CGPoint(x: 0.0, y: 1.0), control1: CGPoint(x: -0.22, y: 0.74), control2: CGPoint(x: -0.10, y: 0.90))
        vane.addCurve(to: CGPoint(x: 0.21, y: 0.60), control1: CGPoint(x: 0.12, y: 0.90), control2: CGPoint(x: 0.21, y: 0.76))
        // One bold notch in the trailing edge.
        vane.addLine(to: CGPoint(x: 0.05, y: 0.50))
        vane.addLine(to: CGPoint(x: 0.19, y: 0.44))
        vane.addCurve(to: CGPoint(x: 0.0, y: 0.18), control1: CGPoint(x: 0.16, y: 0.32), control2: CGPoint(x: 0.08, y: 0.22))
        vane.closeSubpath()

        ctx.setFillColor(NSColor.black.cgColor)
        ctx.addPath(vane)
        ctx.fillPath()

        // Shaft as a clear line through the vane.
        ctx.setBlendMode(.clear)
        ctx.setLineCap(.round)
        ctx.setLineWidth(0.07)
        ctx.move(to: CGPoint(x: 0.0, y: 0.22))
        ctx.addLine(to: CGPoint(x: 0.0, y: 0.84))
        ctx.strokePath()
        ctx.setBlendMode(.normal)

        // Quill, overlapping the shaft's start so the two join.
        ctx.setStrokeColor(NSColor.black.cgColor)
        ctx.setLineWidth(0.1)
        ctx.move(to: CGPoint(x: 0.0, y: 0.03))
        ctx.addLine(to: CGPoint(x: 0.0, y: 0.25))
        ctx.strokePath()
    }
}
