---
title: アーキテクチャ概要
description: Chronicle のキャプチャ、永続化、変換、ストレージ、リプレイの責務を分離する境界。
slug: ja/0-1/docs/architecture/overview
---

Chronicle は 13 個の crate で構成される Rust workspace です。各 crate には主担当が 1 つあり、外側の adapter は下位層の用語ではなく application-owned contract を通じて通信します。

## 実行経路

```text
capture-ebpf → capture events → WAL → session reconstruction → ETL
                                                     ↓
                                               canonical session
                                                     ↓
                                               local storage
                                                     ↓
                                                  replay
```

application crate が use case を組み立てます。CLI は引数を解析し、application result を表示し、exit code を対応付けます。プロトコルのデコード、WAL の走査、eBPF のロード、replay policy の所有は行いません。

## 論理的なサービス境界

本番パイプラインは次の論理境界に分かれます。

- **Recorder** — キャプチャ、プロトコルに依存しない証拠、ローカル WAL の append／commit／復旧、segment と epoch の rollover、欠損の記録、将来の durable evidence shipping。プロトコルのデコードや canonical storage のレイアウトは所有しません。
- **ローカル WAL** — キャプチャの durability と復旧の権威。
- **Durable Evidence Store** — Recorder と ETL の間の不変な証拠の handoff。checksum、parent／epoch lineage、冪等な publication、独立したライフサイクルを保持します。将来の S3 互換ストアは durable な handoff／distribution 境界であり、キャプチャのホットパスでローカル WAL durability を置き換えません。
- **ETL** — reconstruction、プロトコルのデコード、canonicalization、incremental／final publication、検証、checkpoint advancement ordering。
- **Canonical Store** — inspect と replay が利用する、永続化された canonical session と payload artifact。
- **Replay** — canonical evidence を消費し、Recorder、WAL、ETL、evidence store の内部実装から独立しています。

現在のローカルデプロイでは Recorder と ETL を同一プロセスに配置できますが、正しさがプロセス、メモリ、キャプチャ所有権、ローカルファイルシステム名前空間の共有に依存することはありません。ETL は独立してデプロイ可能なままです。WAL segment、epoch、object-store object、ETL batch の境界は、プロトコルまたは論理的な相互作用の境界ではありません。

## 責務の分担

| Boundary | Responsibility |
| --- | --- |
| `chronicle-capture-ebpf` | Linux eBPF のソケットライフサイクルと payload evidence。Aya と kernel ABI は非公開。 |
| `chronicle-capture` | 正規化された capture evidence と fixture source。 |
| `chronicle-wal` | 追記専用の framing、commit authority、復旧、retention、ローカル永続化。 |
| `chronicle-session` | socket generation と evidence reconstruction。 |
| `chronicle-etl` | canonical publication と checkpoint ordering までを含む完全な Extract–Transform–Load。 |
| `chronicle-canonical` | プロトコルに依存しない session model と検証。 |
| `chronicle-storage` | ファイルシステムおよびメモリ上の session store、atomic publication。 |
| `chronicle-protocol` | Protocol SPI と registry contract。 |
| `chronicle-protocol-builtins` | 現在の HTTP/1.1 挙動を含む具体的なプロトコル実装。 |
| `chronicle-replay` | 計画、実行、検証、安全性を考慮した結果報告。 |
| `chronicle-application` | ユーザー向け use case の組み立て。 |
| `chronicle-cli` | 解析、表示、exit mapping。 |

## 信頼性の境界

WAL の commit-marker 永続性と復旧の権威、canonical schema の互換性、checkpoint ordering、replay の default-deny policy、再現可能な replay 挙動、eBPF のプライバシーは意図的な境界です。ウェブサイトの説明ではこれらを分かりやすく示し、将来の adapter がすでに動作するかのように書かないでください。

## 現在と計画中

現在のエンドツーエンド挙動は、対応する Linux 上の上限付き平文 HTTP/1.1 です。完全な detector／decoder／canonicalizer／replay／verifier 経路が実装されていない protocol registry entry は scaffolding であり、対応済みとはみなしません。PostgreSQL、MySQL/MariaDB、MongoDB、Kafka、NATS、Oracle の research entry は現在の対応範囲ではありません。

依存方向を変更する場合は、リポジトリの [crate boundary policy](https://github.com/r05323028/chronicle/blob/main/docs/architecture/crate-boundaries.md)を読んでください。
