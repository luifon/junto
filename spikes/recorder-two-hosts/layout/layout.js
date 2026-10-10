'use strict';

// Recording layout page for LiveKit Egress.
// Egress opens it as <base>?layout=<layout>&url=<ws url>&token=<token>.
//
// Picture: the main screen share fills the stage; cameras and other shares are tiles.
// Sound: every microphone track, plus the audio track of the main share only.
// The page subscribes only to the tracks it shows or plays. It never subscribes to
// the audio of a share that is not the main one: in Egress's Chrome, detaching an
// audio element did not stop the sound of a track that had been played.
//
// The main share is the one the controller names, or the most recently started one.
// The controller names it (by track id or by participant identity) through any of:
//   - the layout string:      main:<id>          (start request, then UpdateLayout)
//   - the room metadata:      {"main_share":"<id>"}
//   - a data message, topic "layout": {"main_share":"<id>"}
// An empty id returns to the automatic choice. Add ";debug" to the layout string to
// draw a status line on the recording.
//
// Flags of the layout string used only by the two-host spike:
//   lowtiles   ask the server for the lowest simulcast layer of every tile
//   novideo    hide the stage (a recording without a picture, for the result check)
//   nosound    attach no audio track (a recording without sound, for the result check)

const { Room, RoomEvent, Track, VideoQuality } = LivekitClient;
const query = new URLSearchParams(location.search);
const room = new Room({ adaptiveStream: false, dynacast: false });

const mainElement = document.getElementById('main');
const tilesElement = document.getElementById('tiles');
const soundsElement = document.getElementById('sounds');
const statusElement = document.getElementById('status');

let requestedMain = '';
let requestedBy = 'auto';
let debug = false;
let lowTiles = false;
let noVideo = false;
let noSound = false;
const firstSeen = new Map();
let recordingStarted = false;

function applyLayoutString(text, origin) {
  const parts = (text || '').split(';').map((p) => p.trim());
  debug = parts.includes('debug');
  lowTiles = parts.includes('lowtiles');
  noVideo = parts.includes('novideo');
  noSound = parts.includes('nosound');
  const main = parts.find((p) => p.startsWith('main:'));
  if (main !== undefined) setRequestedMain(main.slice(5), origin);
}

function applyJson(text, origin) {
  let value;
  try { value = JSON.parse(text); } catch { return; }
  if (value && typeof value.main_share === 'string') setRequestedMain(value.main_share, origin);
  if (value && typeof value.layout === 'string') applyLayoutString(value.layout, origin);
}

function setRequestedMain(id, origin) {
  requestedMain = id;
  requestedBy = id ? origin : 'auto';
}

function publications(source) {
  const found = [];
  for (const participant of room.remoteParticipants.values()) {
    for (const publication of participant.trackPublications.values()) {
      if (publication.source === source && publication.track) {
        found.push({ participant, publication });
      }
    }
  }
  return found;
}

// Start order of a share: the time in the track's server version stamp when the
// SDK exposes it, otherwise the time at which this page first saw the track.
function startedAt(publication) {
  if (!firstSeen.has(publication.trackSid)) {
    const version = publication.trackInfo && publication.trackInfo.version;
    const stamped = version ? Number(version.unixMicro) : 0;
    firstSeen.set(publication.trackSid, stamped > 0 ? stamped : Date.now() * 1000);
  }
  return firstSeen.get(publication.trackSid);
}

function chooseMain(shares) {
  const requested = shares.find((s) =>
    s.publication.trackSid === requestedMain || s.participant.identity === requestedMain);
  if (requested) return requested;
  return shares.reduce((latest, s) =>
    (!latest || startedAt(s.publication) > startedAt(latest.publication)) ? s : latest, null);
}

function videoBox(container, key, entry, label) {
  let box = container.querySelector(`[data-key="${key}"]`);
  if (!box) {
    box = document.createElement('div');
    box.dataset.key = key;
    if (container === tilesElement) box.className = 'tile';
    else box.style.cssText = 'position:absolute;inset:0';
    const name = document.createElement('div');
    name.className = 'name';
    box.media = entry.publication.track.attach();
    box.track = entry.publication.track;
    box.append(box.media, name);
    container.append(box);
  }
  box.querySelector('.name').textContent = label;
  return box;
}

function removeUnless(container, keep) {
  for (const box of [...container.querySelectorAll('[data-key]')]) {
    if (!keep.has(box.dataset.key)) {
      box.track.detach(box.media);
      box.remove();
    }
  }
}

function subscribeTo(wanted) {
  for (const participant of room.remoteParticipants.values()) {
    for (const publication of participant.trackPublications.values()) {
      const desired = wanted(participant, publication);
      if (publication.isDesired !== desired) publication.setSubscribed(desired);
    }
  }
}

function render() {
  const always = [Track.Source.ScreenShare, Track.Source.Camera, Track.Source.Microphone];
  const mainIdentity = () => {
    const current = chooseMain(publications(Track.Source.ScreenShare));
    return current ? current.participant.identity : null;
  };
  subscribeTo((participant, publication) => always.includes(publication.source)
    || (publication.source === Track.Source.ScreenShareAudio && participant.identity === mainIdentity()));

  const shares = publications(Track.Source.ScreenShare);
  shares.forEach((s) => startedAt(s.publication));
  const cameras = publications(Track.Source.Camera);
  const main = chooseMain(shares);

  const keepMain = new Set();
  const keepTiles = new Set();
  if (main) {
    const key = main.publication.trackSid;
    keepMain.add(key);
    videoBox(mainElement, key, main, `${main.participant.identity} (screen)`);
    main.publication.setVideoQuality(VideoQuality.HIGH);
  }
  const tileQuality = lowTiles ? VideoQuality.LOW : VideoQuality.HIGH;
  for (const camera of cameras) {
    keepTiles.add(camera.publication.trackSid);
    videoBox(tilesElement, camera.publication.trackSid, camera, camera.participant.identity);
    camera.publication.setVideoQuality(tileQuality);
  }
  for (const share of shares) {
    if (share === main) continue;
    keepTiles.add(share.publication.trackSid);
    videoBox(tilesElement, share.publication.trackSid, share, `${share.participant.identity} (screen)`);
    share.publication.setVideoQuality(tileQuality);
  }
  document.getElementById('stage').style.visibility = noVideo ? 'hidden' : 'visible';
  removeUnless(mainElement, keepMain);
  removeUnless(tilesElement, keepTiles);
  document.getElementById('empty').style.display = main ? 'none' : 'flex';

  const audible = noSound ? [] : publications(Track.Source.Microphone);
  if (main && !noSound) {
    audible.push(...publications(Track.Source.ScreenShareAudio)
      .filter((a) => a.participant.identity === main.participant.identity));
  }
  const keepSounds = new Set(audible.map((a) => a.publication.trackSid));
  for (const entry of audible) {
    const key = entry.publication.trackSid;
    if (!soundsElement.querySelector(`[data-key="${key}"]`)) {
      const box = document.createElement('div');
      box.dataset.key = key;
      box.media = entry.publication.track.attach();
      box.track = entry.publication.track;
      box.append(box.media);
      soundsElement.append(box);
    }
  }
  removeUnless(soundsElement, keepSounds);

  statusElement.style.display = debug ? 'block' : 'none';
  const requestedIsMain = main && requestedMain
    && [main.publication.trackSid, main.participant.identity].includes(requestedMain);
  statusElement.textContent = `main=${main ? main.participant.identity : '-'}`
    + ` by=${requestedIsMain ? requestedBy : 'auto'}`
    + ` shares=${shares.length} cameras=${cameras.length}`
    + ` sounds=${audible.map((a) => `${a.participant.identity}:${a.publication.source}`).join(',')}`;
  if (debug) {
    const details = [];
    for (const participant of room.remoteParticipants.values()) {
      for (const publication of participant.audioTrackPublications.values()) {
        const elements = publication.track ? publication.track.attachedElements : [];
        details.push(`${participant.identity}:${publication.source}:sub=${publication.isSubscribed}`
          + `:attached=${elements.length}:playing=${elements.filter((e) => !e.paused && e.srcObject).length}`);
      }
    }
    statusElement.textContent += `\naudio elements=${document.querySelectorAll('audio').length} ${details.join(' ')}`;
  }
}

function startRecordingOnce() {
  if (recordingStarted) return;
  recordingStarted = true;
  console.log('START_RECORDING');
}

async function run() {
  applyLayoutString(query.get('layout'), 'layout');

  const redraw = [
    RoomEvent.TrackPublished, RoomEvent.TrackSubscribed, RoomEvent.TrackUnsubscribed,
    RoomEvent.TrackUnpublished, RoomEvent.ParticipantConnected,
    RoomEvent.ParticipantDisconnected, RoomEvent.TrackMuted, RoomEvent.TrackUnmuted,
  ];
  redraw.forEach((event) => room.on(event, render));
  room.on(RoomEvent.RoomMetadataChanged, (metadata) => {
    applyJson(metadata, 'room-metadata');
    render();
  });
  room.on(RoomEvent.ParticipantMetadataChanged, (_previous, participant) => {
    if (participant === room.localParticipant) {
      applyJson(participant.metadata, 'update-layout');
      render();
    }
  });
  room.on(RoomEvent.DataReceived, (payload, _participant, _kind, topic) => {
    if (topic === 'layout') {
      applyJson(new TextDecoder().decode(payload), 'data-message');
      render();
    }
  });
  room.on(RoomEvent.Disconnected, () => console.log('END_RECORDING'));

  await room.connect(query.get('url'), query.get('token'), { autoSubscribe: false });
  await room.startAudio().catch(() => {});
  if (room.metadata) applyJson(room.metadata, 'room-metadata');
  render();
  setTimeout(startRecordingOnce, 1500);
}

run().catch((error) => console.error('layout page failed', error));
