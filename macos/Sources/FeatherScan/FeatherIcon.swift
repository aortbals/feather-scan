import AppKit
import QuartzCore

/// The menu bar feather, drawn in code as a template image so it follows the
/// menu bar's light/dark appearance.
enum FeatherIcon {
    static let size = NSSize(width: 18, height: 18)

    /// Resting feather. `dimmed` is used while offline.
    static func still(dimmed: Bool = false) -> NSImage {
        dimmed ? stillDimmed : stillNormal
    }

    private static let stillNormal = image { _ in 1 }
    private static let stillDimmed = image { _ in 0.4 }

    /// Renders the feather once into 1x and 2x bitmaps, which are cheap to show
    /// (a drawing-handler image would re-render every time the menu bar draws it).
    /// `opacity(y)` sets the opacity along the feather (y = 0 at the quill, 1 at the tip).
    private static func image(opacity: (Double) -> Double) -> NSImage {
        let img = NSImage(size: size)
        for scale in [1, 2] {
            let px = Int(size.width) * scale
            let rep = NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8, samplesPerPixel: 4,
                hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
            rep.size = size
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
            let ctx = NSGraphicsContext.current!.cgContext
            ctx.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
            ctx.translateBy(x: size.width / 2, y: size.height / 2)
            // The quill leans to the right, like a pen.
            ctx.rotate(by: tilt)
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
            NSGraphicsContext.restoreGraphicsState()
            img.addRepresentation(rep)
        }
        img.isTemplate = true
        img.accessibilityDescription = "Feather Scan"
        return img
    }

    // MARK: Shape (shared with scripts/make-icon.swift, which draws the app icon)

    /// The feather leans like a pen: rotation applied to the unit shape.
    static let tilt = -38 * Double.pi / 180

    /// Vane outline in a unit box: base of the quill at y = 0, tip at y = 1.
    /// Kept bold and simple so it reads at 18 pt: a solid vane with one notch.
    static let vanePath: CGPath = {
        let vane = CGMutablePath()
        vane.move(to: CGPoint(x: 0.0, y: 0.18))
        vane.addCurve(to: CGPoint(x: -0.23, y: 0.55), control1: CGPoint(x: -0.18, y: 0.24), control2: CGPoint(x: -0.24, y: 0.40))
        vane.addCurve(to: CGPoint(x: 0.0, y: 1.0), control1: CGPoint(x: -0.22, y: 0.74), control2: CGPoint(x: -0.10, y: 0.90))
        vane.addCurve(to: CGPoint(x: 0.21, y: 0.60), control1: CGPoint(x: 0.12, y: 0.90), control2: CGPoint(x: 0.21, y: 0.76))
        // One bold notch in the trailing edge.
        vane.addLine(to: CGPoint(x: 0.085, y: 0.52))
        vane.addLine(to: CGPoint(x: 0.20, y: 0.44))
        vane.addCurve(to: CGPoint(x: 0.0, y: 0.18), control1: CGPoint(x: 0.17, y: 0.32), control2: CGPoint(x: 0.08, y: 0.22))
        vane.closeSubpath()
        return vane
    }()

    /// Shaft through the vane: from, to, width (round caps). Its ends sit the
    /// same distance in from the tip and from the vane's base.
    static let shaft = (from: CGPoint(x: 0.0, y: 0.34), to: CGPoint(x: 0.0, y: 0.84), width: 0.07)
    /// Quill below the vane; the shaft cut-out sits inside its top end.
    static let quill = (from: CGPoint(x: 0.0, y: 0.03), to: CGPoint(x: 0.0, y: 0.25), width: 0.1)

    static func drawFeather(in ctx: CGContext) {
        ctx.setFillColor(NSColor.black.cgColor)
        ctx.addPath(vanePath)
        ctx.fillPath()

        ctx.setLineCap(.round)
        ctx.setStrokeColor(NSColor.black.cgColor)
        ctx.setLineWidth(quill.width)
        ctx.move(to: quill.from)
        ctx.addLine(to: quill.to)
        ctx.strokePath()

        // Shaft: a capsule cut out last, so it's rounded at both ends.
        ctx.setBlendMode(.clear)
        ctx.setLineWidth(shaft.width)
        ctx.move(to: shaft.from)
        ctx.addLine(to: shaft.to)
        ctx.strokePath()
        ctx.setBlendMode(.normal)
    }
}

/// The scanning shimmer: a gradient mask on the menu bar item's button that dims
/// the feather and sweeps a band of full opacity from the quill to the tip.
/// Core Animation renders it outside the app, so it doesn't wake the app per frame.
///
/// SwiftUI's MenuBarExtra has no API for its NSStatusItem, so the button is found
/// in the app's status bar window (the approach MenuBarExtraAccess uses).
@MainActor
enum MenuBarShimmer {
    private static let key = "featherShimmer"

    static func start() {
        guard let layer = statusButton()?.layer, layer.mask?.animation(forKey: key) == nil else { return }
        let mask = CAGradientLayer()
        mask.frame = layer.bounds
        let dim = NSColor(white: 0, alpha: 0.45).cgColor
        mask.colors = [dim, NSColor.black.cgColor, dim]
        // Quill at the bottom left, tip at the top right.
        mask.startPoint = CGPoint(x: 0, y: 0)
        mask.endPoint = CGPoint(x: 1, y: 1)
        mask.locations = [-0.25, -0.05, 0.15]
        let sweep = CABasicAnimation(keyPath: "locations")
        sweep.fromValue = [-0.25, -0.05, 0.15]
        sweep.toValue = [0.85, 1.05, 1.25]
        sweep.duration = 1.3
        sweep.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        let cycle = CAAnimationGroup()
        cycle.animations = [sweep]
        cycle.duration = 1.9  // 1.3 s sweep, 0.6 s rest
        cycle.repeatCount = .infinity
        mask.add(cycle, forKey: key)
        layer.mask = mask
    }

    static func stop() {
        statusButton()?.layer?.mask = nil
    }

    private static func statusButton() -> NSButton? {
        for window in NSApp.windows where String(describing: type(of: window)).contains("NSStatusBarWindow") {
            if let button = findButton(in: window.contentView) {
                button.wantsLayer = true
                return button
            }
        }
        return nil
    }

    private static func findButton(in view: NSView?) -> NSButton? {
        guard let view else { return nil }
        if let b = view as? NSButton { return b }
        for sub in view.subviews {
            if let b = findButton(in: sub) { return b }
        }
        return nil
    }
}
