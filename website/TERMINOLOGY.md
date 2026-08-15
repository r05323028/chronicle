# Chronicle documentation localization

English is canonical. Localized pages preserve commands, flags, filenames, API names, configuration keys, schema names, protocol names, and format versions exactly. Translate explanatory prose, not identifiers.

| English | Traditional Chinese (`zh-TW`) | Japanese (`ja`) | Usage |
| --- | --- | --- | --- |
| Chronicle | Chronicle | Chronicle | Product name; never translate. |
| capture | 擷取 | キャプチャ | Observation boundary for socket lifecycle and payload evidence. Avoid 「在工作負載周圍捕捉」; describe connecting from outside a command, process, or cgroup. |
| recorder | recorder／錄製器 | recorder／レコーダー | Process or command that owns recording lifecycle. Keep `recorder` for the internal service name. |
| WAL | WAL／預寫式日誌 | WAL／先行書き込みログ | Write-ahead log and local durability boundary. Explain once, then keep `WAL`. |
| ETL | ETL | ETL | Complete Extract–Transform–Load path from recovered evidence to canonical publication. |
| canonical model | canonical model | canonical model | Stable, protocol- and storage-independent representation shared by capture, ETL, storage, and replay. Do not translate as 「規範模型」 or 「正規モデル」 when that suggests a normative specification. |
| canonical session | canonical session | canonical session | Portable session represented by the canonical model. Explain the stable shared representation on first use. |
| session | session／工作階段 | セッション | Canonical unit containing connections, operations, integrity, and replayability. Use `session` in technical prose when 「工作階段」 would sound forced. |
| replay | 重播 | リプレイ | Plan or execute recorded behavior against an authorized loopback target. |
| workload | 工作負載 | ワークロード | Command, process, or cgroup being observed. Use 「有明確上限的工作負載」 for a bounded workload, not 「有界工作負載」. |
| operation / effect | 操作；effect 是安全分類，不是「效果」 | 操作；effect は安全性の分類 | Operation behavior and its read/write/other safety classification. Translate replay authorization as 「操作授權」 or 「操作の認可」. |
| authorization | 授權 | 認可 | Policy permission for a replay operation. |
| storage | 儲存／storage | ストレージ | Persistence and atomic publication boundary. |
| fixture | fixture／測試資料 | fixture | Test input source that does not require live capture. Keep `fixture` in technical names. |
| trace | trace／追蹤記錄 | トレース | Ordered event record used for diagnosis or correlation; Chronicle’s public units are recording and session. |
| live capture | live capture／即時擷取 | ライブキャプチャ | Evidence collected while a workload runs. |
| bounded | 有明確上限的／受限制的 | 上限付き／制限された | Explicit duration, capacity, or protocol limit. |
| fallback | 備援路徑（fallback） | フォールバック | Switching to another target when the primary target is unavailable. Replay never falls back to a production destination. |
| deterministic | deterministic（結果固定且可重現） | deterministic（再現性のある） | Same inputs and rules produce predictable, reproducible results. |
| checkpoint | checkpoint／檢查點 | チェックポイント | Persisted ETL progress tied to publication ordering. |

## Writing rules

- Use `上一頁` and `下一頁` in Traditional Chinese UI, never 「前一則」 or 「下一則」.
- Use `キャプチャ`, `リプレイ`, `フォールバック`, and `現時点で利用できる範囲` in Japanese; never literal 「捕捉」「再生」「表面」 for these product concepts.
- Translate `resolve the latest stable release` as 「取得最新穩定版本」 or 「最新の安定リリースを取得」, not 「解析／解決」.
- Translate physical capacity as 「實體容量上限」 or `物理容量上限`; translate `static encryption` as 「靜態資料加密」 or `保存時暗号化`.
- Keep safety defaults explicit: `dry-run` by default, effects denied until the relevant operation authorization is present, and no production-destination fallback.
- Keep user-facing **recording** for the bounded capture lifecycle and **session** for the canonical replay unit. Say **canonical session**, not “normalized recording”.

## Freshness contract

`website/localization-manifest.json` records SHA-256 hashes of every canonical English page. Run `npm run verify:localization` from `website/` after editing docs. A changed English page, a new English page, a missing locale page, an exact English fallback body, a structural mismatch, or changed code/link tokens fails validation. The byte hash is deliberately conservative: formatting or frontmatter changes also require review, preventing silent omissions. After reviewing and updating both locales, refresh hashes with `npm run update:localization` and commit the manifest.

## Framework exception

The current Starlight/Expressive Code build still emits the inaccessible `Terminal window` screen-reader fallback for shell code blocks, even with the supported `expressiveCode.*` translations registered. Do not fork or patch the dependency for this harmless upstream string; revisit it when the framework exposes a working override. Starlight navigation, headings, theme/language controls, and version UI are localized.
