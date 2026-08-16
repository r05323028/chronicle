import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightVersions from "starlight-versions";
import zhTwUi from "./src/content/i18n/zh-TW.json" with { type: "json" };
import jaUi from "./src/content/i18n/ja.json" with { type: "json" };

const isGitHubActions = process.env.GITHUB_ACTIONS === "true";
const repository = process.env.GITHUB_REPOSITORY?.split("/").at(-1);
const owner = process.env.GITHUB_REPOSITORY?.split("/")[0]?.toLowerCase();
const site =
  process.env.SITE ??
  (owner ? `https://${owner}.github.io` : "https://r05323028.github.io");
const base =
  process.env.BASE_PATH ??
  (isGitHubActions && repository ? `/${repository}` : "/");

const translated = (zhTw, ja) => ({
  translations: {
    "zh-TW": zhTw,
    ja,
  },
});

const siteTranslations = {
  name: "Chronicle version translations",
  hooks: {
    "i18n:setup"({ injectTranslations }) {
      injectTranslations({
        "zh-TW": {
          ...zhTwUi,
          "starlightVersions.link.latest":
            "切換至<a href={{link}}>最新版本</a>以取得最新文件。",
          "starlightVersions.outdated.label": "此內容適用於 {{label}}。",
          "starlightVersions.outdated.slug": "此內容適用於 {{slug}} 版本。",
          "starlightVersions.search.link.latest":
            "切換至<a href={{link}}>最新版本</a>以取得最新搜尋結果。",
          "starlightVersions.search.outdated.label": "搜尋範圍限於 {{label}}。",
          "starlightVersions.search.outdated.slug":
            "搜尋範圍限於 {{slug}} 版本。",
          "starlightVersions.select.accessibleLabel": "選擇版本",
        },
        ja: {
          ...jaUi,
          "starlightVersions.link.latest":
            "最新のドキュメントを見るには<a href={{link}}>最新バージョン</a>へ移動してください。",
          "starlightVersions.outdated.label":
            "このコンテンツは{{label}}向けです。",
          "starlightVersions.outdated.slug":
            "このコンテンツは{{slug}}バージョン向けです。",
          "starlightVersions.search.link.latest":
            "最新の検索結果を見るには<a href={{link}}>最新バージョン</a>へ移動してください。",
          "starlightVersions.search.outdated.label":
            "検索対象は{{label}}に限定されています。",
          "starlightVersions.search.outdated.slug":
            "検索対象は{{slug}}バージョンに限定されています。",
          "starlightVersions.select.accessibleLabel": "バージョンを選択",
        },
      });
    },
    "config:setup"() {},
  },
};

export default defineConfig({
  site,
  base,
  trailingSlash: "always",
  output: "static",
  integrations: [
    starlight({
      title: "Chronicle",
      defaultLocale: "root",
      locales: {
        root: { label: "English", lang: "en" },
        "zh-tw": { label: "繁體中文", lang: "zh-TW" },
        ja: { label: "日本語", lang: "ja" },
      },
      customCss: ["./src/styles/starlight.css"],
      social: [
        {
          icon: "github",
          label: "Chronicle on GitHub",
          href: "https://github.com/r05323028/chronicle",
        },
      ],
      sidebar: [
        {
          label: "Getting started",
          ...translated("開始使用", "はじめに"),
          items: [
            {
              label: "Introduction",
              ...translated("簡介", "概要"),
              slug: "docs",
            },
            {
              label: "Installation",
              ...translated("安裝", "インストール"),
              slug: "docs/getting-started/installation",
            },
            {
              label: "Quick start",
              ...translated("快速開始", "クイックスタート"),
              slug: "docs/getting-started/quick-start",
            },
          ],
        },
        {
          label: "Concepts",
          ...translated("核心概念", "概念"),
          items: [
            {
              label: "Capture",
              ...translated("擷取", "キャプチャ"),
              slug: "docs/concepts/capture",
            },
            {
              label: "WAL",
              ...translated("WAL", "WAL"),
              slug: "docs/concepts/wal",
            },
            {
              label: "Sessions",
              ...translated("工作階段", "セッション"),
              slug: "docs/concepts/sessions",
            },
            {
              label: "Canonical model",
              ...translated("canonical model", "canonical model"),
              slug: "docs/concepts/canonical-model",
            },
            {
              label: "Replay",
              ...translated("重播", "リプレイ"),
              slug: "docs/concepts/replay",
            },
          ],
        },
        {
          label: "Architecture",
          ...translated("架構", "アーキテクチャ"),
          items: [
            {
              label: "Overview",
              ...translated("總覽", "概要"),
              slug: "docs/architecture/overview",
            },
            {
              label: "Recorder",
              ...translated("Recorder", "レコーダー"),
              slug: "docs/architecture/recorder",
            },
            {
              label: "ETL",
              ...translated("ETL", "ETL"),
              slug: "docs/architecture/etl",
            },
            {
              label: "Storage",
              ...translated("儲存", "ストレージ"),
              slug: "docs/architecture/storage",
            },
          ],
        },
        {
          label: "Deployment",
          ...translated("部署", "デプロイ"),
          items: [
            {
              label: "Local Linux",
              ...translated("本機 Linux", "ローカル Linux"),
              slug: "docs/deployment/local",
            },
          ],
        },
        {
          label: "Reference",
          ...translated("參考", "リファレンス"),
          items: [
            {
              label: "CLI",
              ...translated("CLI", "CLI"),
              slug: "docs/reference/cli",
            },
            {
              label: "Terminology",
              ...translated("術語", "用語"),
              slug: "docs/reference/terminology",
            },
          ],
        },
        {
          label: "Troubleshooting",
          ...translated("疑難排解", "トラブルシューティング"),
          items: [
            {
              label: "Troubleshooting",
              ...translated("疑難排解", "トラブルシューティング"),
              slug: "docs/troubleshooting",
            },
          ],
        },
      ],
      plugins: [
        starlightVersions({
          current: { label: "Latest", redirect: "same-page" },
          versions: [
            { slug: "0-1", label: "0.1", redirect: "same-page" },
          ],
        }),
        siteTranslations,
      ],
    }),
  ],
});
