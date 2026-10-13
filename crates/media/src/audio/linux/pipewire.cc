// The SDK's PipeWire trampolines also resolve calls made by the sound host.
// This declaration matches webrtc-sys 0.3.48 in Cargo.lock for livekit 0.9.4.
// Recheck its signature when updating either dependency.
namespace webrtc {
bool InitializePipeWire();
}

extern "C" bool junto_initialize_pipewire() {
    return webrtc::InitializePipeWire();
}
