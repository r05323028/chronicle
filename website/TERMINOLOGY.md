# Chronicle terminology

English is canonical. Localized pages keep command names, schema names, and format names unchanged. Translate explanatory prose, not identifiers.

| Canonical term | Traditional Chinese (`zh-TW`) | Japanese (`ja`) | Usage |
| --- | --- | --- | --- |
| capture | 捕捉 | キャプチャ | Kernel-level observation of socket lifecycle and payload evidence. Use `capture` for the action and evidence boundary. |
| recorder | recorder／錄製器 | recorder／レコーダー | A process or command that owns recording lifecycle. Keep the CLI noun `recorder` when referring to the internal service. |
| WAL | WAL／預寫式日誌 | WAL／先行書き込みログ | Write-ahead log. Keep acronym `WAL`; explain once per locale. |
| ETL | ETL | ETL | Complete Extract–Transform–Load pipeline from recovered evidence to published canonical session. |
| canonical model | 規範模型 | 正規モデル | Protocol- and storage-independent session representation. |
| replay | 重播 | リプレイ | Execute or plan recorded behavior against an authorized loopback target. |
| session | 工作階段 | セッション | Canonical recording unit containing connections, operations, integrity, and replayability. |
| checkpoint | 檢查點 | チェックポイント | Persisted ETL progress tied to publication ordering. |

Never describe planned PostgreSQL, S3, TLS decryption, HTTP/2+, Docker, Kubernetes, encryption at rest, or redaction as current features. Use “planned”, “future”, or “not implemented” only when the source docs say so.
