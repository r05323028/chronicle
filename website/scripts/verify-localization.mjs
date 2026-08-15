import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const websiteDir = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const docsDir = path.join(websiteDir, "src", "content", "docs");
const manifestPath = path.join(websiteDir, "localization-manifest.json");
const locales = ["zh-tw", "ja"];
const forbiddenTerms = {
  "zh-tw": [
    ["捕捉", "擷取"],
    ["規範模型", "canonical model"],
    ["規範工作階段", "canonical session"],
    ["有界", "有明確上限的"],
    ["解析最新", "取得最新"],
    ["前一則", "上一頁"],
    ["下一則", "下一頁"],
    ["效果授權", "操作授權"],
    ["WAL 實體上限", "WAL 的實體容量上限"],
  ],
  ja: [
    ["捕捉", "キャプチャ"],
    ["再生", "リプレイ"],
    ["正規セッション", "canonical session"],
    ["正規モデル", "canonical model"],
    ["有界", "上限付き"],
    ["効果の許可", "操作の認可"],
    ["表面", "対応範囲"],
    ["監視下のコピー", "監視対象のコピー"],
    ["WAL の物理上限", "WAL の物理容量上限"],
    ["fallback", "フォールバック"],
  ],
};
const update = process.argv.includes("--update");
const errors = [];
const log = (message) => process.stdout.write(`${message}\n`);
const fail = (message) => errors.push(message);

const read = (filePath) =>
  readFileSync(filePath, "utf8").replace(/\r\n/g, "\n");
const sha256 = (value) => createHash("sha256").update(value).digest("hex");
const relative = (filePath) =>
  path.relative(websiteDir, filePath).split(path.sep).join("/");
const normalize = (value) => value.replace(/[ \t]+$/gm, "").trim();

function markdownBody(value) {
  return value.replace(/^---\n[\s\S]*?\n---\n/, "");
}

function withoutCode(value) {
  const lines = value.split("\n");
  const result = [];
  let fenced = false;
  for (const line of lines) {
    if (/^\s*(```+|~~~+)/.test(line)) {
      fenced = !fenced;
      continue;
    }
    if (!fenced) result.push(line);
  }
  return result.join("\n");
}

function headings(value) {
  return withoutCode(markdownBody(value))
    .split("\n")
    .flatMap((line) => {
      const match = /^(#{1,6})\s+/.exec(line);
      return match ? [match[1].length] : [];
    });
}

function fencedBlocks(value) {
  const lines = markdownBody(value).split("\n");
  const blocks = [];
  let opening = null;
  let body = [];
  for (const line of lines) {
    const open = /^\s*(```+|~~~+)(.*)$/.exec(line);
    if (!opening && open) {
      opening = open[1][0] + open[2].trim();
      body = [];
    } else if (
      opening &&
      new RegExp(
        `^\\s*${opening[0]}{${opening.length > 0 ? 3 : 3},}\\s*$`,
      ).test(line)
    ) {
      blocks.push(`${opening}\n${body.join("\n")}`);
      opening = null;
    } else if (opening) {
      body.push(line);
    }
  }
  return blocks;
}

function inlineCode(value) {
  return [...withoutCode(markdownBody(value)).matchAll(/`([^`\n]+)`/g)].map(
    (match) => match[1],
  );
}

function linkDestinations(value) {
  return [
    ...markdownBody(value).matchAll(/\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g),
  ].map((match) => match[1]);
}

function hasLocalizedText(line) {
  return /[\u3040-\u30ff\u3400-\u9fff]/u.test(line);
}

function englishLeakLines(value) {
  const common =
    /\b(?:the|this|that|and|are|with|from|into|only|when|before|after|does|will|not|for|of|to|is|has|have|can)\b/i;
  return withoutCode(markdownBody(value))
    .split("\n")
    .filter((line) => {
      const clean = line.replace(/`[^`]*`/g, "").replace(/https?:\/\/\S+/g, "");
      const words = clean.trim().split(/\s+/).filter(Boolean);
      return (
        !hasLocalizedText(clean) && words.length >= 5 && common.test(clean)
      );
    });
}

function sourceFiles() {
  const result = [];
  const addRoot = (slug, root) => {
    if (!existsSync(root)) return;
    const walk = (directory) => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const filePath = path.join(directory, entry.name);
        if (entry.isDirectory()) walk(filePath);
        else if (entry.isFile() && entry.name.endsWith(".md")) {
          const fileRelative = path
            .relative(root, filePath)
            .split(path.sep)
            .join("/");
          const sourceKey = `${slug}/${fileRelative}`;
          result.push({
            sourceKey,
            sourcePath: filePath,
            localePaths: Object.fromEntries(
              locales.map((locale) => [
                locale,
                path.join(
                  docsDir,
                  locale,
                  slug === "docs" ? "docs" : path.join(slug, "docs"),
                  fileRelative,
                ),
              ]),
            ),
          });
        }
      }
    };
    walk(root);
  };

  addRoot("docs", path.join(docsDir, "docs"));
  for (const entry of readdirSync(docsDir, { withFileTypes: true })) {
    if (entry.isDirectory() && !["docs", ...locales].includes(entry.name)) {
      addRoot(entry.name, path.join(docsDir, entry.name, "docs"));
    }
  }
  return result.sort((left, right) =>
    left.sourceKey.localeCompare(right.sourceKey),
  );
}

const sources = sourceFiles();
if (sources.length === 0)
  fail("No canonical English documentation pages found.");

const expected = {};
for (const page of sources) {
  const english = read(page.sourcePath);
  expected[page.sourceKey] = { sourceSha256: sha256(english) };
  const sourceHeadingShape = headings(english);
  const sourceCode = fencedBlocks(english);
  const sourceInlineCode = inlineCode(english);
  const sourceLinks = linkDestinations(english);

  for (const locale of locales) {
    const localePath = page.localePaths[locale];
    if (!existsSync(localePath)) {
      fail(`${page.sourceKey}: missing ${locale} page ${relative(localePath)}`);
      continue;
    }
    const localized = read(localePath);
    const localizedBody = normalize(markdownBody(localized));
    const englishBody = normalize(markdownBody(english));
    if (localizedBody === englishBody) {
      fail(`${page.sourceKey}: ${locale} is an exact English fallback`);
    }
    if (
      /This content is not available in your language yet\.|本頁內容尚未翻譯|このページはまだ日本語に翻訳されていません/u.test(
        localizedBody,
      )
    ) {
      fail(
        `${page.sourceKey}: ${locale} contains an untranslated-content notice`,
      );
    }
    const localizedHeadingShape = headings(localized);
    if (
      JSON.stringify(localizedHeadingShape) !==
      JSON.stringify(sourceHeadingShape)
    ) {
      fail(
        `${page.sourceKey}: ${locale} heading structure differs (expected ${sourceHeadingShape.join(",") || "none"}, got ${localizedHeadingShape.join(",") || "none"})`,
      );
    }
    const localizedCode = fencedBlocks(localized);
    if (JSON.stringify(localizedCode) !== JSON.stringify(sourceCode)) {
      fail(
        `${page.sourceKey}: ${locale} changed a fenced code block or command`,
      );
    }
    const localizedInlineCode = inlineCode(localized);
    for (const token of sourceInlineCode) {
      if (!localizedInlineCode.includes(token))
        fail(
          `${page.sourceKey}: ${locale} removed inline code token \`${token}\``,
        );
    }
    const localizedLinks = linkDestinations(localized);
    for (const destination of sourceLinks) {
      if (!localizedLinks.includes(destination))
        fail(
          `${page.sourceKey}: ${locale} changed link destination ${destination}`,
        );
    }
    for (const line of englishLeakLines(localized)) {
      fail(
        `${page.sourceKey}: ${locale} has possible English prose leak: ${line.trim()}`,
      );
    }
    if (!page.sourceKey.endsWith("reference/terminology.md")) {
      const bodyForTermCheck = withoutCode(markdownBody(localized));
      for (const [term, preferred] of forbiddenTerms[locale]) {
        if (bodyForTermCheck.includes(term)) {
          fail(
            `${page.sourceKey}: ${locale} uses ${term}; prefer ${preferred}`,
          );
        }
      }
    }
  }
}

if (!update) {
  if (existsSync(manifestPath)) {
    let manifest;
    try {
      manifest = JSON.parse(read(manifestPath));
    } catch (error) {
      fail(`${relative(manifestPath)} is not valid JSON: ${error.message}`);
    }
    const recorded = manifest?.sources ?? {};
    for (const sourceKey of Object.keys(expected)) {
      if (!recorded[sourceKey])
        fail(
          `${sourceKey}: missing freshness entry in ${relative(manifestPath)}`,
        );
      else if (
        recorded[sourceKey].sourceSha256 !== expected[sourceKey].sourceSha256
      ) {
        fail(
          `${sourceKey}: English source changed; review both translations and refresh the manifest`,
        );
      }
    }
    for (const sourceKey of Object.keys(recorded)) {
      if (!expected[sourceKey])
        fail(`${sourceKey}: freshness entry has no canonical English page`);
    }
  } else {
    fail(
      `Missing ${relative(manifestPath)}; run npm run update:localization after reviewing translations.`,
    );
  }
} else if (errors.length === 0) {
  writeFileSync(
    manifestPath,
    `${JSON.stringify({ schemaVersion: 1, algorithm: "sha256", sources: expected }, null, 2)}\n`,
  );
  log(
    `Updated ${relative(manifestPath)} for ${sources.length} canonical English pages and locales zh-tw/ja.`,
  );
}

if (errors.length > 0) {
  process.stderr.write(
    `Localization validation failed (${errors.length} issue${errors.length === 1 ? "" : "s"}):\n`,
  );
  process.stderr.write(`${errors.map((error) => `- ${error}`).join("\n")}\n`);
  process.exit(1);
}

if (!update)
  log(
    `Verified ${sources.length} canonical English pages with complete zh-tw/ja counterparts, aligned structure/tokens/links, no fallback bodies, and fresh source hashes.`,
  );
