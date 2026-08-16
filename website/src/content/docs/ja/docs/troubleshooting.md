---
title: トラブルシューティング
description: readiness、キャプチャ、finalization、リプレイの失敗を推測せずに診断します。
---

まず非破壊の readiness report を確認します。

```bash
chronicle doctor
chronicle --format json doctor
```

ホストを変更する前に、probe code と対処方法を読んでください。

## ライブキャプチャを利用できない

次を確認します。

- ホストが Linux 6.1 以降であること；
- cgroup v2 がマウントされ、選択したワークロードが意図した subtree にあること；
- `/sys/kernel/btf/vmlinux` が存在すること；
- バイナリに capture object とプログラムが含まれること；
- 記録プロセスに `CAP_BPF` と `CAP_NET_ADMIN` があること；
- アーキテクチャがリトルエンディアンの x86_64 または aarch64 であること。

非 Linux ビルドでも fixture の記録、一覧表示、検査、リプレイの計画と検証、doctor は利用できます。ライブ eBPF キャプチャは利用できません。

## 操作が表示されない

現在の decoder が対応するのは上限付きの平文 HTTP/1.1 だけです。TLS 暗号文、HTTP/2 以降、upgrade、pipelining、chunked request、未対応プロトコルの通信は、リプレイ可能な HTTP operation になりません。記録中にワークロードへ loopback 以外のアドレスから到達できたこと、通信が記録期間内に到着したことを確認してください。

## Finalization が停止した、または WAL が上限に近い

公開 recording には暗黙の時間制限はありません。境界付きの epoch WAL（漯認で物理上限 4 GiB）は recording を終了させずに rollover します。ディスク容量と recording directory を確認してください。recording が復旧可能なら、再キャプチャせずに finalization を再試行します。

```bash
chronicle record --retry checkout
```

復旧が recording を診断している間は、segment や manifest を削除しないでください。完全な破損、identity の不一致、sequence gap、無効な commit reference は fail closed になります。

## リプレイが拒否される

dry-run と拒否は期待されるデフォルトです。次を確認します。

- すべての connection に target mapping があること；
- command mode が所有する一意の loopback listener を 1 つ検出できること；
- explicit target が `http://` の loopback IP リテラルであること；
- `--allow-host` が target host と完全に一致すること；
- `--allow-read` または `--allow-write` が意図した操作を認可すること；
- explicit-target execution に `--execute` があること。

書き込み、認証、公開、未知の操作は、明示的に対応して認可されない限り拒否されます。記録された production 宛先へはフォールバックしません。

## データが欠けているように見える

Chronicle は時間範囲の loss window と completeness state を保持します。曖昧な欠損と重なる操作は incomplete、truncated、unmatched、not replayable のいずれかになる可能性があります。inspect は loss warning と replay eligibility を報告しますが、欠けた endpoint や body を捏造しません。

## Artifact に機密データが含まれる

WAL と payload file にはキャプチャした credential、header、body、個人データが含まれる可能性があります。data directory を機密として扱い、ファイル権限を使い、独立した確認なしに artifact を共有しないでください。Chronicle は現在、保存時暗号化や包括的なデータ秘匿化を保証しません。
