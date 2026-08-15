# Website maintainers

## Normal documentation change

Edit Latest English source under `website/src/content/docs/docs/`. Edit the corresponding Traditional Chinese and Japanese pages under `website/src/content/docs/zh-tw/docs/` and `website/src/content/docs/ja/docs/`; English is canonical, but every exposed page must have both localized counterparts. Run `npm run verify:localization` after reviewing translations. Terminology and freshness workflow live in [TERMINOLOGY.md](TERMINOLOGY.md). Open a PR. The Website workflow runs localization/version checks, `npm ci`, `npm run check`, `npm run build`, and static-output verification. Merge to `main` publishes one GitHub Pages site.

## Localization

English changes require a review of both `zh-tw` and `ja` pages. `npm run verify:localization` fails on missing pages, structural/token drift, English fallback bodies, or stale canonical source hashes. After both translations are reviewed, run `npm run update:localization` and commit `localization-manifest.json`. See [TERMINOLOGY.md](TERMINOLOGY.md) for the small glossary and writing rules.

## New minor release

Finish release documentation in Latest, then create one committed minor-version snapshot:

```bash
cd website
npm ci
npm run docs:version -- 0.2
```

Use `0.2`, not `0.2.0`. The command validates the minor format, updates `astro.config.mjs`, invokes the installed Starlight Versions archive process during `npm run build`, copies English plus `zh-tw` and `ja` content, writes sidebar metadata, verifies the snapshot, and prints paths to review and commit.

Review and commit the generated `0-2` directories and `src/content/versions/0-2.json` with the config change. Keep archived snapshots mostly immutable. Normal evolution belongs in Latest. The release workflow requires the matching minor snapshot for tags such as `v0.2.0`.

## Patch release

Reuse its existing minor docs version. `v0.2.1` still uses the `0.2` snapshot; do not create `0.2.1` docs unless compatibility genuinely needs a separate snapshot.

## Deployment and recovery

Pushes to `main` deploy after validation. Pull requests validate and build but never deploy production. To rerun deployment, open **Actions → Website → Run workflow** on `main`; the workflow is read-only for repository source and rebuilds the complete site from committed files.

GitHub Pages must use **GitHub Actions** as its source and the `github-pages` environment. This repository is a project site: production base is derived as `/${repository}` in CI (currently `/chronicle`). No `CNAME` is configured.
