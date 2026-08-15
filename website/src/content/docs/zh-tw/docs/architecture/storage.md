---
title: 儲存
description: canonical session 與 payload 在本機檔案系統中的發佈邊界。
---

Chronicle 目前會將 recording 與 canonical session 儲存在本機檔案系統。Storage 負責持久化與 atomic publication；replay 讀取已持久化的 canonical artifact，而不是 WAL 內部細節。

## 公開 data directory

公開命令依照以下順序解析 data directory：

1. `--data-dir DIR`；
2. 設定檔中的 `data_dir`；
3. `CHRONICLE_DATA_DIR`；
4. 平台預設值。

會修改資料的命令只在需要時建立私有目錄，並拒絕不安全的 root 或 symlink 形式。`doctor` 會回報既有或預計使用的位置，不會建立 probe artifact。

```text
<data-dir>/
  .chronicle-domain.lock
  catalog.json
  recordings/<bare-recording-uuid>/
  sessions/<session-uuid>/
```

一個標準化的 `.chronicle-domain.lock` 會保護 name claim、capture、ETL、publication 與 catalog update，將它們視為一個 transaction。

## canonical session 的發佈

每個 session 會以以下形式發佈：

```text
sessions/<session-id>/
  manifest.json
  session.json
  payloads/<sha256>
```

在 Unix 上，staging directory 使用 `0700`，檔案使用 `0600`。最後才寫入 manifest，publication 只會將 destination 重新命名為尚不存在的位置。Inspect 會驗證 artifact metadata；replay 會 hydrate payload 並檢查 SHA-256。

## 這裡沒有的功能

PostgreSQL metadata storage、S3 相容的 artifact storage、遠端 WAL 封存、靜態資料加密、redaction policy 與 tenant isolation 尚未實作。不要從 storage interface 或 protocol registry 推論這些功能已存在。

:::caution
本機 artifact 可能包含 production header、body、憑證與個人資料。請將 data directory 視為敏感資料，並套用主機層級的存取控制。
:::
