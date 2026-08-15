import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const configPath = path.join(websiteDir, 'astro.config.mjs');
const docsDir = path.join(websiteDir, 'src', 'content', 'docs');
const versionsDir = path.join(websiteDir, 'src', 'content', 'versions');
const log = (message) => process.stdout.write(`${message}\n`);
const fail = (message) => process.stderr.write(`${message}\n`);
const requested = process.argv[2]?.replace(/^v/, '');
const match = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(requested ?? '');

if (!match) {
  fail(`Invalid release version '${process.argv[2] ?? ''}'. Expected semantic version like 0.2.0.`);
  process.exit(1);
}

const docsLabel = `${match[1]}.${match[2]}`;
const slug = `${match[1]}-${match[2]}`;
const config = readFileSync(configPath, 'utf8');
const entry = new RegExp(`slug:\\s*['"]${slug}['"]`);
const required = [
  path.join(docsDir, slug, 'docs', 'index.md'),
  path.join(docsDir, 'ja', slug, 'docs', 'index.md'),
  path.join(docsDir, 'zh-tw', slug, 'docs', 'index.md'),
  path.join(versionsDir, `${slug}.json`),
];
const missing = required.filter((filePath) => !existsSync(filePath));

if (!entry.test(config) || missing.length > 0) {
  fail(`Release ${requested} requires committed docs version ${docsLabel}.`);
  if (!entry.test(config)) fail(`- astro.config.mjs has no ${docsLabel} version entry`);
  for (const filePath of missing) fail(`- missing ${path.relative(websiteDir, filePath)}`);
  process.exit(1);
}

log(`Release ${requested}: committed docs snapshot ${docsLabel} found for English, Traditional Chinese, Japanese, and sidebar metadata.`);
