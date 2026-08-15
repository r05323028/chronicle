---
title: WAL
description: Chronicle 為什麼會在轉換前先持久化擷取到的證據。
slug: zh-tw/0-1/docs/concepts/wal
---

Chronicle 會先將擷取到的證據寫入分段、只能追加的 write-ahead log，再由 ETL 解讀。WAL 是本機的持久性邊界，也是下游處理可以消費哪些資料的復原依據。

## Commit marker 定義可持久化的前綴

每個 group 會追加 data frame，在 WAL 中追加一個 `CommitMarker`，執行 flush，再進行一次 `fdatasync`。只有 sync 成功後才會回報 acknowledgement。最後一個有效 marker 證明已持久化的前綴；它不會被外部 watermark 檔案取代。

```text
segment 00000000000000000000.chwal
  capture event
  capture event
  commit marker  ← durable prefix
  capture event  ← complete but uncommitted suffix
```

ETL 只會讀取由復原流程認定為權威的已提交前綴。最後一個有效 marker 之後的完整 frame，仍然會以未提交尾端的形式存在，永遠不會成為 canonical evidence。

## 復原行為

復原流程會驗證 segment header、envelope version、recording identity、sequence continuity、frame checksum、marker reference、累積總量與 marker digest。只有在驗證通過後，才可能修復最後一個不完整的 frame 或 marker 尾端。完整損毀、identity 不符、reference 無效、不支援的版本與 sequence gap 都會 fail closed。

重新開啟 recording 時，系統會從最後一個權威 marker 之後繼續，只移除回報的未提交尾端，並保留 sequence continuity。系統絕不會只根據 bytes 推斷呼叫端曾看見 acknowledgement。

## 實體容量上限

Segment 大小限制在 16 MiB 到 4 GiB 之間。`segments/` 下的總容量預設為 4 GiB，且絕不會超過 4 GiB。寫入器會在寫入前預留完整 data frame 與最後 marker 的空間；輪替時也會預留下一個 header 與暫時的 publication peak。

佇列納入失敗與 WAL limit 失敗都會保留為具型別的證據，不會被隱藏成成功的擷取。

:::caution
WAL 與 payload artifact 可能包含 production header、body、憑證與個人資料。私有檔案權限是保護措施，不代表靜態資料加密、完整資料遮罩或 tenant isolation。
:::

目前的 v1 framing contract 請參考儲存庫的 [WAL 格式](https://github.com/r05323028/chronicle/blob/main/docs/wal-format.md)。
