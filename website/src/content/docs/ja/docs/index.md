---
title: 概要
description: 実際のアプリケーションの挙動を、決定論的で再現可能な回帰テストの証拠に変換します。
---

Chronicle は実際のアプリケーション通信を記録し、決定論的で再現可能な回帰テストの証拠に変換します。

監視対象のコマンド、実行中のプロセス、または cgroup に外部から接続するため、アプリケーション側に計測コードを追加する必要はありません。キャプチャした証拠は解釈する前にローカルの write-ahead log（WAL）へ書き込み、プロトコルに依存しない canonical session に再構成します。リプレイは明示的に認可された loopback ターゲットに対してのみ行います。

:::caution
キャプチャした通信には認証情報や個人データが含まれる可能性があります。リプレイは副作用を伴う場合があります。Chronicle はデフォルトで dry-run とし、すべての操作を拒否します。記録された production 宛先へフォールバックすることもありません。
:::

## 現在の対応範囲

0.1.x の機能範囲は意図的に限定されています。

- Linux 上で eBPF によるライブキャプチャを行い、上限付きの平文 HTTP/1.1 通信を取得します。
- コマンド、既存プロセス、または cgroup の周辺で記録します。
- WAL 内の commit marker を備えた、セグメント化されたクラッシュ復旧可能な WAL。
- ETL により、再現可能な 1 つの canonical session をローカルファイルシステムストレージへ公開します。
- loopback 認可を備えた安全な command mode と explicit-target replay。
- すべてのプラットフォームで fixture の記録、検査、catalog の一覧表示、非破壊の readiness check。

TLS 復号、HTTP/2 以降、その他のプロトコル実装、リモート永続化、保存時暗号化、包括的なデータ秘匿化、Docker パッケージ、Kubernetes パッケージは未実装です。

## 経路

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

まず[インストール](./getting-started/installation/)を行い、[クイックスタート](./getting-started/quick-start/)へ進んでください。コマンドの背後にあるモデルを確認するには、[キャプチャ](./concepts/capture/)、[WAL](./concepts/wal/)、[canonical model](./concepts/canonical-model/)、[リプレイ](./concepts/replay/)を読んでください。

## ドキュメントの状態

英語が canonical source です。日本語と繁體中文のページは現在の英語ページと同期して保守しており、対応するすべてのページに翻訳があります。言語間でコマンド名、フォーマットバージョン、フラグは変更しないでください。用語については[用語](./reference/terminology/)を参照してください。
