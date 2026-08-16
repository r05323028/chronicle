---
title: 疑難排解
description: 診斷就緒、擷取、finalization 與 replay 失敗，不靠猜測。
slug: zh-tw/0-1/docs/troubleshooting
---

先查看非破壞性的就緒報告：

```bash
chronicle doctor
chronicle --format json doctor
```

變更主機前，先閱讀 probe code 與修正建議。

## 無法使用 live capture

請檢查：

- 主機是 Linux 6.1 以上；
- 已掛載 cgroup v2，且選定的工作負載位於預期的 subtree；
- `/sys/kernel/btf/vmlinux` 存在；
- 二進位檔包含 capture object 與程式；
- 錄製程序具有 `CAP_BPF` 與 `CAP_NET_ADMIN`；
- 架構是小端序 x86_64 或 aarch64。

非 Linux 建置仍支援 fixture 錄製、列出、檢查、replay 規劃與驗證，以及 doctor；但不提供 live eBPF capture。

## 沒有出現任何操作

目前 decoder 只支援有明確上限的明文 HTTP/1.1。TLS 密文、HTTP/2 以上版本、upgrade、pipelining、chunked request 與不支援的協定流量，不會成為可 replay 的 HTTP operation。確認錄製期間工作負載能透過非 loopback 位址連線，且流量在錄製時間範圍內抵達。

## Finalization 停止，或 WAL 接近容量上限

錄製受 duration 與 4 GiB WAL 實體容量上限限制。檢查磁碟空間與 recording directory。如果 recording 可以復原，請在不重新擷取的情況下重試 finalization：

```bash
chronicle record --retry checkout
```

復原正在診斷 recording 時，不要刪除 segment 或 manifest。完整損毀、identity 不符、sequence gap 與無效的 commit reference 都會 fail closed。

## Replay 被拒絕

Dry-run 與拒絕是預期的預設值。請檢查：

- 每個 connection 都有 target mapping；
- command mode 能找出一個由它擁有且唯一的 loopback listener；
- explicit target 是 `http://` loopback IP 字面值；
- `--allow-host` 與 target host 完全相符；
- `--allow-read` 或 `--allow-write` 授權預期的操作；
- explicit-target execution 具有 `--execute`。

寫入、驗證、發佈與未知操作，除非明確支援並授權，否則都會維持拒絕。記錄中的 production destination 永遠不是 fallback。

## 資料看起來遺失

Chronicle 會保留時間範圍內的 loss window 與 completeness state。與有歧義的遺失重疊的操作，可能是 incomplete、truncated、unmatched 或 not replayable。Inspect 會回報遺失警告與 replay eligibility；它不會捏造缺少的 endpoint 或 body。

## Artifact 包含敏感資料

WAL 與 payload 檔案可能包含擷取到的憑證、header、body 與個人資料。請將 data directory 視為敏感資料、使用檔案系統權限，並在獨立審查前不要分享 artifact。Chronicle 目前不保證靜態資料加密或完整資料遮罩。
