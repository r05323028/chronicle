---
title: 快速開始
description: 錄製、檢查並安全地重播有界 HTTP/1.1 工作負載。
---

本流程使用支援的 Linux 主機與明文 HTTP/1.1 應用程式。先確認主機：

```bash
chronicle doctor
```

接著在受監督的指令周圍錄製：

```bash
chronicle record --name checkout -- ./my-app
```

錄製結束後查看目錄並檢查錄製：

```bash
chronicle list
chronicle inspect checkout
```

在新的受監督複本中重播：

```bash
chronicle replay checkout -- ./my-app
```

目前一次錄製預設 600 秒，最多 3600 秒；WAL 實體上限為 4 GiB。應用程式在錄製時必須透過非 loopback 位址可達。重播預設為 dry-run，且永遠不會回退到記錄的生產目的地。

明確目標模式必須使用 loopback IP、相符的 `--allow-host`、效果授權與 `--execute`。寫入另外需要 `--allow-write`：

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```
