import CoreGraphics
import Foundation

// usage: hold_key <pid> <keycode> <hold seconds> [keycode to press while the accent menu is open]
let arguments = CommandLine.arguments
guard arguments.count >= 4, let pid = Int32(arguments[1]), let key = UInt16(arguments[2]),
      let hold = Double(arguments[3]) else {
    print("usage: hold_key <pid> <keycode> <hold seconds> [choice keycode]")
    exit(2)
}
let choice = arguments.count > 4 ? UInt16(arguments[4]) : nil
let source = CGEventSource(stateID: .hidSystemState)

func post(_ keycode: UInt16, down: Bool, isRepeat: Bool = false) {
    guard let event = CGEvent(keyboardEventSource: source, virtualKey: keycode, keyDown: down) else { return }
    if isRepeat {
        event.setIntegerValueField(.keyboardEventAutorepeat, value: 1)
    }
    event.postToPid(pid)
}

post(key, down: true)
Thread.sleep(forTimeInterval: 0.5)
let deadline = Date().addingTimeInterval(hold)
while Date() < deadline {
    post(key, down: true, isRepeat: true)
    Thread.sleep(forTimeInterval: 0.083)
}
if let choice {
    post(key, down: false)
    Thread.sleep(forTimeInterval: 0.2)
    post(choice, down: true)
    Thread.sleep(forTimeInterval: 0.05)
    post(choice, down: false)
} else {
    post(key, down: false)
}
print("done")
