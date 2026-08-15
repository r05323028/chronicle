---
title: ETL
description: 復原後的證據如何成為一個結果固定的 canonical session。
---

ETL 是完整的 Extract–Transform–Load，不只是 decoder。它負責從復原後的 WAL 證據一路處理到 session reconstruction、protocol handling、canonical validation，以及 atomic storage publication。

## Extract（擷取）

ETL 會透過 recovery authority 掃描 recording WAL。只有到最後一個有效 commit marker 為止的 envelope 才符合資格。它會繼續傳遞 loss window、sequence gap、endpoint evidence 與 provenance，而不是將它們丟棄。

## Transform（轉換）

Session reconstruction 會依 socket generation 與方向分組。目前的 HTTP/1.1 路徑支援有明確上限的 origin-form request、精確的 `Content-Length`、有明確上限的 chunked response、可信任的 close-delimited response，以及循序的 keep-alive exchange。Socket evidence 遺失或互相衝突時會產生具型別的失敗；系統不會捏造操作。

Protocol module 負責 detection、decoding、correlation、canonicalization、replay 與 verification contract。沒有完整實作的 registry entry 只是 scaffolding，不代表支援。

## Load（載入）

ETL 會驗證一個 canonical session，將 manifest、session JSON 與 content-addressed payload 放入私有目錄進行 staging，最後寫入 manifest，並以 atomic 方式發佈 destination。Checkpoint ordering 遵循 publication：尚未發佈的進度不能被 checkpoint 宣稱已完成。

```text
recovered WAL prefix
       │
       ▼
socket/session reconstruction
       │
       ▼
bounded protocol decode
       │
       ▼
canonical validation
       │
       ▼
atomic filesystem publication
```

## 可重新啟動

失敗的 finalization 可以根據持久化的 WAL 與 checkpoint／publication state 繼續。Checkpoint 是進度證據，不是 WAL durability 的替代品，也不是任意輸出的 identity binding。互相矛盾的 metadata 會 fail closed，並保留供診斷使用。
