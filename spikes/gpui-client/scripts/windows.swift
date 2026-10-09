import CoreGraphics
import Foundation

// Lists the on-screen windows of one process with their window level (layer).
// usage: windows <pid>
guard CommandLine.arguments.count >= 2, let pid = Int(CommandLine.arguments[1]) else { exit(2) }
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for window in list where (window[kCGWindowOwnerPID as String] as? Int) == pid {
    let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
    print("id=\(window[kCGWindowNumber as String] ?? 0) layer=\(window[kCGWindowLayer as String] ?? 0) name=\(window[kCGWindowName as String] ?? "") bounds=\(bounds["X"] ?? 0),\(bounds["Y"] ?? 0) \(bounds["Width"] ?? 0)x\(bounds["Height"] ?? 0)")
}
