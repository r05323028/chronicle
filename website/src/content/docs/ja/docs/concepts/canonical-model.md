---
title: canonical model
description: "`inspect` と `replay` が利用する、プロトコルとストレージに依存しない session 表現。"
---

canonical session は、キャプチャとリプレイの間で Chronicle が受け渡す表現です。eBPF hook、WAL framing、特定の storage backend ではなく、アプリケーションの挙動を安定したモデルで表します。

## 含まれるもの

現在の v1 model には次が含まれます。

- recording、session、connection、operation の強い identity；
- socket evidence から導出した canonical client/server endpoint；
- 相対 nanosecond offset による deterministic な operation timeline；
- 型付きの operation kind と effect 分類；
- request と recorded-response payload reference；
- connection と operation の completeness map；
- source provenance、loss window、integrity、WAL checkpoint の情報；
- replay attribute と明示的な blocker；
- core field と分離して保持する protocol extension bytes。

timeline が唯一の operation order です。completeness map が権威になります。endpoint evidence が欠落または衝突した場合、ETL は失敗し、捏造した `unknown:0` endpoint を保存することはありません。

## 1 つの可変 v1 契約

現在の canonical schema は `CANONICAL_SCHEMA_VERSION = 1` です。reader は他のバージョンを拒否します。明示的な compatibility freeze の前に、Chronicle が過去の migration reader を維持することはありません。将来バージョンを変更する場合は、別の design change で互換性と migration policy を定義する必要があります。

## ファイルシステム artifact

ローカル store は次を公開します。

```text
sessions/<session-id>/
  manifest.json
  session.json
  payloads/<sha256>
```

manifest には identity、canonical version、checksum、payload の数とサイズ、WAL checkpoint、上限付きの issue、completeness、replay blocker が記録されます。Unix ではファイルに private mode を設定します。replay は payload を hydrate し、送信前に SHA-256 を確認します。

## ポータビリティの境界

replay が消費するのは canonical session と protocol interface です。capture、eBPF、WAL、ETL、元の storage implementation には依存しません。この分離により、同じ replay core で fixture から作った session と production から作った session の両方を扱えます。
