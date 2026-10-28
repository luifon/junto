import { messages } from './messages.js';

const { Room, RoomEvent, Track, VideoQuality, ConnectionState, DisconnectReason } = LivekitClient;
export const room = new Room({ adaptiveStream: false, dynacast: false });
const query = new URLSearchParams(location.search);
const mainElement = document.querySelector('#main');
const tilesElement = document.querySelector('#tiles');
const soundsElement = document.querySelector('#sounds');
const stage = document.querySelector('#stage');
const status = document.querySelector('#status');
const boxes = new Map();
const firstSeen = new Map();
let requestedMain = query.get('layout')?.match(/^main:(.*)$/)?.[1] ?? '';
let started = false;
let ended = false;
let ready = false;
let queued = false;
let wanted = [];
let startTimer;
let reconnectTimer;
let reconnectDelay = 1000;
let connecting = false;
let joinToken = query.get('token');

function readLayout() {
  try {
    const value = JSON.parse(room.localParticipant.metadata);
    if (typeof value?.layout === 'string' && /^(main:.*)?$/.test(value.layout)) {
      requestedMain = value.layout.slice(5);
    }
  } catch { /* Metadata unrelated to the layout leaves the current choice intact. */ }
}

function entries() {
  return [...room.remoteParticipants.values()].flatMap((participant) =>
    [...participant.trackPublications.values()].map((publication) => ({ participant, publication })));
}

function removeBox(sid) {
  const box = boxes.get(sid);
  if (!box) return;
  box.media.pause();
  box.track.detach(box.media);
  box.media.srcObject = null;
  box.element.remove();
  boxes.delete(sid);
}

function show(entry, container) {
  const { participant, publication } = entry;
  const track = publication.track;
  if (!track || !publication.isSubscribed) return;
  const sid = publication.trackSid;
  if (boxes.has(sid) && boxes.get(sid).track !== track) removeBox(sid);
  let box = boxes.get(sid);
  if (!box) {
    const element = document.createElement('div');
    element.dataset.trackSid = sid;
    element.dataset.source = publication.source;
    const media = track.attach();
    media.autoplay = true;
    element.append(media);
    if (track.kind === Track.Kind.Video) {
      element.className = 'tile';
      media.muted = true;
      media.playsInline = true;
      const label = document.createElement('span');
      label.className = 'label';
      element.append(label);
    }
    box = { element, media, track };
    boxes.set(sid, box);
    media.addEventListener('loadeddata', maybeStart);
    media.addEventListener('playing', maybeStart);
  }
  const label = box.element.querySelector('.label');
  if (label) {
    label.textContent = participant.name || '';
    label.hidden = !participant.name;
  }
  if (box.element.parentNode !== container) container.append(box.element);
}

function fitTiles() {
  const children = [...tilesElement.children];
  const count = children.length;
  if (!count) return;
  const gap = parseFloat(getComputedStyle(document.documentElement).fontSize) / 2;
  const width = tilesElement.clientWidth;
  const height = tilesElement.clientHeight;
  let best = { size: 0, columns: 1, rows: count };
  for (let columns = 1; columns <= count; columns++) {
    const rows = Math.ceil(count / columns);
    const size = Math.min((width - gap * (columns + 1)) / columns,
      (height - gap * (rows + 1)) / rows * 16 / 9);
    if (size > best.size) best = { size, columns, rows };
  }
  const tileWidth = Math.max(1, best.size);
  const tileHeight = tileWidth * 9 / 16;
  children.forEach((child, index) => {
    const row = Math.floor(index / best.columns);
    const inRow = Math.min(best.columns, count - row * best.columns);
    const left = (width - inRow * (tileWidth + gap) + gap) / 2;
    const top = (height - best.rows * (tileHeight + gap) + gap) / 2;
    Object.assign(child.style, {
      width: `${tileWidth}px`, height: `${tileHeight}px`,
      left: `${left + index % best.columns * (tileWidth + gap)}px`,
      top: `${top + row * (tileHeight + gap)}px`,
    });
  });
}

function requestVideoSizes() {
  for (const { publication: p } of wanted) {
    const box = boxes.get(p.trackSid);
    if (p.kind !== Track.Kind.Video || !box) continue;
    const width = box.media.clientWidth;
    const height = box.media.clientHeight;
    if (!width || !height) continue;
    const info = p.trackInfo;
    const scale = Math.min(width / info.width, height / info.height) * devicePixelRatio;
    const layers = [...(info.layers?.length ? info.layers : info.codecs?.[0]?.layers) ?? []]
      .filter((layer) => layer.width && layer.height).sort((a, b) => a.width * a.height - b.width * b.height);
    const fitting = layers.find((layer) => layer.width >= info.width * scale
      && layer.height >= info.height * scale) ?? layers.at(-1);
    if (fitting) p.setVideoQuality(fitting.quality);
    else p.setVideoDimensions({ width, height });
  }
}

// The slot of a share among the shares of its person, from the name of its track
// (ADR 0737): `screen/30/1` and `share-sound/1` name slot 1; every other name, as the
// name of a client of one share, is slot 0.
function shareSlot(publication) {
  const parts = (publication.trackName ?? '').split('/');
  const slot = publication.source === Track.Source.ScreenShare
    ? (parts.length === 3 && ['screen', 'window'].includes(parts[0]) ? parts[2] : '0')
    : (parts.length === 2 && parts[0] === 'share-sound' ? parts[1] : '0');
  return /^[0-2]$/.test(slot) ? Number(slot) : 0;
}

function render() {
  const all = entries();
  const liveIds = new Set(all.map(({ publication: p }) => p.trackSid));
  for (const sid of firstSeen.keys()) if (!liveIds.has(sid)) firstSeen.delete(sid);
  for (const { publication: p } of all) {
    if (p.source === Track.Source.ScreenShare && !firstSeen.has(p.trackSid)) {
      // Cache the first stamp: a later mute can advance the server's version.
      const stamp = Number(p.trackInfo?.version?.unixMicro);
      firstSeen.set(p.trackSid, stamp > 0 ? stamp : Date.now() * 1000);
    }
  }
  const videos = all.filter(({ publication: p }) => !p.isMuted
    && [Track.Source.Camera, Track.Source.ScreenShare].includes(p.source));
  const shares = videos.filter(({ publication: p }) => p.source === Track.Source.ScreenShare);
  const main = requestedMain
    ? shares.find(({ publication: p }) => p.trackSid === requestedMain)
    : shares.reduce((latest, entry) => !latest
      || firstSeen.get(entry.publication.trackSid) > firstSeen.get(latest.publication.trackSid)
      ? entry : latest, undefined);
  const sharing = shares.length > 0;
  stage.classList.toggle('sharing', sharing);
  wanted = all.filter((entry) => videos.includes(entry)
    || entry.publication.source === Track.Source.Microphone
    || (main && entry.participant === main.participant
      && entry.publication.source === Track.Source.ScreenShareAudio
      && shareSlot(entry.publication) === shareSlot(main.publication)));
  const keep = new Set(wanted.map(({ publication: p }) => p.trackSid));
  for (const sid of boxes.keys()) if (!keep.has(sid)) removeBox(sid);
  for (const entry of all) {
    const p = entry.publication;
    const desired = keep.has(p.trackSid);
    if (p.isDesired !== desired) p.setSubscribed(desired);
    if (desired && p.kind === Track.Kind.Video) {
      if (!boxes.has(p.trackSid)) p.setVideoQuality(VideoQuality.LOW);
    }
  }
  for (const entry of wanted) {
    const container = entry.publication.kind === Track.Kind.Audio ? soundsElement
      : entry === main ? mainElement : tilesElement;
    show(entry, container);
    if (container === mainElement) boxes.get(entry.publication.trackSid)?.element.removeAttribute('style');
  }
  status.textContent = room.state === ConnectionState.Reconnecting ? messages.reconnecting
    : sharing && !main ? messages.waiting : videos.length ? '' : messages.empty;
  fitTiles();
  requestVideoSizes();
  maybeStart();
}

function scheduleRender() {
  if (queued || ended) return;
  queued = true;
  queueMicrotask(() => { queued = false; if (!ended) render(); });
}

function maybeStart() {
  if (started || ended || !ready || room.state !== ConnectionState.Connected) return;
  if (wanted.some(({ publication: p }) => !p.isSubscribed || !boxes.has(p.trackSid))) return;
  if ([...boxes.values()].some(({ media, track }) => media.paused
    || (track.kind === Track.Kind.Video && media.readyState < 2))) return;
  signalStart();
}

function signalStart() {
  if (started || ended) return;
  started = true;
  clearTimeout(startTimer);
  console.log('START_RECORDING');
}

for (const event of [RoomEvent.TrackPublished, RoomEvent.TrackUnpublished,
  RoomEvent.TrackSubscribed, RoomEvent.TrackUnsubscribed, RoomEvent.TrackMuted,
  RoomEvent.TrackUnmuted, RoomEvent.ParticipantConnected, RoomEvent.ParticipantDisconnected,
  RoomEvent.ParticipantNameChanged, RoomEvent.Reconnecting]) room.on(event, scheduleRender);
room.on(RoomEvent.ParticipantMetadataChanged, (_previous, participant) => {
  if (participant === room.localParticipant) { readLayout(); scheduleRender(); }
});
room.on(RoomEvent.Reconnected, () => { readLayout(); scheduleRender(); });
room.on(RoomEvent.Disconnected, (reason) => {
  ready = false;
  for (const sid of boxes.keys()) removeBox(sid);
  if ([DisconnectReason.ROOM_DELETED, DisconnectReason.PARTICIPANT_REMOVED].includes(reason)) {
    if (ended) return;
    ended = true;
    clearTimeout(startTimer);
    clearTimeout(reconnectTimer);
    status.textContent = messages.ended;
    console.log('END_RECORDING');
  } else {
    status.textContent = messages.reconnecting;
    scheduleReconnect();
  }
});
const resize = new ResizeObserver(() => { fitTiles(); requestVideoSizes(); });
resize.observe(tilesElement);
resize.observe(mainElement);
status.textContent = messages.connecting;

function scheduleReconnect() {
  if (ended || connecting || reconnectTimer) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = undefined;
    void connect();
  }, reconnectDelay);
  reconnectDelay = Math.min(reconnectDelay * 2, 10_000);
}

async function connect() {
  const url = query.get('url');
  try {
    if (!url || !joinToken || !['ws:', 'wss:'].includes(new URL(url).protocol)) throw new Error();
  } catch {
    status.textContent = messages.failed;
    return;
  }
  connecting = true;
  try {
    // Keep the refreshed credential when a terminal SDK disconnect needs a new join.
    joinToken = room.engine?.token || joinToken;
    await room.connect(url, joinToken, { autoSubscribe: false });
    if (ended) return;
    reconnectDelay = 1000;
    if (!started && !startTimer) startTimer = setTimeout(signalStart, 10_000);
    readLayout();
    await Promise.all([room.startAudio(), document.fonts.ready]);
    ready = true;
    render();
  } catch {
    status.textContent = messages.reconnecting;
  } finally {
    connecting = false;
    if (room.state === ConnectionState.Disconnected) scheduleReconnect();
  }
}
void connect();
