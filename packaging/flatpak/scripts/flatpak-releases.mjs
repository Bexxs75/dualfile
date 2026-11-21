#!/usr/bin/env node
// Offline: only read this checkout's changelog; emit XML, never execute source code.
import { readFileSync } from 'node:fs';
const changelog = readFileSync(new URL('../../../CHANGELOG.md', import.meta.url), 'utf8');
const escapeXml = value => value.replace(/[&<>"']/g, c => ({
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;',
}[c]));
const entries = [...changelog.matchAll(/^## \[(\d+\.\d+\.\d+)\] - (\d{4}-\d{2}-\d{2})\r?\n([\s\S]*?)(?=^## |$(?![\s\S]))/gm)].slice(0, 3);
if (entries.length !== 3) throw new Error('Expected at least three dated changelog releases');
console.log('  <releases>');
for (const [, version, date, body] of entries) {
  const items = [...body.matchAll(/^- (.+)$/gm)].map(([, text]) => text.replace(/`([^`]+)`/g, '$1'));
  if (!items.length) throw new Error(`No release text for ${version}`);
  console.log(`    <release version="${escapeXml(version)}" date="${date}">\n      <description>\n        <ul>`);
  for (const item of items) console.log(`          <li>${escapeXml(item)}</li>`);
  console.log('        </ul>\n      </description>\n    </release>');
}
console.log('  </releases>');
