---
title: Sessions
description: Chronicle recording 的 identity 與完整性模型。
slug: zh-tw/0-1/docs/concepts/sessions
---

Chronicle 對使用者公開的是 **recording**。recording 是有明確上限的擷取生命週期、其 WAL、metadata、catalog identity，以及已發佈的 canonical result。canonical result 是 **session**：供 `inspect` 與 `replay` 使用的可攜單位。

## Recording 識別資訊

公開參照採用穩定且適合人閱讀的形式：

- `rec_<uuid>` 是對使用者公開的 recording ID。
- `latest` 會透過 catalog 解析為最新的已發佈 recording。
- recording 唯一時，可以用 `checkout` 這類完全相符的名稱解析它。
- 直接查詢 identity 時可以使用單獨的 UUID。

catalog 只提供參考。WAL 中由復原流程認定為權威的事實，以及 canonical session 的事實，優先於互相矛盾的 catalog 資料。`chronicle record --retry RECORDING` 會重試可復原的 finalization 與 publication，不會重新擷取工作負載。

## Session 識別資訊

canonical `SessionId` 是獨立且 deterministic 的識別值，可能與 recording ID 不同。關聯會使用 source provenance；只有舊版相容路徑才會以 ID 相等作為後備。使用者應指定 recording，不要依賴內部 session ID。

## 完整性是明確定義的

操作可能是 `complete`、`incomplete`、`truncated`、`malformed`、`unmatched` 或 `unsupported`。與遺失範圍重疊的操作可能因時間範圍內的遺失而變成 incomplete。Chronicle 不會把缺少的證據靜默轉成可 replay 的操作。

`inspect` 會回報以下其中一種高階 replay 狀態：

- `fully_replayable`
- `partially_replayable`
- `not_replayable`

部分可 replay 的 session 仍可執行已證明位於遺失範圍之外的操作，同時將不安全或有歧義的操作清楚標示為 skipped。

## 本機發佈

ETL 會以 atomic 方式發佈 canonical session。manifest、session JSON 與 content-addressed payload artifact 會先寫入私有 staging directory；最後才寫入 manifest，只有 publication 完成後才會重新命名 destination。
