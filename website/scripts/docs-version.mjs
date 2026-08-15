import { existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const configPath = path.join(websiteDir, 'astro.config.mjs');
const docsDir = path.join(websiteDir, 'src', 'content', 'docs');
const versionsDir = path.join(websiteDir, 'src', 'content', 'versions');
const locales = ['ja', 'zh-tw'];
const log = (message) => process.stdout.write(`${message}\n`);
const fail = (message) => process.stderr.write(`${message}\n`);
const requested = process.argv[2]?.replace(/^v/, '');
const match = /^(\d+)\.(\d+)$/.exec(requested ?? '');

if (!match) {
  fail('Usage: npm run docs:version -- <minor-version> (for example, 0.2)');
  process.exit(1);
}

const label = `${match[1]}.${match[2]}`;
const slug = `${match[1]}-${match[2]}`;
const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const versionEntry = new RegExp(`slug:\\s*['"]${escapeRegExp(slug)}['"]`);
const snapshotPaths = [
  path.join(docsDir, slug, 'docs', 'index.md'),
  path.join(docsDir, 'ja', slug, 'docs', 'index.md'),
  path.join(docsDir, 'zh-tw', slug, 'docs', 'index.md'),
  path.join(versionsDir, `${slug}.json`),
];
const existing = snapshotPaths.filter((filePath) => existsSync(filePath));
const config = readFileSync(configPath, 'utf8');

if (versionEntry.test(config) || existing.length > 0) {
  if (versionEntry.test(config) && existing.length === snapshotPaths.length) {
    log(`Docs version ${label} already exists; no archive changes needed.`);
  } else {
    fail(`Docs version ${label} is partially present. Repair it manually before retrying.`);
    fail(existing.map((filePath) => `- ${path.relative(websiteDir, filePath)}`).join('\n'));
    process.exit(1);
  }
} else {
  const versionsBlock = /(versions:\s*\[\n)([\s\S]*?)(\n\s*\],)/m.exec(config);
  if (!versionsBlock) {
    fail('Could not find multiline starlight-versions configuration in astro.config.mjs.');
    process.exit(1);
  }

  const entry = `            { slug: '${slug}', label: '${label}', redirect: 'same-page' },`;
  const updatedConfig = config.replace(
    versionsBlock[0],
    `${versionsBlock[1]}${entry}\n${versionsBlock[2].trimEnd()}${versionsBlock[3]}`,
  );
  writeFileSync(configPath, updatedConfig);
  log(`Configured Starlight Versions entry ${label}.`);
}

const runBuild = () =>
  spawnSync('npm', ['run', 'build'], {
    cwd: websiteDir,
    env: process.env,
    stdio: 'inherit',
  });

const build = runBuild();
if (build.status !== 0) {
  process.exit(build.status ?? 1);
}

const archivedSlugs = readdirSync(docsDir, { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && !['docs', ...locales, slug].includes(entry.name))
  .map((entry) => entry.name);
const nestedLocaleSnapshots = locales
  .flatMap((locale) => archivedSlugs.map((archivedSlug) => path.join(docsDir, locale, slug, archivedSlug)))
  .filter((filePath) => existsSync(filePath));

for (const filePath of nestedLocaleSnapshots) {
  rmSync(filePath, { recursive: true, force: true });
}

if (nestedLocaleSnapshots.length > 0) {
  log('Removed stale archived-locale copies from new snapshot; rebuilding.');
  const rebuild = runBuild();
  if (rebuild.status !== 0) {
    process.exit(rebuild.status ?? 1);
  }
}

const missing = snapshotPaths.filter((filePath) => !existsSync(filePath));
if (missing.length > 0) {
  fail('Build completed without materializing the expected version snapshot:');
  fail(missing.map((filePath) => `- ${path.relative(websiteDir, filePath)}`).join('\n'));
  process.exit(1);
}

const verify = spawnSync('npm', ['run', 'verify:build'], {
  cwd: websiteDir,
  env: process.env,
  stdio: 'inherit',
});
if (verify.status !== 0) {
  process.exit(verify.status ?? 1);
}

log(`Docs version ${label} is ready. Review and commit:`);
log([
  'astro.config.mjs',
  `src/content/docs/${slug}/`,
  `src/content/docs/ja/${slug}/`,
  `src/content/docs/zh-tw/${slug}/`,
  `src/content/versions/${slug}.json`,
].map((filePath) => `- ${filePath}`).join('\n'));
