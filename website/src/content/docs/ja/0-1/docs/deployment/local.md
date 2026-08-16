---
title: ローカル Linux
description: ライブキャプチャとリプレイに対応するデプロイ形態。
slug: ja/0-1/docs/deployment/local
---

Chronicle がライブキャプチャで対応するデプロイ形態はローカル Linux ホストです。リリース版バイナリの target は `x86_64-unknown-linux-gnu` と `aarch64-unknown-linux-gnu` です。キャプチャアダプターには、バイナリに加えて kernel と capability のサポートが必要です。

## 準備状態チェックリスト

```bash
chronicle doctor
```

ライブキャプチャには次が必要です。

- Linux 6.1 以上；
- cgroup v2；
- `/sys/kernel/btf/vmlinux` に BTF があること；
- リトルエンディアンの x86_64 または aarch64；
- 記録プロセスの `CAP_BPF` と `CAP_NET_ADMIN`；
- バイナリに組み込まれた eBPF プログラム；
- 上限付きの平文 HTTP/1.1 を生成するワークロード。

自動化層で安定した probe data が必要な場合は、グローバルオプションを使って `doctor --format json` を実行します。

```bash
chronicle --format json doctor
```

## ローカルデータと容量

recording、WAL segment、manifest、checkpoint、canonical payload は、解決されたローカル data directory に残ります。recording は終了または明示的な停止まで続き、物理 WAL 容量は 4 GiB を超えません。ディスク容量とファイル権限は、バイナリサイズだけでなくキャプチャしたデータを基準に計画してください。

## デプロイの境界

Chronicle は現在、Docker／Kubernetes パッケージ、常時稼働する分散キャプチャプレーン、PostgreSQL／S3 永続化、リモート artifact 公開を提供していません。これらは将来の課題であり、0.1.x のデプロイ手順ではありません。

監視対象のコマンドには次を使います。

```bash
chronicle record --name checkout -- ./my-app
```

既存プロセスまたは cgroup には `--pid PID` または `--cgroup PATH` を使います。Chronicle はこれらのワークロードを終了させません。リプレイは引き続き loopback のみに限定され、明示的な認可が必要です。
