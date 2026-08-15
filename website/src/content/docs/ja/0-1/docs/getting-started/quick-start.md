---
title: クイックスタート
description: 上限付きの HTTP/1.1 ワークロードを記録、検査、安全にリプレイします。
slug: ja/0-1/docs/getting-started/quick-start
---

この手順では、対応する Linux ホスト上で平文 HTTP/1.1 アプリケーションを command mode で監視します。監視するアプリケーションに合わせて `./my-app` を置き換えてください。

## ホストを確認する

```bash
chronicle doctor
```

記録を始める前に、プラットフォーム、cgroup、BTF、capability、組み込みプログラムについて報告された問題を修正してください。`doctor` はホストを変更しません。

## 挙動を記録する

```bash
chronicle record --name checkout -- ./my-app
```

Chronicle は先にキャプチャをアタッチしてからアプリケーションを起動します。アプリケーションが終了したとき、`Ctrl+C` を押したとき、または期間の上限に達したときに記録を停止します。実行中は別のターミナルから代表的なリクエストを送ってください。

ワンショット記録の公開デフォルトは 600 秒、最大値は 3600 秒です。WAL 全体の物理容量上限は 4 GiB です。

:::caution
記録中、アプリケーションには loopback 以外のアドレスから到達できなければなりません。command mode replay は loopback 上で監視対象のコピーを起動し、記録された宛先そのものをターゲットにすることを拒否します。
:::

## 記録を探す

```bash
chronicle list
chronicle inspect checkout
```

記録は `latest`、`rec_<uuid>`、UUID 単体、または完全一致する名前で指定できます。`inspect` はエンドポイント、操作、loss warning、リプレイ可能性を要約しますが、キャプチャした body や任意のヘッダー値は表示しません。

## 新しいコピーへリプレイする

```bash
chronicle replay checkout -- ./my-app
```

command mode はターゲットを起動する前に計画を完了し、その scope が所有する loopback listener を 1 つ検出します。ターゲットに依存しない policy check を通過した後にだけリプレイします。デフォルトは dry-run です。該当する policy を明示的に認可しない限り、書き込みなどの操作は拒否されたままです。

すでに起動しているアプリケーションには、loopback の IP リテラルと必要なすべての gate を指定した explicit target mode だけを使用してください。

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```

書き込み操作を許可する場合に限り `--allow-write` を追加してください。Chronicle は記録された production 宛先へフォールバックしません。

## 機械可読な結果を確認する

すべての公開コマンドはグローバルな format オプションを受け付けます。

```bash
chronicle --format json list
chronicle --format json inspect checkout
chronicle --format json replay checkout -- ./my-app
```

JSON 出力は上限付きの操作が完了した後に生成されます。ツール連携には JSON を使い、対話的な診断には人間向けの出力を使ってください。
