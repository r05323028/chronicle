import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const distDir = path.join(websiteDir, 'dist');
const docsDir = path.join(websiteDir, 'src', 'content', 'docs');
const marketingCss = readFileSync(path.join(websiteDir, 'src', 'styles', 'marketing.css'), 'utf8');
const starlightCss = readFileSync(path.join(websiteDir, 'src', 'styles', 'starlight.css'), 'utf8');
const expressiveCodeConfig = readFileSync(path.join(websiteDir, 'ec.config.mjs'), 'utf8');
const base = (process.env.BASE_PATH ?? '/').replace(/\/$/, '') || '/';
const pathPrefix = base === '/' ? '' : base;
const locales = ['zh-tw', 'ja'];
const htmlFiles = [];
const errors = [];
const log = (message) => process.stdout.write(`${message}\n`);
const fail = (message) => process.stderr.write(`${message}\n`);

if (!existsSync(distDir)) {
  errors.push('website/dist does not exist; run npm run build first.');
}

function walk(directory, relative = '') {
  if (!existsSync(directory)) return;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const absolute = path.join(directory, entry.name);
    const next = path.posix.join(relative, entry.name);
    if (entry.isDirectory()) walk(absolute, next);
    else if (entry.isFile() && entry.name.endsWith('.html')) htmlFiles.push(next);
  }
}

walk(distDir);

if (marketingCss.includes('width: min(100%, var(--content-width));')) {
  errors.push('marketing surfaces still use a capped width instead of full-bleed layout.');
}
if (!marketingCss.includes('--page-gutter:')) errors.push('marketing page gutter token missing.');
for (const token of ['--code-bg:', '--code-fg:', '--code-red:']) {
  if (!marketingCss.includes(token)) errors.push(`marketing code token missing: ${token}`);
  if (!starlightCss.includes(token)) errors.push(`Starlight code token missing: ${token}`);
}
const normalizedEcConfig = expressiveCodeConfig.replaceAll('"', "'");
for (const token of ["codePaddingInline: '1.25rem'", "codePaddingBlock: '1rem'"]) {
  if (!normalizedEcConfig.includes(token)) errors.push(`Expressive Code padding override missing: ${token}`);
}
for (const token of ['--color-paper: #f2efe7;', '--color-night: #121918;']) {
  if (!marketingCss.includes(token)) errors.push(`marketing page palette missing: ${token}`);
}
for (const token of ['--chronicle-paper: #f2efe7;', '--chronicle-night: #121918;']) {
  if (!starlightCss.includes(token)) errors.push(`Starlight page palette missing: ${token}`);
}
for (const token of ['tokyo-night', 'tokyo-night-day', '#e1e2e7', '#1a1b26', '#f52a65', '#f7768e']) {
  if (!expressiveCodeConfig.includes(token)) errors.push(`Expressive Code Tokyo Night token missing: ${token}`);
}

function routeFile(route) {
  const clean = decodeURIComponent(route.split(/[?#]/, 1)[0]).replace(/^\/+/, '');
  const candidates = clean.endsWith('/')
    ? [path.join(distDir, clean, 'index.html')]
    : [path.join(distDir, clean), path.join(distDir, clean, 'index.html')];
  return candidates.find((candidate) => existsSync(candidate) && statSync(candidate).isFile());
}

function routeForLink(raw, source) {
  const withoutFragment = raw.split(/[?#]/, 1)[0];
  if (!withoutFragment || withoutFragment.startsWith('#')) return null;
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(withoutFragment)) return null;

  if (withoutFragment.startsWith('/')) {
    if (base !== '/' && withoutFragment !== base && !withoutFragment.startsWith(`${base}/`)) {
      errors.push(`wrong base path: ${source} -> ${raw}`);
      return null;
    }
    return base === '/' ? withoutFragment : withoutFragment.slice(base.length) || '/';
  }

  const sourceDirectory = path.posix.dirname(`/${source}`);
  return path.posix.normalize(path.posix.join(sourceDirectory, withoutFragment));
}

for (const source of htmlFiles) {
  const html = readFileSync(path.join(distDir, source), 'utf8');
  for (const match of html.matchAll(/(?:href|src)=["']([^"']+)["']/g)) {
    const route = routeForLink(match[1], source);
    if (route && !routeFile(route)) errors.push(`missing asset or route: ${source} -> ${match[1]}`);
  }
}

const versionSlugs = readdirSync(path.join(docsDir), { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && !['docs', ...locales].includes(entry.name))
  .map((entry) => entry.name)
  .sort();
const requiredRoutes = ['/', '/docs/', ...locales.map((locale) => `/${locale}/`), ...locales.map((locale) => `/${locale}/docs/`)];
for (const slug of versionSlugs) {
  requiredRoutes.push(`/${slug}/docs/`, ...locales.map((locale) => `/${locale}/${slug}/docs/`));
}
for (const route of requiredRoutes) {
  if (!routeFile(route)) errors.push(`required route missing: ${route}`);
}

const pagefindDir = path.join(distDir, 'pagefind');
if (!existsSync(pagefindDir) || !readdirSync(pagefindDir).length) errors.push('Pagefind output missing.');

const latestDocs = routeFile('/docs/');
const firstVersion = versionSlugs[0];
if (latestDocs && firstVersion) {
  const latestHtml = readFileSync(latestDocs, 'utf8');
  const archivedDocs = routeFile(`/${firstVersion}/docs/`);
  const archivedHtml = archivedDocs ? readFileSync(archivedDocs, 'utf8') : '';
  if (!latestHtml.includes(`${pathPrefix}/${firstVersion}/`)) errors.push('Latest docs has no archived-version switch link.');
  if (!archivedHtml.includes(`${pathPrefix}/docs/`)) errors.push('Archived docs has no Latest switch link.');
  if (!latestHtml.includes('pagefind')) errors.push('Latest docs has no Pagefind search asset.');
  const localizedVersionWarnings = [
    [`/zh-tw/${firstVersion}/docs/`, '此內容適用於'],
    [`/ja/${firstVersion}/docs/`, 'このコンテンツは'],
  ];
  for (const [route, marker] of localizedVersionWarnings) {
    const page = routeFile(route);
    if (!page || !readFileSync(page, 'utf8').includes(marker)) {
      errors.push(`localized version warning missing: ${route}`);
    }
  }
}

for (const locale of ['', ...locales]) {
  const route = locale ? `/${locale}/` : '/';
  const file = routeFile(route);
  if (!file) continue;
  const html = readFileSync(file, 'utf8');
  for (const other of ['', ...locales].filter((candidate) => candidate !== locale)) {
    const target = other ? `${pathPrefix}/${other}/` : `${pathPrefix}/`;
    if (!html.includes(target)) errors.push(`locale switch missing: ${route} -> ${target}`);
  }
}

if (errors.length > 0) {
  fail(`Static output verification failed (${errors.length} issue${errors.length === 1 ? '' : 's'}):`);
  fail(errors.slice(0, 20).map((error) => `- ${error}`).join('\n'));
  process.exit(1);
}

log(`Verified ${htmlFiles.length} HTML pages, ${versionSlugs.length} archived version${versionSlugs.length === 1 ? '' : 's'}, locales en/zh-tw/ja, base ${base}, internal routes/assets, locale/version switches, and Pagefind.`);
