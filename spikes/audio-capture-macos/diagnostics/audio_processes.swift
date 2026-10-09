import CoreAudio
import Foundation
func u32(_ id: AudioObjectID, _ sel: AudioObjectPropertySelector) -> UInt32? {
    var a = AudioObjectPropertyAddress(mSelector: sel, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var v: UInt32 = 0; var s = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(id, &a, 0, nil, &s, &v) == noErr ? v : nil
}
func str(_ id: AudioObjectID, _ sel: AudioObjectPropertySelector) -> String {
    var a = AudioObjectPropertyAddress(mSelector: sel, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var v: Unmanaged<CFString>?; var s = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(id, &a, 0, nil, &s, &v) == noErr, let c = v else { return "" }
    return c.takeRetainedValue() as String
}
var a = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
var size: UInt32 = 0
AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size)
var ids = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &ids)
for id in ids {
    let pid = u32(id, kAudioProcessPropertyPID).map { Int32(bitPattern: $0) } ?? -1
    let out = u32(id, kAudioProcessPropertyIsRunningOutput) ?? 0
    let inp = u32(id, kAudioProcessPropertyIsRunningInput) ?? 0
    print("object=\(id) pid=\(pid) running_output=\(out) running_input=\(inp) bundle=\(str(id, kAudioProcessPropertyBundleID))")
}
