import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const layout = new URL('../layout.js', import.meta.url);
const original = await readFile(layout, 'utf8');
const mutants = [
  ['readiness', "  if (wanted.some", "  signalStart(); return;\n  if (wanted.some", 'no start before a stalled camera'],
  ['preserve', 'room.on(RoomEvent.Reconnected, () => { readLayout(); scheduleRender(); });',
    "room.on(RoomEvent.Reconnected, () => { requestedMain = ''; scheduleRender(); });", 'main picture'],
  ['quality', '    const width = box.media.clientWidth;',
    '    if (box.requestedOnce) continue;\n    box.requestedOnce = true;\n    const width = box.media.clientWidth;', 'switched share layers fit their new sizes'],
  ['cache', 'p.source === Track.Source.ScreenShare && !firstSeen.has(p.trackSid)',
    'p.source === Track.Source.ScreenShare', 'main picture'],
];
assert.ok(!process.argv[2] || mutants.some(([name]) => name === process.argv[2]), 'unknown mutation');
try {
  for (const [name, before, after, expectedFailure] of mutants) {
    if (process.argv[2] && process.argv[2] !== name) continue;
    assert.ok(original.includes(before), `mutation anchor: ${name}`);
    await writeFile(layout, original.replace(before, after));
    const child = spawn(process.execPath, [fileURLToPath(new URL('run.mjs', import.meta.url))], {
      cwd: fileURLToPath(new URL('../', import.meta.url)),
      env: { ...process.env, JUNTO_LAYOUT_CASE: name, JUNTO_LAYOUT_SKIP_OUTAGE: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    child.stdout.on('data', (chunk) => { output += chunk; });
    child.stderr.on('data', (chunk) => { output += chunk; });
    const code = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', resolve);
    });
    assert.notEqual(code, 0, `${name}: disabled behaviour escaped the test\n${output}`);
    assert.ok(output.includes(expectedFailure), `${name}: failed for the wrong reason\n${output}`);
    console.log(`Detected disabled ${name}: ${expectedFailure}`);
  }
} finally {
  await writeFile(layout, original);
  await import('../build.mjs');
}
