import ApplicationServices
import Foundation

// Prints the macOS accessibility tree of one process: role, subrole, title, description, value, actions.
// usage: ax_dump <pid> [max depth]
guard CommandLine.arguments.count >= 2, let pid = Int32(CommandLine.arguments[1]) else { exit(2) }
let maxDepth = CommandLine.arguments.count > 2 ? Int(CommandLine.arguments[2]) ?? 12 : 12

func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func text(_ value: AnyObject?) -> String? {
    guard let value else { return nil }
    if let string = value as? String { return string.isEmpty ? nil : string }
    if let number = value as? NSNumber { return number.stringValue }
    return nil
}

func dump(_ element: AXUIElement, _ depth: Int) {
    var parts: [String] = [text(attribute(element, kAXRoleAttribute)) ?? "?"]
    if let subrole = text(attribute(element, kAXSubroleAttribute)) { parts.append("subrole=\(subrole)") }
    if let title = text(attribute(element, kAXTitleAttribute)) { parts.append("title=\"\(title)\"") }
    if let description = text(attribute(element, kAXDescriptionAttribute)) { parts.append("description=\"\(description)\"") }
    if let value = text(attribute(element, kAXValueAttribute)) { parts.append("value=\"\(value.prefix(80))\"") }
    if let identifier = text(attribute(element, kAXIdentifierAttribute)) { parts.append("identifier=\"\(identifier)\"") }
    if let focused = attribute(element, kAXFocusedAttribute) as? Bool, focused { parts.append("focused") }
    var actions: CFArray?
    if AXUIElementCopyActionNames(element, &actions) == .success, let names = actions as? [String], !names.isEmpty {
        parts.append("actions=\(names.joined(separator: ","))")
    }
    print(String(repeating: "  ", count: depth) + parts.joined(separator: " "))
    guard depth < maxDepth, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] else { return }
    for child in children { dump(child, depth + 1) }
}

let app = AXUIElementCreateApplication(pid)
guard let windows = attribute(app, kAXWindowsAttribute) as? [AXUIElement] else {
    print("no windows (is accessibility access granted to this process?)")
    exit(1)
}
for window in windows { dump(window, 0) }
