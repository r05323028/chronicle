---
title: 用語
description: Chronicle の canonical 用語と各言語での表記。
slug: ja/0-1/docs/reference/terminology
---

英語が canonical documentation language です。各言語のページでは CLI コマンド、フラグ、schema 名、フォーマット名をそのまま保持します。

| English | Traditional Chinese (`zh-TW`) | Japanese (`ja`) | Meaning |
| --- | --- | --- | --- |
| Chronicle | Chronicle | Chronicle | 実際の挙動をリプレイ可能な回帰テスト証拠に変換するツール。 |
| capture | 擷取 | キャプチャ | 選択したワークロードについて観測する socket lifecycle と payload evidence。 |
| recorder | recorder／錄製器 | recorder／レコーダー | recording lifecycle を所有するプロセスまたはコマンド。内部サービスには `recorder` を使います。 |
| WAL | WAL／預寫式日誌 | WAL／先行書き込みログ | write-ahead log。ローカルの永続化境界。 |
| ETL | ETL | ETL | 復旧した evidence から canonical publication までの完全な Extract–Transform–Load 経路。 |
| canonical model | canonical model | canonical model | capture、ETL、storage、replay で共有する安定した session 表現であり、規範文書ではありません。 |
| canonical session | canonical session | canonical session | canonical model で表し、`inspect` と `replay` が利用するポータブルな session。 |
| session | session／工作階段 | セッション | connection、operation、integrity、replayability を含む canonical 単位。 |
| replay | 重播 | リプレイ | 認可された loopback target に対して recording を計画または実行すること。 |
| workload | 工作負載 | ワークロード | 観測または記録されるコマンド、プロセス、cgroup が表す仕事。 |
| operation / effect | 操作；effect 是安全分類，不是「效果」 | 操作；effect は安全性の分類 | 型付きの挙動と、読み取り、書き込みなどの安全性分類。 |
| authorization | 授權 | 認可 | 特定の replay 操作を許可する policy gate。 |
| storage | 儲存／storage | ストレージ | canonical artifact を保存し、atomic publication を担当する境界。 |
| fixture | fixture／測試資料 | fixture | live capture に依存しないテスト入力の source。 |
| trace | trace／追蹤記錄 | トレース | 診断や関連付けに使うイベント列。Chronicle が主に公開するのは recording と session です。 |
| live capture | live capture／即時擷取 | ライブキャプチャ | ワークロードの実行中に外部から取得するリアルタイムの evidence。 |
| bounded | 有明確上限的／受限制的 | 上限付き／制限された | duration、容量、プロトコル範囲などの明示的な上限があること。 |
| fallback | 備援路徑（fallback） | フォールバック | 主な target が使えない場合に別の target を使うこと。replay は production destination にフォールバックしません。 |
| deterministic | deterministic（結果固定且可重現） | deterministic（再現性のある） | 同じ入力と規則から予測可能で再現可能な結果を得る性質。 |
| checkpoint | checkpoint／檢查點 | チェックポイント | publication ordering と結び付いた永続化済み ETL 進捗。 |

## 使用ルール

- `WAL`、`ETL`、`HTTP/1.1`、`eBPF`、`cgroup`、`loopback`、CLI フラグは変更しません。
- ユーザー向けの上限付きキャプチャライフサイクルには **recording**、canonical replay unit には **session** を使います。
- 永続化された model には **canonical session** を使い、「normalized recording」とは書きません。
- ETL 入力には **recovered committed prefix** を使い、「WAL に入っていたもの」とは書きません。
- replay について説明するときは、モードを明記します。command mode は計画の後に、所有する監視対象 target に対する実行と読み取りを認可します。explicit-target mode は、`--execute` と必要なすべての gate が指定されるまで dry-run のままです。書き込みには常に明示的な認可が必要です。
