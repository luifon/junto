import AppKit

// Control for the file drag test: a window that accepts file drags and logs each stage.
// usage: drag_dest <x> <y> <seconds to live>
final class DestView: NSView {
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        FileHandle.standardError.write("drag dest: entered\n".data(using: .utf8)!)
        return .copy
    }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let names = sender.draggingPasteboard.propertyList(forType: NSPasteboard.PasteboardType("NSFilenamesPboardType")) as? [String] ?? []
        FileHandle.standardError.write("drag dest: dropped \(names.count) files\n".data(using: .utf8)!)
        return true
    }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.systemTeal.setFill()
        dirtyRect.fill()
    }
}

let arguments = CommandLine.arguments
guard arguments.count >= 4, let x = Double(arguments[1]), let y = Double(arguments[2]), let life = Double(arguments[3]) else { exit(2) }
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screenHeight = NSScreen.main?.frame.height ?? 982
let window = NSWindow(contentRect: NSRect(x: x, y: screenHeight - y - 120, width: 200, height: 120), styleMask: [.borderless], backing: .buffered, defer: false)
let view = DestView(frame: NSRect(x: 0, y: 0, width: 200, height: 120))
view.registerForDraggedTypes([.fileURL])
window.contentView = view
window.level = .floating
window.orderFrontRegardless()
DispatchQueue.main.asyncAfter(deadline: .now() + life) { exit(0) }
app.run()
