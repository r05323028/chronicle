---
title: CLI 參考
description: Chronicle 目前以意圖為導向的命令介面與全域選項。
---

公開的 0.1.x CLI 有五個以意圖為導向的命令。請執行 `chronicle --help` 或命令的 `--help`，查看二進位檔實際使用的 parser 輸出。

## 全域選項

這些選項要放在子命令之前：

| Option | Purpose |
| --- | --- |
| `--config FILE` | 讀取 TOML 設定檔。秘密值必須透過環境變數參照。 |
| `--data-dir DIR` | 指定公開 data directory。 |
| `--format human\|json` | 選擇人類可讀或機器可讀的輸出格式。 |

## 公開命令

| Command | Use |
| --- | --- |
| `record` | 將命令、程序或 cgroup 擷取成已發佈的 recording。 |
| `replay` | 規劃並安全地重播 recording，目標可以是新啟動或已執行中的應用程式。 |
| `list` | 依新到舊列出 recording。 |
| `inspect` | 依 `latest`、ID 或完全相符的名稱摘要 recording。 |
| `doctor` | 執行非破壞性的 platform、capture、storage、protocol 與 replay-policy probe。 |

### record

```bash
chronicle record --name checkout -- ./my-app
chronicle record --duration 30s -- ./my-app
chronicle record --pid PID
chronicle record --cgroup /sys/fs/cgroup/my-service
chronicle record --retry checkout
```

公開旗標包含 `--name`、可選的整段 recording `--duration`、`--retry`、`--pid` 與 `--cgroup`。`list` 每個穩定 parent 顯示一列，並提供有明確上限的 epoch 計數。命令引數放在 `--` 之後。

### replay

```bash
chronicle replay checkout -- ./my-app
chronicle replay checkout --epoch 0 --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 --allow-read --execute
```

Replay 旗標包含 `--target`、可重複的 `--allow-host`、`--allow-read`、`--allow-write` 與 `--execute`。command mode 與 explicit-target mode 的授權需求不同；啟用操作前請先閱讀[重播安全性](../../concepts/replay/)。

### list、inspect、doctor

```bash
chronicle list
chronicle inspect latest
chronicle doctor
chronicle --format json list
chronicle --format json inspect latest
chronicle --format json doctor
```

這三個命令都可以安全地用於診斷。`doctor` 不會修改系統；`inspect` 不會列出 body 與任意 header 值。

## 隱藏的相容性與進階 entrypoint

0.1.x 保留 continuous recorder、recorder status、internal ETL、fixture recording 與舊版 session-root 語法的隱藏相容路徑。它們會發出棄用警告，不是建議使用的公開 surface。Docker／Kubernetes 命令不存在。

要遷移隱藏的 0.1.x invocation，請使用[發行說明](https://github.com/r05323028/chronicle/blob/main/docs/release-notes.md)與儲存庫的[操作指南](https://github.com/r05323028/chronicle/blob/main/docs/operations.md)。
