---
title: CLI リファレンス
description: 現在の意図指向 Chronicle コマンドサーフェスとグローバルオプション。
slug: ja/0-1/docs/reference/cli
---

公開されている 0.1.x CLI には、意図指向のコマンドが 5 つあります。バイナリの正確な parser 出力を確認するには、`chronicle --help` または各コマンドの `--help` を実行してください。

## グローバルオプション

これらのオプションはサブコマンドより前に指定します。

| Option | Purpose |
| --- | --- |
| `--config FILE` | TOML 設定ファイルを読み込みます。秘密情報は環境変数を通じて参照してください。 |
| `--data-dir DIR` | 公開 data directory を選択します。 |
| `--format human\|json` | 人間向けまたは機械可読な表示を選択します。 |

## 公開コマンド

| Command | Use |
| --- | --- |
| `record` | コマンド、プロセス、cgroup をキャプチャして公開済み recording にします。 |
| `replay` | 起動したアプリケーションまたは既存のアプリケーションに対する recording の計画と安全なリプレイを行います。 |
| `list` | recording を新しい順に一覧表示します。 |
| `inspect` | `latest`、ID、完全一致の名前で recording を要約します。 |
| `doctor` | platform、capture、storage、protocol、replay-policy の probe を非破壊で実行します。 |

### record

```bash
chronicle record --name checkout -- ./my-app
chronicle record --duration 30s -- ./my-app
chronicle record --pid PID
chronicle record --cgroup /sys/fs/cgroup/my-service
chronicle record --retry checkout
```

公開フラグには `--name`、`--duration`、`--retry`、`--pid`、`--cgroup` があります。コマンド引数は `--` の後に続けます。

### replay

```bash
chronicle replay checkout -- ./my-app
chronicle replay checkout --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 --allow-read --execute
```

リプレイのフラグには `--target`、繰り返し指定できる `--allow-host`、`--allow-read`、`--allow-write`、`--execute` があります。command mode と explicit-target mode では認可要件が異なるため、操作を有効にする前に[リプレイの安全性](../../concepts/replay/)を読んでください。

### list、inspect、doctor

```bash
chronicle list
chronicle inspect latest
chronicle doctor
chronicle --format json list
chronicle --format json inspect latest
chronicle --format json doctor
```

3 つとも診断に安全に使えます。`doctor` は非破壊で、`inspect` は body と任意の header value を表示しません。

## 高度な entrypoint

非公開の `internal` namespace は、continuous recorder、recorder status、standalone ETL、決定論的 fixture recording の運用 surface です。推奨される公開 surface ではありません。Docker／Kubernetes コマンドは存在しません。
