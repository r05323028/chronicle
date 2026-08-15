import { existsSync, readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const configPath = path.join(websiteDir, 'astro.config.mjs');
const docsDir = path.join(websiteDir, 'src', 'content', 'docs');
const versionsDir = path.join(websiteDir, 'src', 'content', 'versions');
const locales = ['ja', 'zh-tw'];
const config = readFileSync(configPath, 'utf8');
const versionsBlock = /(versions:\s*\[\n)([\s\S]*?)(\n\s*\],)/m.exec(config);
const errors = [];
const fail = (message) => process.stderr.write(`${message}\n`);

if (!versionsBlock) {
  errors.push('starlight-versions configuration is missing or not multiline.');
}

const configured = versionsBlock
  ? [...versionsBlock[2].matchAll(/slug:\s*["']([^"']+)["']/g)].map((match) => match[1])
  : [];
const sourceVersions = readdirSync(docsDir, { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && !['docs', ...locales].includes(entry.name))
  .map((entry) => entry.name);
const parseMinorSlug = (slug) => {
  const match = /^(\d+)-(\d+)$/.exec(slug);
  return match ? [Number(match[1]), Number(match[2])] : null;
};
const parsedVersions = configured.map(parseMinorSlug);
if (parsedVersions.some((version) => version === null)) {
  errors.push('configured versions must use minor slugs such as 0-2.');
} else {
  const newestFirst = [...configured].sort((left, right) => {
    const [leftMajor, leftMinor] = parseMinorSlug(left);
    const [rightMajor, rightMinor] = parseMinorSlug(right);
    return rightMajor - leftMajor || rightMinor - leftMinor;
  });
  if (configured.some((slug, index) => slug !== newestFirst[index])) {
    errors.push('configured versions must be ordered newest to oldest.');
  }
}

for (const slug of configured) {
  const required = [
    path.join(docsDir, slug, 'docs', 'index.md'),
    path.join(docsDir, 'ja', slug, 'docs', 'index.md'),
    path.join(docsDir, 'zh-tw', slug, 'docs', 'index.md'),
    path.join(versionsDir, `${slug}.json`),
  ];
  for (const filePath of required) {
    if (!existsSync(filePath)) errors.push(`configured version ${slug} missing ${path.relative(websiteDir, filePath)}`);
  }
}

for (const slug of sourceVersions) {
  if (!configured.includes(slug)) errors.push(`source version ${slug} is absent from astro.config.mjs`);
}
for (const slug of configured) {
  if (!sourceVersions.includes(slug)) errors.push(`configured version ${slug} has no source snapshot`);
}

if (errors.length > 0) {
  fail(`Documentation version verification failed (${errors.length} issue${errors.length === 1 ? '' : 's'}):`);
  fail(errors.map((error) => `- ${error}`).join('\n'));
  process.exit(1);
}

process.stdout.write(`Verified ${configured.length} committed documentation version${configured.length === 1 ? '' : 's'} with English, Traditional Chinese, Japanese, and sidebar metadata.\n`);
