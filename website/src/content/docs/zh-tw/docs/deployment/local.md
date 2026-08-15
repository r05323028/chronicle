---
title: 本機 Linux
description: 支援 live capture 與 replay 的部署形式。
---

Chronicle 支援的 live-capture 部署形式是本機 Linux 主機。發行版本二進位檔的 target 為 `x86_64-unknown-linux-gnu` 與 `aarch64-unknown-linux-gnu`；capture adapter 除了需要二進位檔，也需要核心與 capability 支援。

## 就緒檢查清單

```bash
chronicle doctor
```

live capture 需要：

- Linux 6.1 以上；
- cgroup v2；
- `/sys/kernel/btf/vmlinux` 提供 BTF；
- 小端序 x86_64 或 aarch64；
- 錄製程序需要 `CAP_BPF` 與 `CAP_NET_ADMIN`；
- 二進位檔包含內嵌 eBPF 程式；
- 工作負載產生有明確上限的明文 HTTP/1.1。

自動化層需要穩定的 probe data 時，請透過全域選項使用 `doctor --format json`：

```bash
chronicle --format json doctor
```

## 本機資料與容量

Recording、WAL segment、manifest、checkpoint 與 canonical payload 都會留在解析後的本機 data directory。一次性 recording 預設 600 秒，最大 3600 秒，且不能超過 4 GiB 的實體 WAL 容量上限。規劃磁碟與檔案權限時，應以擷取資料而不是二進位檔大小為準。

## 部署邊界

Chronicle 目前沒有提供 Docker 或 Kubernetes 封裝、常駐的分散式 capture plane、PostgreSQL／S3 persistence 或遠端 artifact publication。這些是未來的議題，不是 0.1.x 的部署指示。

對受監督的命令，請使用：

```bash
chronicle record --name checkout -- ./my-app
```

對既有程序或 cgroup，請使用 `--pid PID` 或 `--cgroup PATH`；Chronicle 不會終止這些工作負載。Replay 仍然只允許 loopback，且必須明確授權。
