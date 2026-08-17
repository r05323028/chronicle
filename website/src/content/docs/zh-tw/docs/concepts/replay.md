---
title: 重播
description: 透過明確的 loopback 安全閘門規劃並執行 recording。
---

重播是 Chronicle 風險最高的邊界。它會消費 canonical session 與 protocol interface，不會重新連線到記錄中的 production 目的地。

## 安全預設值

- Command mode 只有在完成與 target 無關的規劃後，才會對 Chronicle 擁有的受監督 listener 授予執行與讀取效果。
- Explicit-target mode 在提供 `--execute` 與所有必要的 target／effect gate 之前，維持 dry-run。
- 寫入一律需要明確授權（`--allow-write`）；驗證、發佈與未知效果維持拒絕。
- 每個 canonical connection 都需要 target mapping。
- 記錄中的 destination 永遠不是 fallback。
- incomplete、malformed、unmatched、unsupported、pipelined 或涉及有歧義遺失的操作，都會保持可見且不會嘗試執行。

## Command mode

command mode 會啟動應用程式的受監督複本，並找出一個由該 scope 擁有且唯一的 loopback listener：

```bash
chronicle replay checkout -- ./my-app
```

在 target 啟動前，規劃與拒絕檢查就會完成。command mode 可以為受監督的複本推導 loopback target 與相符的 host，但不會授予寫入、驗證、發佈或未知操作的權限。

## Explicit-target mode

對已經執行中的應用程式，請提供 loopback IP 字面值 target 與所有必要的 gates：

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```

Explicit-target mode 需要：

- 使用 `http://` 與 loopback IP 字面值；
- 重複提供且相符的 `--allow-host` 值；
- 使用 `--allow-read` 或 `--allow-write` 等操作授權；
- `--execute`。

寫入操作另外需要 `--allow-write`。設定檔不能靜默提供這些執行 gates。

## HTTP request 處理

對於有明確上限的明文 HTTP/1.1，重播會移除擷取到的 `Host`、hop-by-hop 欄位、`Authorization`、`Proxy-Authorization`、`Cookie`、forwarding header、`Expect` 與 `Transfer-Encoding`。它會送出一個 target `Host` 與重新計算的 `Content-Length`，且永遠不會追蹤 redirect。選用的授權只來自設定好的環境變數名稱，不會取自擷取到的憑證。

## 驗證

驗證會比較 status、body SHA-256／size，以及依序排列且未被忽略的 header。詳細資料不會列出 body 或任意 header 值。結果會區分 passed、failed、skipped、inconclusive 與 unsupported operation。

:::caution
不要把 replay 指向 production destination。資料庫寫入或訊息發佈可能無法復原。除非 protocol canonicalizer 已分類未知操作，或 operator 建立狹窄且明確的 policy，否則未知操作會維持拒絕。
:::
