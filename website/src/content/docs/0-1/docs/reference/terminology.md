---
title: Terminology
description: Canonical Chronicle terms and their localized forms.
slug: 0-1/docs/reference/terminology
---

English is the canonical documentation language. Localized pages preserve CLI commands, flags, schema names, and format names exactly.

| English | Traditional Chinese (`zh-TW`) | Japanese (`ja`) | Meaning |
| --- | --- | --- | --- |
| capture | 捕捉 | キャプチャ | Socket lifecycle and payload evidence observed for a selected workload. |
| recorder | recorder／錄製器 | recorder／レコーダー | Process or command that owns recording lifecycle. Keep `recorder` for the internal service. |
| WAL | WAL／預寫式日誌 | WAL／先行書き込みログ | Write-ahead log; the local durability boundary. |
| ETL | ETL | ETL | Complete Extract–Transform–Load path to canonical publication. |
| canonical model | 規範模型 | 正規モデル | Protocol- and storage-independent recording representation. |
| replay | 重播 | リプレイ | Plan or execute recorded behavior against an authorized loopback target. |
| session | 工作階段 | セッション | Canonical unit containing connections, operations, integrity, and replayability. |
| checkpoint | 檢查點 | チェックポイント | Persisted ETL progress tied to publication ordering. |

## Usage rules

* Keep `WAL`, `ETL`, `HTTP/1.1`, `eBPF`, `cgroup`, `loopback`, and CLI flags unchanged.
* Use **recording** for the user-facing bounded capture lifecycle and **session** for the canonical replay unit.
* Say **canonical session**, not “normalized recording,” when referring to the persisted model.
* Say **recovered committed prefix**, not “whatever was in the WAL,” when describing ETL input.
* When discussing replay, describe the mode: command mode grants execution/read for the owned supervised target after planning; explicit-target mode stays dry-run until `--execute` and all required gates are supplied; writes always require explicit authorization.
