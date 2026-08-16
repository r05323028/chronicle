---
title: canonical model
description: "`inspect` 與 `replay` 使用的、與協定及儲存無關的 session 表示法。"
---

canonical session 是 Chronicle 在擷取與重播之間交接的資料。它以穩定的模型保存應用程式行為，而不是保存 eBPF hook、WAL framing 或特定 storage backend。

## 內容

目前的 v1 model 包含：

- 強型別的 recording、session、connection 與 operation identity；
- 根據 socket evidence 得出的 canonical client/server endpoint；
- 以相對 nanosecond offset 表示的 deterministic operation timeline；
- 具型別的 operation kind 與 effect 分類；
- request 與 recorded-response payload reference；
- connection 與 operation completeness map；
- source provenance、loss window、integrity 與 WAL checkpoint 資訊；
- replay attribute 與明確的 blocker；
- 與核心欄位分開保存的 protocol extension bytes。

Timeline 是唯一的 operation order。Completeness map 具有權威性。跨 epoch operation 會保留 completion owner 與 contributing epoch/WAL provenance；只有 continuation 或 incomplete outcome 不可重播。若 endpoint evidence 遺失或互相衝突，ETL 會失敗；它絕不會儲存捏造的 `unknown:0` endpoint。

## 唯一可變動的 v1 契約

目前的 canonical schema 是 `CANONICAL_SCHEMA_VERSION = 1`。Reader 會拒絕其他版本。在明確的 compatibility freeze 之前，Chronicle 不會維護歷史 migration reader；未來變更版本時，必須在獨立的 design change 中定義相容性與 migration policy。

## 檔案系統 artifact

本機 store 會發佈：

```text
sessions/<session-id>/
  manifest.json
  session.json
  payloads/<sha256>
```

Manifest 會記錄 identity、canonical version、checksum、payload 數量與大小、WAL checkpoint、有明確上限的 issues、completeness 與 replay blocker。Unix 上的檔案會使用 private mode。Replay 會 hydrate payload，並在送出任何資料前檢查 SHA-256。

## 可攜性邊界

Replay 只消費 canonical session 與 protocol interface，不依賴 capture、eBPF、WAL、ETL 或原始 storage implementation。這種分離讓同一個 replay core 可以處理 fixture 產生的 session，也可以處理 production 產生的 session。
