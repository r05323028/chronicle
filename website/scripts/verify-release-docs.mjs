import { existsSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const websiteDir = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const configPath = path.join(websiteDir, "astro.config.mjs");
const docsDir = path.join(websiteDir, "src", "content", "docs");
const versionsDir = path.join(websiteDir, "src", "content", "versions");
const log = (message) => process.stdout.write(`${message}\n`);
const fail = (message) => process.stderr.write(`${message}\n`);
const requested = process.argv[2]?.replace(/^v/, "");
const match = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(requested ?? "");

if (!match) {
  fail(
    `Invalid release version '${process.argv[2] ?? ""}'. Expected semantic version like 0.2.0.`,
  );
  process.exit(1);
}

const docsLabel = `${match[1]}.${match[2]}`;
const slug = `${match[1]}-${match[2]}`;
const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const config = readFileSync(configPath, "utf8");
const entry = new RegExp(`slug:\\s*['"]${escapeRegExp(slug)}['"]`);
const required = [
  path.join(docsDir, slug, "docs", "index.md"),
  path.join(docsDir, "ja", slug, "docs", "index.md"),
  path.join(docsDir, "zh-tw", slug, "docs", "index.md"),
  path.join(versionsDir, `${slug}.json`),
];
const missing = required.filter((filePath) => !existsSync(filePath));

const snapshotRoots = [
  { locale: "en", root: path.join(docsDir, slug, "docs") },
  { locale: "zh-tw", root: path.join(docsDir, "zh-tw", slug, "docs") },
  { locale: "ja", root: path.join(docsDir, "ja", slug, "docs") },
];
const releaseStateContradictions = [
  {
    id: "public-release-not-yet-available",
    locale: "en",
    pattern:
      /public release does not exist|has not published its first stable release|from the first public release|starting with the first public release/i,
  },
  {
    id: "public-release-not-yet-available",
    locale: "zh-tw",
    pattern:
      /尚未發布第一個穩定版本|首次公開版本發布後|自首次公開版本起|將支援/u,
  },
  {
    id: "public-release-not-yet-available",
    locale: "ja",
    pattern:
      /最初の安定リリースはまだ公開されていません|最初の公開リリース以降|対応する予定です/u,
  },
];

function markdownFiles(root) {
  if (!existsSync(root)) return [];
  const files = [];
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const filePath = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(filePath);
      else if (entry.isFile() && entry.name.endsWith(".md"))
        files.push(filePath);
    }
  };
  visit(root);
  return files;
}

const contradictions = [];
for (const { locale, root } of snapshotRoots) {
  for (const filePath of markdownFiles(root)) {
    const content = readFileSync(filePath, "utf8");
    for (const predicate of releaseStateContradictions) {
      if (predicate.locale === locale && predicate.pattern.test(content)) {
        contradictions.push(
          `- ${path.relative(websiteDir, filePath)}: ${predicate.id}`,
        );
      }
    }
  }
}

if (!entry.test(config) || missing.length > 0 || contradictions.length > 0) {
  fail(`Release ${requested} requires committed docs version ${docsLabel}.`);
  if (!entry.test(config))
    fail(`- astro.config.mjs has no ${docsLabel} version entry`);
  for (const filePath of missing)
    fail(`- missing ${path.relative(websiteDir, filePath)}`);
  if (contradictions.length > 0) {
    fail("- release snapshot contains pre-release state:");
    fail(contradictions.join("\n"));
  }
  process.exit(1);
}

log(
  `Release ${requested}: committed docs snapshot ${docsLabel} found for English, Traditional Chinese, Japanese, and sidebar metadata.`,
);
