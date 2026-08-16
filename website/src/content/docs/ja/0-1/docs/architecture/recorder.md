---
title: レコーダー
description: キャプチャと WAL の所有権を中心とした記録のライフサイクル。
slug: ja/0-1/docs/architecture/recorder
---

recording lifecycle は 1 つの capture scope、1 つの WAL domain、1 つの finalization path を所有します。通常のユーザーは command mode から始めます。

```bash
chronicle record --name checkout -- ./my-app
```

## コマンドモードのライフサイクル

1. 公開 data directory を解決してロックします。
2. recording identity と上限付きの WAL domain を準備します。
3. 監視対象のコマンドを起動する前に capture source をアタッチします。
4. 正規化されたイベントを上限付きの queue に取り込みます。
5. group commit で evidence を WAL に書き込み、欠損を可視化します。
6. プロセス終了、signal、期間の上限、物理 WAL limit のいずれかで停止します。
7. 権威ある WAL prefix を復旧します。
8. ETL を実行し、canonical session を atomic に公開します。
9. canonical publication の後にだけ advisory catalog を更新します。

recording が復旧可能なら、finalization の失敗時に再キャプチャする必要はありません。

```bash
chronicle record --retry checkout
```

## 継続レコーダー

リポジトリには対応するデプロイ向けの上限付き continuous recorder も含まれています。意図指向の公開 CLI surface が安定するまで、foreground entrypoint は非公開のままです。1 つの filesystem domain、epoch rotation、incremental ETL resume、liveness／health metadata、shutdown cleanup を所有します。

これは常時稼働する分散キャプチャサービスではありません。recorder state、WAL、manifest、checkpoint、catalog fact はローカルに保持され、上限もあります。この高度な経路を運用する前に、リポジトリの [continuous recorder runbook](https://github.com/r05323028/chronicle/blob/main/docs/continuous-recorder-runbook.md)を確認してください。

## 停止と復旧

最初の termination signal では、設定された上限内で drain と finalization を行います。強制終了や容量制限は recording metadata と WAL-loss evidence に残ります。復旧が修復するのは検証済みの不完全な final tail だけであり、完全な破損を隠したり acknowledgement history を作ったりはしません。
