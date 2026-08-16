---
title: レコーダー
description: キャプチャと WAL の所有権を中心とした記録のライフサイクル。
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
6. プロセス終了、signal、任意の recording 全体の deadline、または致命的な capture／storage 失敗で停止します。epoch と物理 WAL の上限は通常終了ではなく rollover を起こします。
7. 権威ある WAL prefix を復旧します。
8. ETL を実行し、完了した各 epoch に 1 つの不変な canonical session を atomic に公開し、parent／epoch provenance を保持します。
9. canonical publication の後にだけ advisory catalog を更新します。

recording が復旧可能なら、finalization の失敗時に再キャプチャする必要はありません。

```bash
chronicle record --retry checkout
```

## 継続レコーダー

command、PID、cgroup、daemon の各モードは 1 つの continuous coordinator を共有します。1 つの filesystem domain、上限付き epoch rotation、incremental ETL／continuation resume、liveness／health metadata、shutdown cleanup を所有し、先行 epoch の ETL が遅れても capture を継続できます。

これは常時稼働する分散キャプチャサービスではありません。recorder state、WAL、manifest、checkpoint、catalog fact はローカルに保持され、上限もあります。この高度な経路を運用する前に、リポジトリの [continuous recorder runbook](https://github.com/r05323028/chronicle/blob/main/docs/continuous-recorder-runbook.md)を確認してください。

## 停止と復旧

最初の termination signal では、設定された上限内で drain と finalization を行います。強制終了や安全でない successor capacity 失敗は recording metadata と WAL-loss evidence に残ります。epoch threshold だけでは終了せず rollover を要求します。復旧が修復するのは検証済みの不完全な final tail だけであり、完全な破損を隠したり acknowledgement history を作ったりはしません。
