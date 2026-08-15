---
title: 架構總覽
description: 維持 Chronicle 擷取、持久化、轉換、儲存與重播彼此分離的邊界。
slug: zh-tw/0-1/docs/architecture/overview
---

Chronicle 是由十三個 crate 組成的 Rust workspace。每個 crate 都有一個主要 owner；外層 adapter 透過 application-owned contract 互動，而不是直接使用下層術語。

## 執行流程

```text
capture-ebpf → capture events → WAL → session reconstruction → ETL
                                                     ↓
                                               canonical session
                                                     ↓
                                               local storage
                                                     ↓
                                                  replay
```

Application crate 會組合各個 use case。CLI 會解析引數、呈現 application result 並對應 exit code；它不會解碼協定、掃描 WAL、載入 eBPF，也不擁有 replay policy。

## 責任歸屬

| Boundary | Responsibility |
| --- | --- |
| `chronicle-capture-ebpf` | Linux eBPF socket 生命週期與 payload evidence；Aya 與 kernel ABI 保持私有。 |
| `chronicle-capture` | 標準化的擷取證據與 fixture source。 |
| `chronicle-wal` | 只能追加的 framing、commit authority、復原、retention 與本機持久性。 |
| `chronicle-session` | Socket generation 與 evidence reconstruction。 |
| `chronicle-etl` | 完整的 Extract–Transform–Load，包含 canonical publication 與 checkpoint ordering。 |
| `chronicle-canonical` | 不依賴協定的 session model 與驗證。 |
| `chronicle-storage` | 檔案系統與記憶體 session store；atomic publication。 |
| `chronicle-protocol` | Protocol SPI 與 registry contract。 |
| `chronicle-protocol-builtins` | 具體的協定實作，包含目前的 HTTP/1.1 行為。 |
| `chronicle-replay` | 規劃、執行、驗證，以及考量安全性的結果回報。 |
| `chronicle-application` | 面向使用者的 use-case 組合。 |
| `chronicle-cli` | 解析、呈現與 exit mapping。 |

## 可靠性邊界

WAL commit-marker 持久性與復原權威、canonical schema 相容性、checkpoint ordering、replay default-deny policy、deterministic replay 行為與 eBPF 隱私，都是刻意維持的邊界。網站說明應讓這些邊界容易理解，不應暗示未來的 adapter 已經可用。

## 目前與規劃中

目前端到端行為是支援 Linux 上有明確上限的明文 HTTP/1.1。除非完整實作 detector／decoder／canonicalizer／replay／verifier 路徑，否則 protocol registry entry 只是 extension scaffolding，不代表支援。PostgreSQL、MySQL/MariaDB、MongoDB、Kafka、NATS 與 Oracle research entry 都不是目前支援的功能。

若要變更依賴方向，請閱讀儲存庫的 [crate boundary policy](https://github.com/r05323028/chronicle/blob/main/docs/architecture/crate-boundaries.md)。
