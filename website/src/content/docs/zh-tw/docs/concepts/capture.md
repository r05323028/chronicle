---
title: 擷取
description: Chronicle 如何在不修改應用程式的情況下觀察應用程式行為。
---

擷取是證據邊界。它會觀察所選工作負載的 socket 生命週期事件與依序排列的 payload 片段；它不會把這些事件直接當成應用程式操作。

## 選取工作負載

公開的 `record` 命令支援三種 scope：

```bash
chronicle record -- ./my-app
chronicle record --pid 12345
chronicle record --cgroup /sys/fs/cgroup/my-service
```

command mode 會監督該命令。PID 與 cgroup mode 會觀察既有工作負載，不會終止它們。使用 `--name` 設定穩定名稱，使用 `--duration` 設定可選的整段 recording deadline。省略時會持續到來源完成、明確停止或安全性致命失敗。擷取會在不重新連接來源的情況下輪替有明確上限的 epoch。

## 哪些資料會跨過邊界

Linux adapter 會將 Aya 與 kernel ABI 細節封裝在內部。應用程式層收到的是標準化的擷取事件，包含 socket identity、端點證據、方向與 payload 片段。在解讀沒有端點資訊的 payload 片段之前，系統會先取得端點以及 active/passive role 的證據。

目前只有明文 TCP payload 對 HTTP/1.1 decoder 有用。TLS 密文仍然無法解讀。擷取遺失會以帶有時間範圍的遺失證據表示；Chronicle 不會跨越有歧義的遺失範圍，硬湊出完整操作。

## 為什麼不需要 instrumentation？

Chronicle 會從命令、程序或 cgroup 外部掛接。應用程式不必加入 Chronicle SDK 呼叫、修改程式、以特殊模式重新啟動，也不需要協定專用的測試 hook。這能讓 production 整合保持精簡，同時由標準化的證據邊界讓 fixture 與 eBPF 擷取共用相同的下游 pipeline。

## 上限是契約的一部分

整段 recording 的 duration 可省略；`10m`、`24h` 等經檢查的值會設定 deadline，且不會在 epoch 輪替時重設。每個 epoch 仍有有明確上限的 WAL/segment 上限，parent 沒有總 WAL 上限。擷取佇列仍有明確上限；無法納入的證據會成為可見遺失，而不是被靜默丟棄。

接著閱讀 [WAL](../wal/)、[ETL](../../architecture/etl/) 與[本機部署](../../deployment/local/)，了解後續邊界。
