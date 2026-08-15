---
title: 概要
description: 実際のアプリケーションの挙動を、決定的で再生可能な回帰テストの証拠に変換します。
slug: ja/0-1/docs
---

Chronicle は実際のアプリケーション通信を記録し、決定的で再生可能な回帰テストの証拠に変換します。

監視下のコマンド、実行中のプロセス、または cgroup の外側から接続でき、アプリケーションへの計測追加は不要です。捕捉した証拠は解釈より先にローカルの write-ahead log（WAL）へ書き込み、プロトコルに依存しない正規セッションへ再構成します。再生先は明示的に許可した loopback ターゲットだけです。

:::caution
捕捉した通信には認証情報や個人データが含まれる可能性があります。再生は副作用を持ち得ます。Chronicle はデフォルトで dry-run とし、すべての効果を拒否します。記録された本番宛先へは fallback しません。
:::

## 現在の対応範囲

0.1.x の範囲は意図的に限定されています。Linux の平文 HTTP/1.1 のライブキャプチャ、fixture 記録、ローカル保存、安全な loopback 再生、`doctor` による準備状態チェックが現在の表面です。

TLS 復号、HTTP/2 以降、追加プロトコル、リモート保存、保存時暗号化、包括的な redact、Docker、Kubernetes は未実装です。

## 経路

```text
アプリケーションの挙動 → eBPF capture → WAL → ETL → 正規セッション → loopback replay
```

[インストール](./getting-started/installation/)から始め、[クイックスタート](./getting-started/quick-start/)へ進んでください。
