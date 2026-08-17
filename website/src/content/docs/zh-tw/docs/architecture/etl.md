---
title: ETL
description: 復原後的證據如何成為結果固定的 canonical session——每個完成定稿的 epoch 各一個。
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

## 增量處理

錄製期間中，ETL 會針對具復原權威的 committed WAL prefix 增量執行：發佈 canonical delta batch，並只在發佈後推進 durable incremental checkpoint。Epoch rollover 會持久化有明確上限、經 checksum 與 lineage 驗證的 continuation evidence，因此跨 epoch 的狀態仍可處理；WAL epoch 邊界不是協定重建邊界。Finalization 會針對每個完成定稿的 epoch，執行一個決定性 canonical session 的最終權威性發佈。

## 部署獨立性

目前的本機部署可以將 Recorder 與 ETL 放在同一程序中，但 ETL 透過 durable evidence contract 消費具復原權威的證據，不需要擷取所有權、Recorder 程序或共用的檔案系統命名空間。ETL 擁有 canonical publication、發佈驗證與 checkpoint advancement ordering。

## 可重新啟動

失敗的 finalization 可以根據持久化的 WAL 與 checkpoint／publication state 繼續。Checkpoint 是進度證據，不是 WAL durability 的替代品，也不是任意輸出的 identity binding。互相矛盾的 metadata 會 fail closed，並保留供診斷使用。
