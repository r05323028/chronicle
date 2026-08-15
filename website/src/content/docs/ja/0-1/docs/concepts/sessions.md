---
title: セッション
description: Chronicle の recording における identity と完全性のモデル。
slug: ja/0-1/docs/concepts/sessions
---

Chronicle がユーザーに公開するのは **recording** です。recording は上限付きのキャプチャライフサイクル、その WAL、metadata、catalog identity、公開済みの canonical result から成ります。canonical result は **session**、つまり `inspect` と `replay` が利用するポータブルな単位です。

## recording の識別情報

公開参照は安定した、人間が扱いやすい形式です。

- `rec_<uuid>` はユーザー向けの recording ID です。
- `latest` は catalog を通じて、最新の公開済み recording に解決されます。
- `checkout` のような完全一致の名前は、recording が一意の場合に解決できます。
- identity を直接検索する場合は UUID 単体も使えます。

catalog は参考情報です。復旧の権威が持つ WAL の事実と canonical session の事実が、矛盾する catalog の情報より優先されます。`chronicle record --retry RECORDING` は再キャプチャせず、復旧可能な finalization と publication を再試行します。

## session の識別情報

canonical `SessionId` は recording ID とは独立して再現可能に生成されるため、両者は異なる場合があります。関連付けには source provenance を使い、ID の一致は旧来のフォールバックに限ります。ユーザーは内部の session ID に依存せず、recording を指定してください。

## 完全性は明示される

操作は `complete`、`incomplete`、`truncated`、`malformed`、`unmatched`、`unsupported` のいずれかになります。loss window と重なる操作は、時間範囲内の欠損によって incomplete になる場合があります。欠けた証拠を、黙ってリプレイ可能な操作へ変換することはありません。

`inspect` は次の高レベルなリプレイ状態のいずれかを報告します。

- `fully_replayable`
- `partially_replayable`
- `not_replayable`

部分的にリプレイ可能な session では、loss window の外側で証明できる操作だけを実行し、安全でない、または曖昧な操作は skipped として表示できます。

## ローカル公開

ETL は canonical session を atomic に公開します。manifest、session JSON、content-addressed payload artifact は private staging directory に書き込み、最後に manifest を書きます。公開が完了した場合にだけ destination を rename します。
