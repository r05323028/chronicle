---
title: クイックスタート
description: 限定された HTTP/1.1 ワークロードを記録、検査、安全に再生します。
---

対応する Linux ホストと平文 HTTP/1.1 アプリケーションを使います。まず準備状態を確認します。

```bash
chronicle doctor
```

監視下のコマンドを記録します。

```bash
chronicle record --name checkout -- ./my-app
```

記録を一覧し、検査します。

```bash
chronicle list
chronicle inspect checkout
```

新しい監視下のコピーへ再生します。

```bash
chronicle replay checkout -- ./my-app
```

ワンショット記録のデフォルトは 600 秒、最大 3600 秒、WAL の物理上限は 4 GiB です。再生は dry-run がデフォルトで、記録された本番宛先へ fallback しません。明示的なターゲットには loopback、`--allow-host`、効果の許可、`--execute` が必要です。
