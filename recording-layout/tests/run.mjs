import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createHmac, randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import '../build.mjs';
import { testEnvironment } from './environment.mjs';

const root = new URL('../', import.meta.url);
const repo = fileURLToPath(new URL('../../', import.meta.url));
const livekitUrl = process.env.JUNTO_TEST_LIVEKIT_URL ?? `ws://127.0.0.1:${process.env.JUNTO_TEST_LIVEKIT_PORT ?? '27880'}`;
const roomName = `layout-${randomUUID()}`;
const environment = testEnvironment(process.env);


function token(identity, metadata = '', admin = false) {
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const body = `${encode({ alg: 'HS256', typ: 'JWT' })}.${encode({
    iss: process.env.JUNTO_TEST_LIVEKIT_API_KEY ?? 'devkey', sub: identity, nbf: Math.floor(Date.now() / 1000) - 10,
    exp: Math.floor(Date.now() / 1000) + 3600, metadata,
    video: { room: roomName, roomJoin: !admin, roomAdmin: admin, roomCreate: admin,
      canPublish: identity !== 'recorder', canSubscribe: true,
      canPublishData: true, canUpdateOwnMetadata: false },
  })}`;
  return `${body}.${createHmac('sha256', process.env.JUNTO_TEST_LIVEKIT_API_SECRET ?? 'secret').update(body).digest('base64url')}`;
}

async function api(method, body) {
  const response = await fetch(`${process.env.JUNTO_TEST_LIVEKIT_API_URL ?? livekitUrl.replace(/^ws/, 'http')}/twirp/livekit.RoomService/${method}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token('controller', '', true)}` },
    body: JSON.stringify({ room: roomName, ...body }), signal: AbortSignal.timeout(15_000),
  });
  assert.equal(response.status, 200, `${method}: ${await response.text()}`);
}

async function startLivekit() {
  const child = spawn(`${repo}/tools/with-test-livekit.sh`,
    [process.execPath, fileURLToPath(new URL('hold-livekit.mjs', import.meta.url))],
    { env: environment, stdio: ['pipe', 'pipe', 'pipe'] });
  const exited = once(child, 'exit');
  let errors = '';
  child.stderr.on('data', (data) => { errors += data; });
  await new Promise((resolve, reject) => {
    child.stdout.on('data', (data) => { if (String(data).includes('livekit-ready')) resolve(); });
    child.once('error', reject);
    child.once('exit', (code) => reject(new Error(`LiveKit exited ${code}: ${errors}`)));
  });
  return async () => { child.stdin.end(); await exited; };
}

async function until(check, label, timeout = 45_000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { last = await check(); if (last) return; } catch (error) { last = error.message; }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  assert.fail(`${label}: ${JSON.stringify(last)}`);
}

const types = { html: 'text/html', js: 'text/javascript', css: 'text/css', woff2: 'font/woff2' };
const files = new Map(['index.html', 'layout.js', 'theme.css', 'messages.js', 'livekit-client.umd.js', 'atkinson.woff2']
  .map((file) => [`/${file}`, new URL(`dist/${file}`, root)]));
files.set('/', new URL('dist/index.html', root));
files.set('/participant.html', new URL('participant.html', import.meta.url));
files.set('/participant.js', new URL('participant.js', import.meta.url));
const server = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://localhost').pathname;
  const file = files.get(pathname);
  if (!file) { response.writeHead(404).end(); return; }
  response.setHeader('Content-Type', types[file.pathname.split('.').at(-1)]);
  response.end(await readFile(file));
});

await test('recording layout against LiveKit', async (t) => {
  const load = process.env.JUNTO_LAYOUT_CPU_LOAD === '1'
    ? Array.from({ length: 4 }, () => spawn('nice', ['-n', '19', process.execPath, '-e', 'while(true) {}'])) : [];
  if (load.length) console.log(`CPU load PIDs: ${load.map((child) => child.pid).join(', ')}`);
  await t.test('caller LiveKit variables take precedence', () => {
    const caller = { JUNTO_TEST_LIVEKIT_CONTAINER: 'caller-container', JUNTO_TEST_LIVEKIT_PORT: '28880',
      JUNTO_TEST_LIVEKIT_TCP_PORT: '28881', JUNTO_TEST_LIVEKIT_UDP_PORT: '28882',
      JUNTO_TEST_LIVEKIT_URL: 'ws://example.invalid', JUNTO_TEST_LIVEKIT_API_URL: 'http://example.invalid',
      JUNTO_TEST_WEBHOOK_ADDRESS: '127.0.0.1:28890' };
    for (const [key, value] of Object.entries(caller)) assert.equal(testEnvironment(caller)[key], value, key);
  });
  await t.test('built SDK carries bundled licence notices', async () => {
    const bundle = await readFile(new URL('dist/livekit-client.umd.js', root), 'utf8');
    for (const file of ['@livekit/mutex/LICENSE', '@livekit/protocol/LICENSE', 'events/LICENSE', 'jose/LICENSE.md',
      'loglevel/LICENSE-MIT', 'machina/LICENSE', 'sdp/LICENSE', 'sdp-transform/LICENSE', 'webrtc-adapter/LICENSE.md']) {
      const licence = await readFile(new URL(`node_modules/${file}`, root), 'utf8');
      assert.ok(bundle.includes(licence.split('\n').map((line) => `// ${line}`).join('\n')), `missing full licence: ${file}`);
    }
    for (const notice of ['The WebRTC project authors', 'Buf Technologies', 'ts-debounce', 'Permission is hereby granted', 'Redistribution and use']) {
      assert.ok(bundle.includes(notice), `missing bundled notice: ${notice}`);
    }
  });
  let stopLivekit;
  let browser;
  try {
    stopLivekit = await startLivekit();
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    const base = `http://127.0.0.1:${server.address().port}`;
    browser = await chromium.launch({ headless: true, args: [
      '--autoplay-policy=no-user-gesture-required', '--mute-audio',
      '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
    ] });
    const context = await browser.newContext({ viewport: { width: 1280, height: 720 } });
    const publishers = [];
    async function participant(identity) {
      const page = await context.newPage();
      await page.goto(`${base}/participant.html`);
      await page.waitForFunction(() => window.join);
      await page.evaluate(([url, jwt]) => join(url, jwt), [livekitUrl, token(identity)]);
      publishers.push(page);
      return page;
    }
    const first = await participant('first');
    const second = await participant('second');
    const voice = await participant('voice-only');
    const publish = (page, source, frequency) => page.evaluate(([source, frequency]) => window.publish(source, frequency), [source, frequency]);
    const tracks = {};
    tracks.firstCamera = await publish(first, 'camera');
    tracks.firstMic = await publish(first, 'microphone', 440);
    tracks.firstShare = await publish(first, 'screen_share');
    tracks.firstSound = await publish(first, 'screen_share_audio', 1000);
    tracks.secondCamera = await publish(second, 'camera');
    tracks.secondMic = await publish(second, 'microphone', 620);
    tracks.secondShare = await publish(second, 'screen_share');
    tracks.secondSound = await publish(second, 'screen_share_audio', 1370);
    tracks.voice = await publish(voice, 'microphone', 790);
    tracks.unknown = await publish(voice, 'unknown');
    const recorderContext = await browser.newContext({ viewport: { width: 1280, height: 720 } });
    const page = await recorderContext.newPage();
    const markers = [];
    const errors = [];
    page.on('console', (message) => {
      if (['START_RECORDING', 'END_RECORDING'].includes(message.text())) markers.push(message.text());
    });
    page.on('pageerror', (error) => errors.push(error.message));
    async function open(layout = '', metadata = '') {
      await page.goto(`${base}/?${new URLSearchParams({ url: livekitUrl, token: token('recorder', metadata), layout })}`);
      await page.waitForFunction(async () => {
        const { room } = await import('/layout.js');
        window.room = room;
        return room.state === 'connected';
      });
    }
    async function mainIs(sid) {
      await until(() => page.evaluate((sid) => {
        const video = document.querySelector('#main video');
        return video?.parentElement.dataset.trackSid === sid && video.videoWidth > 0 && !video.paused;
      }, sid), `main picture ${sid}`);
    }
    async function subscriptions(expected) {
      await until(() => page.evaluate((expected) => {
        const all = [...room.remoteParticipants.values()].flatMap((p) => [...p.trackPublications.values()]);
        const actual = all.filter((p) => p.isSubscribed && p.track).map((p) => p.trackSid).sort();
        return JSON.stringify(actual) === JSON.stringify([...expected].sort())
          && all.filter((p) => !expected.includes(p.trackSid)).every((p) => !p.isDesired && !p.track);
      }, expected), 'exact subscriptions and detached unwanted tracks');
    }
    async function sounds(expected) {
      await until(() => page.evaluate(async (expected) => {
        const elements = [...document.querySelectorAll('audio')];
        if (JSON.stringify(elements.map((e) => e.parentElement.dataset.trackSid).sort()) !== JSON.stringify([...expected].sort())) return false;
        window.audioAnalysis ??= new AudioContext();
        window.analyzers ??= new WeakMap();
        await audioAnalysis.resume();
        return elements.every((element) => {
          if (element.paused || element.muted || element.volume !== 1 || !element.srcObject) return false;
          let analyser = analyzers.get(element);
          if (!analyser) {
            analyser = audioAnalysis.createAnalyser();
            analyser.fftSize = 2048;
            audioAnalysis.createMediaStreamSource(element.srcObject).connect(analyser);
            analyzers.set(element, analyser);
          }
          const samples = new Float32Array(analyser.fftSize);
          analyser.getFloatTimeDomainData(samples);
          return Math.sqrt(samples.reduce((sum, value) => sum + value * value, 0) / samples.length) > 0.02;
        });
      }, expected), 'every attached audio stream plays non-silent decoded samples');
    }
    const baseTracks = () => [tracks.firstCamera, tracks.firstMic, tracks.firstShare,
      tracks.secondCamera, tracks.secondMic, tracks.secondShare, tracks.voice];
    async function update(layout) {
      await api('UpdateParticipant', { identity: 'recorder', metadata: JSON.stringify({ layout }) });
    }
    async function tilesFit() {
      await until(() => page.evaluate(() => [...document.querySelectorAll('.tile')].every((tile) => {
        const r = tile.getBoundingClientRect();
        return r.width > 0 && r.height > 0 && r.left >= 0 && r.top >= 0
          && r.right <= innerWidth + 1 && r.bottom <= innerHeight + 1;
      })), 'all tiles fit inside the output');
    }

    const selectedCase = process.env.JUNTO_LAYOUT_CASE;
    const caseNames = {
      readiness: ['stalled camera'], deadline: ['stalled camera'], labels: ['labels show'],
      quality: ['live layout update'], cache: ['empty update', 'mute updates'],
      preserve: ['empty update', 'recorder full reconnect'], outage: ['outage beyond'],
    };
    const scenario = (name, run) => t.test(name, {
      skip: selectedCase && !name.startsWith('start request wins')
        && !caseNames[selectedCase]?.some((part) => name.startsWith(part)),
    }, run);
    await scenario('start request wins, exact subscriptions, low layers, all intended sound', async () => {
      await open(`main:${tracks.firstShare}`);
      await mainIs(tracks.firstShare);
      await subscriptions([...baseTracks(), tracks.firstSound]);
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice, tracks.firstSound]);
      await until(() => markers.filter((m) => m === 'START_RECORDING').length === 1, 'one start signal');
      await until(() => page.evaluate(() => [...document.querySelectorAll('#tiles video')].every((v) => v.videoWidth === 320)
        && document.querySelector('#main video').videoWidth === 1280), 'actual low tile and high main video layers');
      assert.equal(await page.evaluate(() => [...room.remoteParticipants.values()].flatMap((p) => [...p.videoTrackPublications.values()])
        .filter((p) => p.isSubscribed).every((p) => p.videoQuality === (p.trackSid === document.querySelector('#main .tile').dataset.trackSid ? 2 : 0))), true);
      assert.equal(await page.locator('.tile').count(), 4);
      await tilesFit();
      if (process.env.JUNTO_LAYOUT_SCREENSHOT) await page.screenshot({ path: process.env.JUNTO_LAYOUT_SCREENSHOT });
    });
    await scenario('live layout update switches picture and sound; untrusted metadata and data do not', async () => {
      await update(`main:${tracks.secondShare}`);
      await mainIs(tracks.secondShare);
      await until(() => page.evaluate(() => document.querySelector('#main video')?.videoWidth === 1280
        && [...document.querySelectorAll('#tiles video')].every((v) => v.videoWidth === 320)), 'switched share layers fit their new sizes');
      await subscriptions([...baseTracks(), tracks.secondSound]);
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice, tracks.secondSound]);
      await page.evaluate(() => {
        window.receivedLayoutData = false;
        room.once(LivekitClient.RoomEvent.DataReceived, () => { receivedLayoutData = true; });
      });
      await api('UpdateParticipant', { identity: 'first', metadata: JSON.stringify({ layout: `main:${tracks.firstShare}` }) });
      await api('UpdateRoomMetadata', { metadata: JSON.stringify({ main_share: tracks.firstShare }) });
      await first.evaluate((sid) => publisher.localParticipant.publishData(new TextEncoder().encode(JSON.stringify({ layout: `main:${sid}` })), { reliable: true, topic: 'layout' }), tracks.firstShare);
      await until(() => page.evaluate(() => receivedLayoutData
        && room.remoteParticipants.get('first').metadata.includes('layout')
        && room.metadata.includes('main_share')), 'untrusted updates arrived');
      await mainIs(tracks.secondShare);
    });
    await scenario('empty update selects latest share and new publications change the fallback', async () => {
      await update('');
      await mainIs(tracks.secondShare);
      await first.evaluate((sid) => unpublish(sid), tracks.firstShare);
      tracks.firstShare = await publish(first, 'screen_share');
      await mainIs(tracks.firstShare);
      await subscriptions([...baseTracks(), tracks.firstSound]);
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice, tracks.firstSound]);
      await update('{invalid-layout');
      await mainIs(tracks.firstShare);
    });
    await scenario('mute updates do not reorder cached shares', async () => {
      await update('');
      await mainIs(tracks.firstShare);
      await second.evaluate((sid) => mute(sid, true), tracks.secondShare);
      await subscriptions([...baseTracks().filter((sid) => sid !== tracks.secondShare), tracks.firstSound]);
      await page.evaluate((sid) => {
        window.advancedVersion = false;
        // Replay a newer track-info version at the real unmute event.
        const advance = (publication) => {
          if (publication.trackSid !== sid) return;
          publication.trackInfo.version = { unixMicro: BigInt(Date.now() * 1000 + 1_000_000) };
          advancedVersion = true;
          room.off(LivekitClient.RoomEvent.TrackUnmuted, advance);
        };
        room.on(LivekitClient.RoomEvent.TrackUnmuted, advance);
      }, tracks.secondShare);
      await second.evaluate((sid) => mute(sid, false), tracks.secondShare);
      await until(() => page.evaluate(() => advancedVersion), 'unmute delivered the newer version');
      await mainIs(tracks.firstShare);
      await subscriptions([...baseTracks(), tracks.firstSound]);
    });
    await scenario('named missing share waits; arrival, mute and unpublish remove unwanted media', async () => {
      await update('main:TR_missing');
      await until(() => page.locator('#main video').count().then((n) => n === 0), 'absent named share has no replacement');
      await subscriptions(baseTracks());
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice]);
      await update(`main:${tracks.firstShare}`);
      await mainIs(tracks.firstShare);
      await first.evaluate((sid) => mute(sid, true), tracks.firstShare);
      await subscriptions(baseTracks().filter((sid) => sid !== tracks.firstShare));
      assert.equal(await page.locator('#main video').count(), 0);
      await first.evaluate((sid) => mute(sid, false), tracks.firstShare);
      await mainIs(tracks.firstShare);
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice, tracks.firstSound]);
    });
    await scenario('a participant leaves and rejoins with a new microphone', async () => {
      const oldMicrophone = tracks.voice;
      await voice.evaluate(() => publisher.disconnect());
      await subscriptions([...baseTracks().filter((sid) => sid !== oldMicrophone), tracks.firstSound]);
      await voice.evaluate(([url, jwt]) => join(url, jwt), [livekitUrl, token('voice-only')]);
      tracks.voice = await publish(voice, 'microphone', 790);
      tracks.unknown = await publish(voice, 'unknown');
      assert.notEqual(tracks.voice, oldMicrophone);
      await subscriptions([...baseTracks(), tracks.firstSound]);
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice, tracks.firstSound]);
      assert.equal(await page.locator('.tile').count(), 4);
    });
    await scenario('recorder full reconnect preserves a choice and accepts the controller resend', async () => {
      await update(`main:${tracks.secondShare}`);
      await mainIs(tracks.secondShare);
      await page.evaluate(() => {
        window.reconnections = 0;
        room.on(LivekitClient.RoomEvent.Reconnected, () => reconnections++);
        return room.simulateScenario('full-reconnect');
      });
      await until(() => page.evaluate(() => reconnections > 0 && room.state === 'connected'), 'recorder reconnect');
      await mainIs(tracks.secondShare);
      await sounds([tracks.firstMic, tracks.secondMic, tracks.voice, tracks.secondSound]);
      await update(`main:${tracks.firstShare}`);
      await mainIs(tracks.firstShare);
      await update(`main:${tracks.secondShare}`);
      await mainIs(tracks.secondShare);
      await subscriptions([...baseTracks(), tracks.secondSound]);
      assert.deepEqual(markers, ['START_RECORDING']);
    });
    await scenario('media server restart reconnects, replaces tracks and follows the new name', async () => {
      const previousShare = tracks.secondShare;
      await stopLivekit();
      stopLivekit = await startLivekit();
      await until(() => page.evaluate(() => reconnections >= 2 && room.state === 'connected'), 'media restart reconnect', 90_000);
      await until(() => second.evaluate(() => publisher.state === 'connected'), 'publisher reconnect', 90_000);
      await until(() => page.evaluate(() => room.remoteParticipants.size === 3
        && [...room.remoteParticipants.values()].flatMap((p) => [...p.trackPublications.values()]).length === 10),
      'all publishers and replacement tracks returned', 90_000);
      const sources = await second.evaluate(() => Object.fromEntries([...publisher.localParticipant.trackPublications.values()].map((p) => [p.source, p.trackSid])));
      assert.notEqual(sources.screen_share, previousShare);
      await update(`main:${sources.screen_share}`);
      await mainIs(sources.screen_share);
      const expected = await page.evaluate((mainSound) => [...room.remoteParticipants.values()].flatMap((p) => [...p.trackPublications.values()])
        .filter((p) => ['camera', 'screen_share', 'microphone'].includes(p.source) || p.trackSid === mainSound).map((p) => p.trackSid), sources.screen_share_audio);
      await subscriptions(expected);
      const audio = await page.evaluate((mainSound) => [...room.remoteParticipants.values()].flatMap((p) => [...p.trackPublications.values()])
        .filter((p) => p.source === 'microphone' || p.trackSid === mainSound).map((p) => p.trackSid), sources.screen_share_audio);
      await sounds(audio);
      assert.deepEqual(markers, ['START_RECORDING']);
    });
    await scenario('no shares gives a fitted camera grid; participants with no video are absent', async () => {
      for (const publisher of [first, second]) {
        await publisher.evaluate(async () => {
          for (const p of [...window.publisher.localParticipant.videoTrackPublications.values()]) {
            if (p.source === 'screen_share') await window.publisher.localParticipant.unpublishTrack(p.track);
          }
        });
      }
      await update('');
      await until(() => page.evaluate(() => document.querySelectorAll('#tiles video').length === 2
        && document.querySelectorAll('#main video').length === 0
        && !document.querySelector('#stage').classList.contains('sharing')), 'camera grid');
      assert.equal(await page.locator('#main video').count(), 0);
      assert.equal(await page.locator('#stage').getAttribute('class'), '');
      await until(() => page.evaluate(() => [...document.querySelectorAll('#tiles video')].every((v) => v.videoWidth === 640)), 'large grid cameras receive fitting layers');
      for (let i = 0; i < 7; i++) await publish(voice, 'camera');
      await until(() => page.locator('#tiles video').count().then((n) => n === 9), 'nine camera tiles');
      await tilesFit();
      await page.setViewportSize({ width: 640, height: 360 });
      await tilesFit();
      await publish(second, 'screen_share');
      await until(() => page.locator('#main video').count().then((n) => n === 1), 'share with nine small tiles');
      await tilesFit();
    });
    await scenario('disconnect emits the end signal once and removes all media', async () => {
      await api('RemoveParticipant', { identity: 'recorder' });
      await until(() => markers.includes('END_RECORDING'), 'end marker');
      assert.deepEqual(markers, ['START_RECORDING', 'END_RECORDING']);
      assert.equal(await page.locator('video, audio').count(), 0);
    });
    await scenario('initial recorder metadata overrides an outdated URL', async () => {
      const existing = await second.evaluate(() => [...publisher.localParticipant.videoTrackPublications.values()].find((p) => p.source === 'screen_share')?.trackSid);
      const sid = existing ?? await publish(second, 'screen_share');
      await open('main:TR_old', JSON.stringify({ layout: `main:${sid}` }));
      await mainIs(sid);
      assert.deepEqual(errors, []);
    });
    await scenario('labels show names only and hide empty names', async () => {
      await api('UpdateParticipant', { identity: 'first', name: 'Display <name>' });
      await until(() => page.locator('.label').allTextContents().then((labels) => labels.includes('Display <name>')), 'display name label');
      assert.equal(await page.locator('.label').allTextContents().then((labels) => labels.some((label) => label.includes('second') || label.includes('voice-only'))), false, 'identities never become labels');
      await first.evaluate(() => publisher.disconnect());
      await first.evaluate(([url, jwt]) => join(url, jwt), [livekitUrl, token('first')]);
      const camera = await publish(first, 'camera');
      await page.waitForFunction((sid) => document.querySelector(`[data-track-sid="${sid}"] video`), camera);
      await until(() => page.locator('.label:visible').count().then((n) => n === 0), 'empty names have no visible label');
    });
    await scenario('stalled camera waits for readiness but start is bounded at ten seconds', async () => {
      const sid = await publish(first, 'camera');
      await page.waitForFunction((sid) => [...room.remoteParticipants.values()].some((p) => p.trackPublications.has(sid)), sid);
      await first.evaluate((sid) => stall(sid), sid);
      await page.goto('about:blank');
      markers.length = 0;
      const joined = Date.now();
      await open();
      await page.waitForFunction((sid) => [...room.remoteParticipants.values()].some((p) => p.trackPublications.has(sid)), sid);
      await new Promise((resolve) => setTimeout(resolve, 2000));
      assert.deepEqual(markers, [], 'no start before a stalled camera has a frame or the deadline');
      assert.equal(await page.evaluate((sid) => {
        const video = document.querySelector(`[data-track-sid="${sid}"] video`);
        return video?.readyState >= 2;
      }, sid), false, 'fixture delivers no video frame');
      try {
        await until(() => markers.includes('START_RECORDING'), 'stalled camera cannot block start beyond the deadline', 20_000);
        assert.ok(Date.now() - joined >= 9500, 'start waits for the ten-second deadline');
      } finally {
        await first.evaluate((sid) => unpublish(sid), sid);
      }
    });
    if (!process.env.JUNTO_LAYOUT_SKIP_OUTAGE) await scenario('outage beyond SDK retries resumes without an end signal', async () => {
      await page.evaluate(() => {
        window.sdkGaveUp = false;
        room.once(LivekitClient.RoomEvent.Disconnected, () => { sdkGaveUp = true; });
      });
      await stopLivekit();
      await until(() => page.evaluate(() => sdkGaveUp), 'SDK exhausted its own retries', 180_000);
      assert.equal(markers.includes('END_RECORDING'), false, 'connection loss never ends the recording');
      stopLivekit = await startLivekit();
      for (const [publisher, identity] of [[first, 'first'], [second, 'second'], [voice, 'voice-only']]) {
        await publisher.evaluate(() => window.publisher.disconnect());
        await publisher.evaluate(([url, jwt]) => join(url, jwt), [livekitUrl, token(identity)]);
      }
      const sid = await publish(second, 'screen_share');
      const sound = await publish(second, 'screen_share_audio', 1370);
      await until(() => page.evaluate(() => room.state === 'connected'), 'page reconnects after SDK gave up', 45_000);
      await update(`main:${sid}`);
      await mainIs(sid);
      await sounds([sound]);
      assert.deepEqual(markers, ['START_RECORDING']);
      await api('DeleteRoom', {});
      await until(() => markers.includes('END_RECORDING'), 'room closure ends recording');
      assert.deepEqual(markers, ['START_RECORDING', 'END_RECORDING']);
    });
  } finally {
    await browser?.close();
    await stopLivekit?.();
    server.close();
    for (const child of load) child.kill('SIGTERM');
    await Promise.all(load.map((child) => once(child, 'exit')));
  }
});
