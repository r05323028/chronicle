---
title: WAL
description: Chronicle が変換前にキャプチャした証拠を永続化する理由。
---

Chronicle はキャプチャした証拠をセグメント化された追記専用の先行書き込みログへ書き込んでから、ETL に解釈させます。WAL はローカルの永続化境界であり、下流処理が消費できる範囲を決める復旧の権威です。

## Commit marker が永続化済みの範囲を定義する

各 group は data frame を追加し、WAL 内に `CommitMarker` を 1 つ追加し、flush を行ってから 1 回の `fdatasync` を実行します。acknowledgement は sync の成功後にだけ記録されます。最後の有効な marker が永続化済みの prefix を証明し、外部の watermark ファイルで置き換えることはありません。

```text
segment 00000000000000000000.chwal
  capture event
  capture event
  commit marker  ← durable prefix
  capture event  ← complete but uncommitted suffix
```

ETL が読むのは、復旧の権威が認めた commit 済み prefix だけです。最後の有効な marker の後にある完全な frame は未 commit の suffix として残りますが、canonical evidence にはなりません。

## 復旧の挙動

復旧では segment header、envelope version、recording identity、sequence continuity、frame checksum、marker reference、累積値、marker digest を検証します。検証後に修復できるのは、最後の不完全な frame または最後の marker の末尾だけです。完全な破損、identity の不一致、無効な reference、未対応バージョン、sequence gap は fail closed になります。

記録を再オープンすると、最後の権威ある marker の後から再開し、報告された未 commit suffix だけを削除して sequence continuity を保ちます。バイト列だけから、呼び出し側が acknowledgement を観測したとは推測しません。

## 物理容量の上限

セグメントサイズは 16 MiB 以上 4 GiB 以下です。epoch 内では、`segments/` 配下の合計バイト数はデフォルトで 4 GiB、4 GiB を超えることはありません。親 recording には合計上限はありません。writer は書き込む前に完全な data frame と最後の marker のための容量を予約します。ローテーションでは次の header と一時的な publication peak も予約します。

キューへの取り込み失敗と WAL limit による失敗は、型付きの evidence として残ります。キャプチャ成功として隠すことはありません。

:::caution
WAL と payload artifact には、本番の header、body、認証情報、個人データが含まれる可能性があります。private file mode は保護策ですが、保存時暗号化、包括的なデータ秘匿化、tenant isolation を意味しません。
:::

現在の v1 framing contract については、リポジトリの [WAL フォーマット](https://github.com/r05323028/chronicle/blob/main/docs/wal-format.md)を参照してください。
