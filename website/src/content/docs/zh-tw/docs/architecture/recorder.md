---
title: Recorder
description: 圍繞擷取與 WAL 所有權的錄製生命週期。
---

一次 recording lifecycle 會擁有一個 capture scope、一個 WAL domain 與一條 finalization path。一般使用者會從 command mode 開始：

```bash
chronicle record --name checkout -- ./my-app
```

## 指令模式生命週期

1. 解析並鎖定公開 data directory。
2. 準備 recording identity 與有明確上限的 WAL domain。
3. 在啟動受監督命令前掛接 capture source。
4. 將標準化事件納入有明確上限的 queue。
5. 將證據以 group commit 寫入 WAL，並讓遺失可見。
6. 在程序結束、收到 signal、達到可選的整個 recording deadline，或發生致命 capture／storage 失敗時停止；epoch 與實體 WAL 上限只會觸發 rollover，不會一般性終止 recording。
7. 復原具權威性的 WAL prefix。
8. 執行 ETL，為每個完成的 epoch 以 atomic 方式發佈一個不可變的 canonical session，並保留 parent／epoch provenance。
9. 只有 canonical publication 完成後，才更新 advisory catalog。

如果 recording 可以復原，finalization 失敗不代表必須重新擷取：

```bash
chronicle record --retry checkout
```

## 持續 Recorder

command、PID、cgroup 與 daemon 模式共用同一個 continuous coordinator。它負責一個 filesystem domain、有明確上限的 epoch rotation、incremental ETL／continuation resume、liveness／health metadata 與 shutdown cleanup；前一個 epoch 的 ETL 落後時，capture 仍可繼續。

這不是常駐的分散式 capture service。Recorder state、WAL、manifest、checkpoint 與 catalog fact 都維持在本機，且有明確上限。操作這條進階路徑前，請參考儲存庫的 [recorder runbook](https://github.com/r05323028/chronicle/blob/main/docs/operations/recorder-runbook.md)。目前的本機部署會把 recorder runtime 與 incremental ETL 放在同一程序中；這是拓撲選擇，不是邏輯所有權。ETL 仍是獨立邊界（參閱 [ETL](../etl/)）。一個 `.chronicle-domain.lock` 保護本機檔案系統協調網域，但不是 Recorder 與 ETL 之間的架構性所有權機制。

## 停止與復原

第一個 termination signal 會在設定的上限內 drain 並完成 finalization。強制終止或不安全的 successor capacity 失敗會保留在 recording metadata 與 WAL-loss evidence 中；單純 epoch threshold 只會要求 rollover。復原只會修復經驗證的不完整 final tail；不會隱藏完整損毀，也不會捏造 acknowledgement history。
