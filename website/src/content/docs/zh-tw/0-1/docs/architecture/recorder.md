---
title: Recorder
description: 圍繞擷取與 WAL 所有權的錄製生命週期。
slug: zh-tw/0-1/docs/architecture/recorder
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
6. 在程序結束、收到 signal、達到 duration limit 或實體 WAL limit 時停止。
7. 復原具權威性的 WAL prefix。
8. 執行 ETL，並以 atomic 方式發佈 canonical session。
9. 只有 canonical publication 完成後，才更新 advisory catalog。

如果 recording 可以復原，finalization 失敗不代表必須重新擷取：

```bash
chronicle record --retry checkout
```

## 持續 Recorder

儲存庫也包含適用於支援部署的、有明確上限的 continuous recorder。當面向意圖的公開 CLI surface 尚在穩定時，它的 foreground entrypoint 仍保持隱藏。它負責一個 filesystem domain、epoch rotation、incremental ETL resume、liveness／health metadata 與 shutdown cleanup。

這不是常駐的分散式 capture service。Recorder state、WAL、manifest、checkpoint 與 catalog fact 都維持在本機，且有明確上限。操作這條進階路徑前，請參考儲存庫的 [continuous recorder runbook](https://github.com/r05323028/chronicle/blob/main/docs/continuous-recorder-runbook.md)。

## 停止與復原

第一個 termination signal 會在設定的上限內 drain 並完成 finalization。強制終止或容量上限會保留在 recording metadata 與 WAL-loss evidence 中。復原只會修復經驗證的不完整 final tail；不會隱藏完整損毀，也不會捏造 acknowledgement history。
