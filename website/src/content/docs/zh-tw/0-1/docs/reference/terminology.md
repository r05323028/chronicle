---
title: 術語
description: Chronicle 的 canonical 術語與各語系寫法。
slug: zh-tw/0-1/docs/reference/terminology
---

英文是 canonical documentation language。各語系頁面會原樣保留 CLI 命令、旗標、schema 名稱與格式名稱。

| English | Traditional Chinese (`zh-TW`) | Japanese (`ja`) | Meaning |
| --- | --- | --- | --- |
| Chronicle | Chronicle | Chronicle | 將真實行為轉成可重播回歸測試證據的工具。 |
| capture | 擷取 | キャプチャ | 針對選定工作負載觀察到的 socket 生命週期與 payload 證據。 |
| recorder | recorder／錄製器 | recorder／レコーダー | 擁有錄製生命週期的程序或命令。提到內部服務時保留 `recorder`。 |
| WAL | WAL／預寫式日誌 | WAL／先行書き込みログ | write-ahead log；本機的持久性邊界。 |
| ETL | ETL | ETL | 從復原證據到 canonical publication 的完整 Extract–Transform–Load 路徑。 |
| canonical model | canonical model | canonical model | 在 capture、ETL、storage 與 replay 之間共用的穩定 session 表示法，不是規範文件。 |
| canonical session | canonical session | canonical session | 由 canonical model 表示、供 `inspect` 與 `replay` 使用的可攜 session。 |
| session | session／工作階段 | セッション | 包含 connection、operation、integrity 與 replayability 的 canonical 單位。 |
| replay | 重播 | リプレイ | 規劃或執行 recording，目標必須是已授權的 loopback。 |
| workload | 工作負載 | ワークロード | 被觀察或錄製的命令、程序或 cgroup 所代表的工作。 |
| operation / effect | 操作；effect 是安全分類，不是「效果」 | 操作；effect は安全性の分類 | 具型別的行為及其讀取、寫入或其他安全分類。 |
| authorization | 授權 | 認可 | 允許特定 replay 操作的 policy gate。 |
| storage | 儲存／storage | ストレージ | 保存 canonical artifact 並負責 atomic publication 的邊界。 |
| fixture | fixture／測試資料 | fixture | 不依賴 live capture 的測試輸入來源。 |
| trace | trace／追蹤記錄 | トレース | 可供診斷或關聯使用的事件序列；Chronicle 主要公開的是 recording 與 session。 |
| live capture | live capture／即時擷取 | ライブキャプチャ | 在工作負載執行時從外部擷取即時證據。 |
| bounded | 有明確上限的／受限制的 | 上限付き／制限された | 明確受 duration、容量或協定範圍限制的工作負載或資料。 |
| fallback | 備援路徑（fallback） | フォールバック | 主要目標不可用時改用另一個目標；replay 絕不會這樣改用 production destination。 |
| deterministic | deterministic（結果固定且可重現） | deterministic（再現性のある） | 相同輸入與規則產生可預期且可重現的結果。 |
| checkpoint | checkpoint／檢查點 | チェックポイント | 與 publication ordering 綁定的持久化 ETL 進度。 |

## 使用規則

- `WAL`、`ETL`、`HTTP/1.1`、`eBPF`、`cgroup`、`loopback` 與 CLI 旗標必須保持不變。
- 面向使用者的有明確上限的擷取生命週期使用 **recording**；canonical replay 單位使用 **session**。
- 描述持久化 model 時使用 **canonical session**，不要寫成「normalized recording」。
- 描述 ETL 輸入時使用 **recovered committed prefix**，不要寫成「WAL 裡有什麼就用什麼」。
- 討論 replay 時要說明模式：command mode 在規劃後會對所擁有的受監督 target 授予執行與讀取；explicit-target mode 在提供 `--execute` 與所有必要 gate 之前維持 dry-run；寫入一律需要明確授權。
