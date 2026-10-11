import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';

const root = new URL('./', import.meta.url);
const output = new URL('dist/', root);
await mkdir(output, { recursive: true });
for (const file of ['index.html', 'layout.js', 'theme.css', 'messages.js']) {
  await copyFile(new URL(file, root), new URL(file, output));
}
const sdk = new URL('node_modules/livekit-client/dist/livekit-client.umd.js', root);
const sourceMap = JSON.parse(await readFile(new URL(`${sdk.href}.map`), 'utf8'));
const notices = new Set();
const packages = new Set(['livekit-client', '@livekit/protocol']);
for (const [index, source] of sourceMap.sources.entries()) {
  const dependency = source.slice(source.lastIndexOf('/node_modules/') + 1)
    .match(/^node_modules\/((?:@[^/]+\/)?[^/]+)\//)?.[1];
  if (dependency) packages.add(dependency);
  for (const match of (sourceMap.sourcesContent[index] ?? '').matchAll(/\/\*[\s\S]*?\*\/|(?:^\/\/[^\n]*\n?)+/gm)) {
    if (/copyright|@license|SPDX-License-Identifier/i.test(match[0])) notices.add(match[0]);
  }
}
for (const name of [...packages].sort()) {
  const directory = new URL(`node_modules/${name}/`, root);
  const files = (await readdir(directory)).filter((file) => /^(licen[cs]e|notice|copying)([.-]|$)/i.test(file));
  if (!files.length && name !== '@bufbuild/protobuf') throw new Error(`Missing licence: ${name}`);
  for (const file of files) notices.add(`${name} / ${file}\n${await readFile(new URL(file, directory), 'utf8')}`);
}
const banner = [...notices].join('\n\n').split('\n').map((line) => `// ${line}`).join('\n');
await writeFile(new URL('livekit-client.umd.js', output), `${banner}\n${await readFile(sdk, 'utf8')}`);
await copyFile(new URL('node_modules/livekit-client/LICENSE', root),
  new URL('livekit-client.LICENSE', output));
await copyFile(new URL('node_modules/@fontsource/atkinson-hyperlegible-next/files/atkinson-hyperlegible-next-latin-400-normal.woff2', root),
  new URL('atkinson.woff2', output));
await copyFile(new URL('node_modules/@fontsource/atkinson-hyperlegible-next/LICENSE', root),
  new URL('atkinson.LICENSE', output));
console.log('Built recording-layout/dist');
