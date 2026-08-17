---
title: ストレージ
description: canonical session と payload をローカルファイルシステムへ公開する境界。
slug: ja/0-1/docs/architecture/storage
---

Chronicle は現在、recording と canonical session をローカルファイルシステムに保存します。storage は耐久性のある永続化と publication プリミティブを担当し、publication の決定、publication 検証、checkpoint advancement ordering は ETL が所有します。replay は WAL の内部ではなく永続化された canonical artifact を読み取ります。

## 公開 data directory

公開コマンドは次の順序で data directory を解決します。

1. `--data-dir DIR`；
2. 設定された `data_dir`；
3. `CHRONICLE_DATA_DIR`；
4. プラットフォームのデフォルト。

変更を伴うコマンドは必要な場合にだけ private directory を作成し、安全でない root や symlink の形式を拒否します。`doctor` は既存または予定される場所を報告しますが、probe artifact は作成しません。

```text
<data-dir>/
  .chronicle-domain.lock
  catalog.json
  recordings/<bare-recording-uuid>/
  sessions/<session-uuid>/
```

ローカルファイルシステムのデプロイ内では、正規化された `.chronicle-domain.lock` が name claim、capture、ETL、publication、catalog update を 1 つの transaction として保護します。この lock はローカルデプロイの調整メカニズムであり、Recorder と ETL の間のアーキテクチャ上の所有メカニズムではありません。

## canonical session の公開

各 session は次の形で公開されます。

```text
sessions/<session-id>/
  manifest.json
  session.json
  payloads/<sha256>
```

Unix では staging directory を `0700`、ファイルを `0600` にします。manifest を最後に書き込み、publication は存在しない destination にだけ rename します。inspect は artifact metadata を検証し、replay は payload を hydrate して SHA-256 を確認します。

## ここにないもの

PostgreSQL metadata storage、S3 互換の artifact storage、リモート WAL アーカイブ、保存時暗号化、redaction policy、tenant isolation は未実装です。storage interface や protocol registry から、これらが存在すると推測しないでください。

:::caution
ローカル artifact には本番の header、body、認証情報、個人データが含まれる可能性があります。data directory を機密として扱い、ホストレベルのアクセス制御を適用してください。
:::
