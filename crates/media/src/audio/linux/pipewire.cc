// The SDK's PipeWire trampolines also resolve calls made by the sound host.
namespace webrtc {
bool InitializePipeWire();
}

extern "C" bool junto_initialize_pipewire() {
    return webrtc::InitializePipeWire();
}
