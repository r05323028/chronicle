---
title: 架構總覽
description: 維持 Chronicle 擷取、持久化、轉換、儲存與重播彼此分離的邊界。
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

## 邏輯服務邊界

生產管線分為下列邏輯邊界。

- **Recorder** — 擷取、與協定無關的證據、本機 WAL 的 append／commit／復原、segment 與 epoch rollover、遺失記錄、未來的 durable evidence shipping。不擁有協定解碼或 canonical storage 配置。
- **本機 WAL** — 擷取的 durable 與復原權威。
- **Durable Evidence Store** — Recorder 與 ETL 之間不可變證據的 handoff。保留 checksum、parent／epoch lineage、冪等發佈與獨立生命週期。未來的 S3 相容儲存是 durable handoff／distribution 邊界，不會在擷取熱路徑上取代本機 WAL durability。
- **ETL** — reconstruction、協定解碼、canonicalization、incremental／final publication、驗證與 checkpoint advancement ordering。
- **Canonical Store** — 供 inspect 與 replay 使用的已持久化 canonical session 與 payload artifact。
- **Replay** — 消費 canonical evidence，並與 Recorder、WAL、ETL、evidence store 的內部實作保持獨立。

目前的本機部署可以將 Recorder 與 ETL 放在同一程序中，但正確性不得依賴共用程序、記憶體、擷取所有權或本機檔案系統命名空間。ETL 仍可獨立部署。WAL segment、epoch、object-store object、ETL batch 的邊界不是協定或邏輯互動邊界。

0.1 目前的 runtime 細節：`ContinuousRecorderService` 在同一程序中組合 capture、WAL 讀取、`chronicle-etl::CommittedWalSnapshot` 處理與檔案系統 publication。Incremental pass 只處理到 soft `batch_records` 上限的完整 commit-marker range；lag 是 committed marker counter 與 checkpoint 的差值；`backoff_millis` 是不阻塞的 retry deadline。這些是目前的檔案系統 adapter，不是未來部署保證。

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
