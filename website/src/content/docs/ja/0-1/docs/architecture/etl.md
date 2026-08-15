---
title: ETL
description: 復旧した証拠を 1 つの再現可能な canonical session に変換する方法。
slug: ja/0-1/docs/architecture/etl
---

ETL は単なる decoder ではなく、完全な Extract–Transform–Load です。復旧した WAL evidence から、session reconstruction、protocol handling、canonical validation、atomic storage publication までの経路を所有します。

## Extract（抽出）

ETL は recovery authority を通じて recording WAL を走査します。最後の有効な commit marker までの envelope だけが対象です。loss window、sequence gap、endpoint evidence、provenance を捨てずに後段へ渡します。

## Transform（変換）

session reconstruction は socket generation と方向をグループ化します。現在の HTTP/1.1 経路は、上限付きの origin-form request、正確な `Content-Length`、上限付きの chunked response、信頼できる close-delimited response、順序どおりの keep-alive exchange を処理します。socket evidence の欠落や衝突は型付きの失敗となり、操作を捏造することはありません。

protocol module は detection、decoding、correlation、canonicalization、replay、verification contract を担当します。完全な実装を持たない registry entry は scaffolding であり、対応を意味しません。

## Load（読み込み）

ETL は 1 つの canonical session を検証し、manifest、session JSON、content-addressed payload を private directory に staging し、最後に manifest を書き込み、destination を atomic に公開します。checkpoint ordering は publication に従うため、まだ公開されていない進捗を checkpoint が主張することはありません。

```text
recovered WAL prefix
       │
       ▼
socket/session reconstruction
       │
       ▼
bounded protocol decode
       │
       ▼
canonical validation
       │
       ▼
atomic filesystem publication
```

## 再起動可能性

finalization に失敗しても、永続化された WAL と checkpoint／publication state から再開できます。checkpoint は進捗の証拠であり、WAL durability の代替でも任意の出力への identity binding でもありません。矛盾する metadata は fail closed となり、診断のために残ります。
