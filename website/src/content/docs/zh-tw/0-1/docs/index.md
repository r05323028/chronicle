---
title: 介紹
description: 將真實應用程式行為轉為確定性、可重播的回歸測試證據。
slug: zh-tw/0-1/docs
---

Chronicle 記錄真實應用程式流量，將其轉為確定性、可重播的回歸測試證據。

它可以附加在受監督的指令、執行中的程序或 cgroup 周圍，不需要在應用程式加入插樁。捕捉到的證據會先寫入本機預寫式日誌（WAL），再被重建為不依賴協定的規範工作階段；重播只會針對明確授權的 loopback 目標。

:::caution
捕捉到的流量可能包含憑證與個人資料。重播可能產生副作用。Chronicle 預設為 dry-run，拒絕所有效果，也永遠不會回退到記錄中的生產目的地。
:::

## 目前支援

目前 0.1.x 的範圍刻意保持狹窄：

* Linux 上以 eBPF 即時捕捉有界明文 HTTP/1.1 流量。
* 圍繞指令、既有程序或 cgroup 錄製。
* 具備 WAL 內提交標記的分段、可當機復原 WAL。
* 將一個確定性規範工作階段發佈到本機檔案系統的 ETL。
* 具 loopback 授權的安全指令模式與明確目標重播。
* 所有平台的 fixture 錄製、檢查、目錄列出與非破壞性就緒檢查。

TLS 解密、HTTP/2+、其他協定、遠端持久化、靜態加密、完整遮罩、Docker 與 Kubernetes 封裝尚未實作。

## 路徑

```text
應用程式行為 → eBPF 捕捉 → WAL → ETL → 規範工作階段 → loopback 重播
```

先閱讀[安裝](./getting-started/installation/)，再依照[快速開始](./getting-started/quick-start/)。術語請參考[術語](./reference/terminology/)。
