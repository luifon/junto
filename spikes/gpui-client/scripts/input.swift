import CoreGraphics
import Foundation

// Posts synthetic pointer events for the spike tests. Coordinates are window-local points.
// usage: input <pid> <window number> <origin x> <origin y> <command> <values...> [--session-state]
//   move <x> <y>                                  posted to the process
//   rightclick <x> <y>                            posted to the process
//   scroll <x> <y> <delta y per event> <events>   posted to the process (gpui did not receive these)
//   drag <x1> <y1> <x2> <y2> <steps> [pause]      posted to the process (gpui did not start a drag from these)
//   gscroll <x> <y> <delta y per event> <events>  posted to the HID tap; the real pointer moves and returns
//   gdrag <x1> <y1> <x2> <y2> <steps> [pause]     posted to the HID tap; the real pointer moves and returns
let arguments = CommandLine.arguments
guard arguments.count >= 6, let pid = Int32(arguments[1]), let windowNumber = Int64(arguments[2]),
      let originX = Double(arguments[3]), let originY = Double(arguments[4]) else {
    print("bad arguments")
    exit(2)
}
let command = arguments[5]
let values = arguments.dropFirst(6).compactMap { Double($0) }

func route(_ event: CGEvent) {
    event.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: windowNumber)
    event.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: windowNumber)
    event.postToPid(pid)
}
let source = CGEventSource(stateID: CommandLine.arguments.contains("--session-state") ? .combinedSessionState : .hidSystemState)

func point(_ x: Double, _ y: Double) -> CGPoint {
    CGPoint(x: originX + x, y: originY + y)
}

func mouse(_ type: CGEventType, _ location: CGPoint, _ button: CGMouseButton = .left, clicks: Int64 = 1) {
    guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: location, mouseButton: button) else { return }
    event.setIntegerValueField(.mouseEventClickState, value: clicks)
    route(event)
}

switch command {
case "scroll":
    guard values.count >= 4 else { exit(2) }
    let location = point(values[0], values[1])
    mouse(.mouseMoved, location)
    for _ in 0..<Int(values[3]) {
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 1, wheel1: Int32(values[2]), wheel2: 0, wheel3: 0) else { continue }
        event.location = location
        route(event)
        Thread.sleep(forTimeInterval: 0.008)
    }
case "gscroll":
    // Global scroll: the pointer is moved over the target, events go through the HID tap, the pointer is put back.
    guard values.count >= 4 else { exit(2) }
    let location = point(values[0], values[1])
    let saved = CGEvent(source: nil)?.location ?? location
    CGWarpMouseCursorPosition(location)
    Thread.sleep(forTimeInterval: 0.05)
    for _ in 0..<Int(values[3]) {
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 1, wheel1: Int32(values[2]), wheel2: 0, wheel3: 0) else { continue }
        event.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.008)
    }
    Thread.sleep(forTimeInterval: 0.05)
    CGWarpMouseCursorPosition(saved)
case "gdrag":
    // Global drag through the HID tap: the real pointer moves, then returns to where it was.
    guard values.count >= 5 else { exit(2) }
    let start = point(values[0], values[1])
    let end = point(values[2], values[3])
    let steps = max(Int(values[4]), 2)
    let pause = values.count > 5 ? values[5] : 0.2
    let saved = CGEvent(source: nil)?.location ?? start
    var previous = start
    func global(_ type: CGEventType, _ location: CGPoint) {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: location, mouseButton: .left) else { return }
        event.setIntegerValueField(.mouseEventClickState, value: 1)
        event.setIntegerValueField(.mouseEventDeltaX, value: Int64((location.x - previous.x).rounded()))
        event.setIntegerValueField(.mouseEventDeltaY, value: Int64((location.y - previous.y).rounded()))
        if type != .mouseMoved && type != .leftMouseUp {
            event.setDoubleValueField(.mouseEventPressure, value: 1.0)
        }
        previous = location
        event.post(tap: .cghidEventTap)
    }
    global(.mouseMoved, start)
    Thread.sleep(forTimeInterval: 0.08)
    global(.leftMouseDown, start)
    Thread.sleep(forTimeInterval: 0.3)
    for nudge in 1...6 {
        global(.leftMouseDragged, CGPoint(x: start.x - Double(nudge), y: start.y - Double(nudge)))
        Thread.sleep(forTimeInterval: 0.03)
    }
    for step in 1...steps {
        let fraction = Double(step) / Double(steps)
        global(.leftMouseDragged, CGPoint(x: start.x + (end.x - start.x) * fraction, y: start.y + (end.y - start.y) * fraction))
        Thread.sleep(forTimeInterval: 0.02)
    }
    Thread.sleep(forTimeInterval: pause)
    global(.leftMouseUp, end)
    Thread.sleep(forTimeInterval: 0.05)
    CGWarpMouseCursorPosition(saved)
case "drag":
    guard values.count >= 5 else { exit(2) }
    let start = point(values[0], values[1])
    let end = point(values[2], values[3])
    let steps = max(Int(values[4]), 2)
    let pause = values.count > 5 ? values[5] : 0.2
    mouse(.mouseMoved, start)
    Thread.sleep(forTimeInterval: 0.05)
    mouse(.leftMouseDown, start)
    Thread.sleep(forTimeInterval: 0.08)
    for step in 1...steps {
        let fraction = Double(step) / Double(steps)
        let location = CGPoint(x: start.x + (end.x - start.x) * fraction, y: start.y + (end.y - start.y) * fraction)
        mouse(.leftMouseDragged, location)
        Thread.sleep(forTimeInterval: 0.02)
    }
    Thread.sleep(forTimeInterval: pause)
    mouse(.leftMouseUp, end)
case "move":
    guard values.count >= 2 else { exit(2) }
    mouse(.mouseMoved, point(values[0], values[1]))
case "rightclick":
    guard values.count >= 2 else { exit(2) }
    let location = point(values[0], values[1])
    mouse(.mouseMoved, location)
    Thread.sleep(forTimeInterval: 0.05)
    mouse(.rightMouseDown, location, .right)
    Thread.sleep(forTimeInterval: 0.05)
    mouse(.rightMouseUp, location, .right)
default:
    print("unknown command")
    exit(2)
}
print("done")
