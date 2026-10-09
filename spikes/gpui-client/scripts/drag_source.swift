import AppKit

// A small window that starts a file drag session (file URLs on the dragging pasteboard,
// the same payload Finder provides) as soon as the mouse is pressed and moved inside it.
// usage: drag_source <x> <y> <seconds to live> <file> [file...]
final class SourceView: NSView, NSDraggingSource {
    var files: [URL] = []

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        FileHandle.standardError.write("drag source: mouse down\n".data(using: .utf8)!)
    }

    override func mouseDragged(with event: NSEvent) {
        let items = files.enumerated().map { index, url -> NSDraggingItem in
            let item = NSDraggingItem(pasteboardWriter: url as NSURL)
            item.setDraggingFrame(NSRect(x: 10, y: 10 + CGFloat(index) * 20, width: 120, height: 18), contents: NSImage(size: NSSize(width: 120, height: 18)))
            return item
        }
        FileHandle.standardError.write("drag source: session started\n".data(using: .utf8)!)
        beginDraggingSession(with: items, event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        FileHandle.standardError.write("drag source: session ended, operation \(operation.rawValue)\n".data(using: .utf8)!)
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.systemOrange.setFill()
        dirtyRect.fill()
    }
}

let arguments = CommandLine.arguments
guard arguments.count >= 5, let x = Double(arguments[1]), let y = Double(arguments[2]), let life = Double(arguments[3]) else {
    print("usage: drag_source <x> <y> <seconds> <file>...")
    exit(2)
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screenHeight = NSScreen.main?.frame.height ?? 982
let window = NSWindow(contentRect: NSRect(x: x, y: screenHeight - y - 80, width: 160, height: 80), styleMask: [.borderless], backing: .buffered, defer: false)
let view = SourceView(frame: NSRect(x: 0, y: 0, width: 160, height: 80))
view.files = arguments.dropFirst(4).map { URL(fileURLWithPath: $0) }
window.contentView = view
window.level = .floating
window.orderFrontRegardless()
DispatchQueue.main.asyncAfter(deadline: .now() + life) { exit(0) }
app.run()
