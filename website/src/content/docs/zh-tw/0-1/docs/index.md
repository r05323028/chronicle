---
title: 簡介
description: 將真實應用程式行為轉為結果固定、可重播的回歸測試證據。
slug: zh-tw/0-1/docs
---

Chronicle 會記錄真實的應用程式流量，將其轉為結果固定、可重播的回歸測試證據。

它會從受監督的命令、執行中的程序或 cgroup 外部掛接，不要求在應用程式中加入 instrumentation。擷取到的證據會先寫入本機 write-ahead log（WAL），再重建為不依賴協定的 canonical session；重播只會針對明確授權的 loopback target。

:::caution
擷取到的流量可能包含憑證與個人資料。重播可能產生副作用。Chronicle 預設以 dry-run 執行，所有操作都拒絕，也永遠不會改用記錄中的 production 目的地。
:::

## 目前支援的範圍

目前 0.1.x 的功能範圍刻意保持狹窄：

- Linux 上透過 eBPF 進行 live capture，擷取有明確上限的明文 HTTP/1.1 流量。
- 針對命令、既有程序或 cgroup 進行錄製。
- 具備 WAL 內 commit marker、可在當機後復原的分段 WAL。
- ETL 針對每個完成定稿的 epoch，將一個結果固定的 canonical session 發佈到本機檔案系統儲存空間。
- 具備 loopback 授權的安全 command mode 與 explicit-target replay。
- 所有平台都支援 fixture 錄製、檢查、列出 catalog，以及非破壞性的就緒檢查。

TLS 解密、HTTP/2 以上版本、其他協定實作、遠端持久化、靜態資料加密、完整資料遮罩、Docker 封裝與 Kubernetes 封裝目前都尚未實作。

## 流程

```text
application behavior
        │
        ▼
eBPF capture evidence
        │
        ▼
segmented WAL ── durable commit boundary
        │
        ▼
ETL ── recover, decode, account for loss
        │
        ▼
canonical session ── inspect and store
        │
        ▼
loopback replay ── verify, never production fallback
```

先閱讀[安裝](./getting-started/installation/)，再依照[快速開始](./getting-started/quick-start/)。需要了解命令背後的模型時，請閱讀[擷取](./concepts/capture/)、[WAL](./concepts/wal/)、[canonical model](./concepts/canonical-model/) 與[重播](./concepts/replay/)。

## 文件狀態

英文是 canonical source。繁體中文與日文頁面會與目前的英文內容同步維護；每個支援的頁面都有對應翻譯。不同語系之間請保留命令名稱、格式版本與旗標不變。常用術語請參考[術語](./reference/terminology/)。
