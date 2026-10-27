const { Room, Track, VideoPreset } = LivekitClient;
window.publisher = new Room({ adaptiveStream: false, dynacast: false });
const context = new AudioContext();
const resources = new Map();

window.join = async (url, token) => {
  await publisher.connect(url, token, { autoSubscribe: false });
  await context.resume();
};
window.publish = async (source, frequency = 440, name = undefined) => {
  let track;
  let cleanup;
  if ([Track.Source.Camera, Track.Source.ScreenShare, Track.Source.Unknown].includes(source)) {
    const canvas = document.createElement('canvas');
    canvas.width = 1280;
    canvas.height = 720;
    const ctx = canvas.getContext('2d');
    let tick = 0;
    const draw = () => {
      ctx.fillStyle = source === Track.Source.ScreenShare ? '#367fbd' : '#527954';
      ctx.fillRect(0, 0, 1280, 720);
      ctx.fillStyle = '#e3d7c1';
      ctx.fillRect((tick++ * 5) % 600, 30, 40, 300);
      ctx.font = '32px sans-serif';
      ctx.fillText(`${publisher.localParticipant.identity} ${source}`, 20, 180);
    };
    draw();
    const timer = setInterval(draw, 100);
    track = canvas.captureStream(10).getVideoTracks()[0];
    cleanup = () => { clearInterval(timer); track.stop(); };
  } else {
    const oscillator = context.createOscillator();
    oscillator.frequency.value = frequency;
    const gain = context.createGain();
    gain.gain.value = 0.2;
    const sink = context.createMediaStreamDestination();
    oscillator.connect(gain).connect(sink);
    oscillator.start();
    track = sink.stream.getAudioTracks()[0];
    cleanup = () => { oscillator.stop(); oscillator.disconnect(); gain.disconnect(); track.stop(); };
  }
  const layers = [new VideoPreset(320, 180, 100_000, 10), new VideoPreset(640, 360, 250_000, 10)];
  const publication = await publisher.localParticipant.publishTrack(track, {
    source, name: name ?? `${source}-${resources.size}`, videoCodec: 'vp8', simulcast: true,
    videoEncoding: { maxBitrate: 800_000, maxFramerate: 10 },
    videoSimulcastLayers: layers, screenShareSimulcastLayers: layers,
    dtx: false, red: false, audioPreset: { maxBitrate: 64_000 },
  });
  resources.set(publication.trackSid, { publication, cleanup });
  return publication.trackSid;
};
window.unpublish = async (sid) => {
  const resource = resources.get(sid);
  await publisher.localParticipant.unpublishTrack(resource.publication.track);
  resource.cleanup();
  resources.delete(sid);
};
window.mute = async (sid, muted) => {
  const track = resources.get(sid).publication.track;
  await (muted ? track.mute() : track.unmute());
};

window.stall = async (sid) => {
  await resources.get(sid).publication.track.sender.replaceTrack(null);
};
