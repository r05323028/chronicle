---
title: ローカル Linux
description: ライブキャプチャとリプレイに対応するデプロイ形態。
---

Chronicle のリリース版ライブキャプチャのデプロイ形態はローカル Linux ホストです。リリース版バイナリの target は `x86_64-unknown-linux-gnu` と `aarch64-unknown-linux-gnu` です。0.1 でリリース検証済みのランタイム環境は Ubuntu 24.04／Linux 6.8／aarch64 です。x86_64 とその他の Linux 6.1 以降の環境には対応する特権アクセプタンスが必要で、バイナリのビルドはランタイム対応の証明になりません。

## 準備状態チェックリスト

```bash
chronicle doctor
```

ライブキャプチャには次が必要です。

- リリース版 target は x86_64-unknown-linux-gnu と aarch64-unknown-linux-gnu；
- リリース検証済みランタイム：Ubuntu 24.04／Linux 6.8／aarch64；
- その他の Linux 6.1 以降のカーネルと x86_64 には対応する特権アクセプタンスが必要；
- cgroup v2、`/sys/kernel/btf/vmlinux` の BTF、必要な capability；
- 記録プロセスの `CAP_BPF` と `CAP_NET_ADMIN`；
- バイナリに組み込まれた eBPF プログラム；
- 上限付きの平文 HTTP/1.1 を生成するワークロード。

自動化層で安定した probe data が必要な場合は、グローバルオプションを使って `doctor --format json` を実行します。

```bash
chronicle --format json doctor
```

## ローカルデータと容量

recording、WAL segment、manifest、checkpoint、canonical payload は、解決されたローカル data directory に残ります。`--duration` を省略すると recording 全体の時間 deadline はなく、明示した deadline は上限付き epoch／segment WAL 容量とは独立です。1 つの parent recording は複数の epoch を持て、全体 WAL 上限はありません。ディスク容量とファイル権限は、バイナリサイズだけでなくキャプチャしたデータを基準に計画してください。

## デプロイの境界

Chronicle は現在、Docker／Kubernetes パッケージ、常時稼働する分散キャプチャプレーン、PostgreSQL／S3 永続化、リモート artifact 公開を提供していません。これらは将来の課題であり、0.1.x のデプロイ手順ではありません。

監視対象のコマンドには次を使います。

```bash
chronicle record --name checkout -- ./my-app
```

既存プロセスまたは cgroup には `--pid PID` または `--cgroup PATH` を使います。Chronicle はこれらのワークロードを終了させません。リプレイは引き続き loopback のみに限定され、明示的な認可が必要です。
