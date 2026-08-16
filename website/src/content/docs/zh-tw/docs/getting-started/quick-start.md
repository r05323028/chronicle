---
title: 快速開始
description: 錄製、檢查並安全地重播有明確上限的 HTTP/1.1 工作負載。
---

這份流程使用支援的 Linux 主機與明文 HTTP/1.1 應用程式，並以 command mode 監督它。請將 `./my-app` 替換成要監督的應用程式。

## 檢查主機

```bash
chronicle doctor
```

開始錄製前，先修正回報的平台、cgroup、BTF、能力或內嵌程式問題。`doctor` 不會修改主機。

## 錄製行為

```bash
chronicle record --name checkout -- ./my-app
```

Chronicle 會先掛接擷取，再啟動應用程式。應用程式結束、按下 `Ctrl+C`，或明確的 `--duration` 到期時，錄製就會停止。錄製期間，請從另一個終端機送出具代表性的請求。

未指定 `--duration` 時，錄製會持續到應用程式結束或停止為止。WAL 的實體容量上限為 4 GiB。

:::caution
錄製期間，應用程式必須能透過非 loopback 位址連線。command mode replay 會在 loopback 上啟動受監督的複本，並拒絕將目標設為當初記錄的完全相同目的地。
:::

## 找到錄製內容

```bash
chronicle list
chronicle inspect checkout
```

可以使用 `latest`、`rec_<uuid>`、單獨的 UUID 或完全相符的名稱指定 recording。`inspect` 會摘要端點、操作、遺失警告與 replay eligibility，但不會列出擷取到的 body 或任意 header 值。

## 重播到新的複本

```bash
chronicle replay checkout -- ./my-app
```

command mode 會在啟動 target 前完成規劃，找出一個由該 scope 擁有的 loopback listener；只有通過與 target 無關的 policy checks 後才會重播。預設為 dry-run；除非明確授權相關 policy，否則寫入與其他操作都會維持拒絕。

對於已經執行中的應用程式，只能在 target 使用 loopback IP 字面值，並提供所有必要的 gates：

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```

只有在 recording 與 target 都已準備好接受寫入操作時，才加入 `--allow-write`。Chronicle 永遠不會把記錄中的 production 目的地當作 fallback。

## 檢查機器可讀的結果

所有公開命令都接受全域 format 選項：

```bash
chronicle --format json list
chronicle --format json inspect checkout
chronicle --format json replay checkout -- ./my-app
```

JSON 輸出會在有明確上限的操作完成後才產生。工具整合請使用 JSON；互動式診斷則保留人類可讀的輸出。
