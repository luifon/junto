import AppKit
import CoreGraphics
import Darwin

func line(_ fd: Int32) -> String? {
    var bytes = [UInt8]()
    var byte: UInt8 = 0
    while bytes.count < 4096 {
        guard Darwin.read(fd, &byte, 1) == 1 else { return nil }
        if byte == 10 { return String(bytes: bytes, encoding: .utf8) }
        bytes.append(byte)
    }
    return nil
}
func reply(_ fd: Int32, _ value: String) {
    let bytes = Array((value + "\n").utf8)
    bytes.withUnsafeBytes { raw in
        var sent = 0
        while sent < raw.count {
            let count = Darwin.write(fd, raw.baseAddress!.advanced(by: sent), raw.count - sent)
            if count <= 0 { return }
            sent += count
        }
    }
}

final class Pattern: NSView {
    static let colors: [NSColor] = [
        NSColor(srgbRed: 0.8, green: 0.1, blue: 0.1, alpha: 1),
        NSColor(srgbRed: 0.1, green: 0.8, blue: 0.1, alpha: 1),
        NSColor(srgbRed: 0.1, green: 0.1, blue: 0.8, alpha: 1),
        NSColor(srgbRed: 0.8, green: 0.8, blue: 0.1, alpha: 1)
    ]
    // A display whose gamut lacks a colour clips it, and capture then returns the clipped colour.
    static func shown(on screen: NSScreen) -> Bool {
        guard let space = screen.colorSpace else { return false }
        return colors.allSatisfy { color in
            guard let source = color.usingColorSpace(.sRGB),
                  let shown = color.usingColorSpace(space)?.usingColorSpace(.sRGB) else { return false }
            return [(source.redComponent, shown.redComponent), (source.greenComponent, shown.greenComponent),
                    (source.blueComponent, shown.blueComponent)].allSatisfy { abs($0.0 - $0.1) * 255 < 2 }
        }
    }
    var marker: UInt32 = 0
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        for index in 0..<4 {
            Pattern.colors[index].setFill()
            NSRect(x: CGFloat(index % 2) * bounds.width / 2, y: bounds.height * 0.1 + CGFloat(index / 2) * bounds.height * 0.45, width: bounds.width / 2, height: bounds.height * 0.45).fill()
        }
        for bit in 0..<32 {
            ((marker >> bit) & 1 == 1 ? NSColor.white : NSColor.black).setFill()
            NSRect(x: CGFloat(bit) * bounds.width / 32, y: 0, width: bounds.width / 32, height: bounds.height * 0.1).fill()
        }
    }
}

final class Fixture: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var pattern: Pattern!
    var player: Process?
    var playerInput: FileHandle?
    var playerOutput: FileHandle?
    let socketPath: String
    let helper: String
    let title: String
    let admission: String
    init(socketPath: String, helper: String, title: String, admission: String) {
        self.socketPath = socketPath; self.helper = helper; self.title = title; self.admission = admission
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        // The first screen is the one with the menu bar; another is used only when it clips a colour.
        guard let screen = NSScreen.screens.first(where: Pattern.shown(on:)) else {
            DispatchQueue.global().async { self.serve("error no display shows the fixture colours") }
            return
        }
        let visible = screen.visibleFrame
        let width = min(800, visible.width - 80)
        let height = min(600, visible.height - 80)
        guard width >= 320, height >= 240 else { exit(2) }
        let frame = NSRect(x: visible.minX + 40, y: visible.maxY - height - 40, width: width, height: height)
        window = NSWindow(contentRect: frame, styleMask: .borderless, backing: .buffered, defer: false)
        window.title = title
        window.level = .normal
        window.isReleasedWhenClosed = false
        pattern = Pattern(frame: NSRect(x: 0, y: 0, width: width, height: height))
        window.contentView = pattern
        window.orderFrontRegardless()
        window.displayIfNeeded()
        let display = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as! NSNumber).uint32Value
        let scale = screen.backingScaleFactor
        let x = Int((frame.minX - screen.frame.minX) * scale)
        let y = Int((screen.frame.maxY - frame.maxY) * scale)
        guard let mode = CGDisplayCopyDisplayMode(display) else { exit(2) }
        let geometry = "ready \(window.windowNumber) \(display) \(x) \(y) \(Int(frame.width * scale)) \(Int(frame.height * scale)) \(mode.pixelWidth) \(mode.pixelHeight)"
        DispatchQueue.global().async { self.serve(geometry) }
    }
    func tone(_ hz: UInt32) -> String {
        if player == nil {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: helper)
            process.arguments = ["fixture-audio", admission]
            var environment = ProcessInfo.processInfo.environment
            environment["JUNTO_TEST_NATIVE_DEVICES"] = "1"
            process.environment = environment
            let input = Pipe(), output = Pipe()
            process.standardInput = input
            process.standardOutput = output
            process.standardError = FileHandle.standardError
            do { try process.run() } catch { return "error player launch" }
            player = process
            playerInput = input.fileHandleForWriting
            playerOutput = output.fileHandleForReading
            guard line(playerOutput!.fileDescriptor) == "ready coreaudio:BlackHole2ch_UID" else { return "error player route" }
        }
        reply(playerInput!.fileDescriptor, "tone \(hz)")
        return line(playerOutput!.fileDescriptor) ?? "error player stopped"
    }
    func serve(_ geometry: String) {
        let client = socket(AF_UNIX, SOCK_STREAM, 0)
        guard client >= 0 else { exit(2) }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let path = Array(socketPath.utf8) + [0]
        guard path.count <= MemoryLayout.size(ofValue: address.sun_path) else { exit(2) }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in raw.copyBytes(from: path) }
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(client, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0 else { exit(2) }
        reply(client, geometry)
        while geometry.hasPrefix("ready "), let command = line(client) {
            let parts = command.split(separator: " ")
            if parts.count == 2, parts[0] == "pattern", let marker = UInt32(parts[1]) {
                DispatchQueue.main.sync {
                    self.pattern.marker = marker
                    self.pattern.needsDisplay = true
                    self.pattern.displayIfNeeded()
                }
                reply(client, "pattern \(marker)")
            } else if parts.count == 2, parts[0] == "tone", let hz = UInt32(parts[1]), hz <= 4000 {
                reply(client, tone(hz))
            } else if command == "close" { reply(client, "closed"); break }
            else { reply(client, "error command") }
        }
        playerInput?.closeFile()
        player?.waitUntilExit()
        close(client)
        DispatchQueue.main.async { NSApplication.shared.terminate(nil) }
    }
}

signal(SIGPIPE, SIG_IGN)
let args = CommandLine.arguments
if args.count != 5 { exit(2) }
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let fixture = Fixture(socketPath: args[1], helper: args[2], title: args[3], admission: args[4])
app.delegate = fixture
app.run()
