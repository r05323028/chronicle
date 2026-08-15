---
title: リプレイ
description: 明示的な loopback 安全 gate によって recording を計画・実行します。
---

リプレイは Chronicle で最もリスクの高い境界です。canonical session と protocol interface を消費し、記録された production 宛先へ再接続することはありません。

## 安全なデフォルト

- デフォルトは dry-run です。
- 読み取り、書き込み、認証、公開、未知の操作は、policy が認可するまで拒否されます。
- すべての canonical connection に target mapping が必要です。
- 記録された宛先はフォールバックに使いません。
- incomplete、malformed、unmatched、unsupported、pipelined、または曖昧な欠損を含む操作は表示されたままになり、実行しません。

## Command mode

command mode はアプリケーションの監視対象コピーを起動し、その scope が所有する一意の loopback listener を 1 つ検出します。

```bash
chronicle replay checkout -- ./my-app
```

ターゲットを起動する前に計画と拒否チェックを完了します。command mode は監視対象コピーの loopback target と一致する host を推測できますが、書き込み、認証、公開、未知の操作を認可することはありません。

## Explicit-target mode

すでに起動しているアプリケーションには、loopback の IP リテラル target と必要なすべての gate を指定します。

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```

Explicit-target mode には次が必要です。

- `http://` と loopback IP リテラル；
- target と同じ `--allow-host` 値を繰り返し指定すること；
- `--allow-read` または `--allow-write` などの操作の認可；
- `--execute`。

書き込みには追加で `--allow-write` が必要です。設定だけでこれらの実行 gate が暗黙に付与されることはありません。

## HTTP リクエストの処理

上限付きの平文 HTTP/1.1 では、リプレイはキャプチャした `Host`、hop-by-hop field、`Authorization`、`Proxy-Authorization`、`Cookie`、forwarding header、`Expect`、`Transfer-Encoding` を削除します。target 用の `Host` を 1 つ出力し、`Content-Length` を再計算します。redirect は追跡しません。任意の認可情報は設定された環境変数名からのみ取得し、キャプチャした credential は使いません。

## 検証

検証では status、body の SHA-256／size、無視対象ではない header の順序を比較します。詳細出力に body や任意の header value を含めません。結果は passed、failed、skipped、inconclusive、unsupported operation を区別します。

:::caution
リプレイを production 宛先に向けないでください。データベースへの書き込みやメッセージ公開は取り消せない可能性があります。protocol canonicalizer が未知の操作を分類するか、operator が狭い明示的 policy を作成するまで、未知の操作は拒否されたままです。
:::
