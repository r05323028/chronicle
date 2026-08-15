---
title: インストール
description: Chronicle のリリース版バイナリをインストールするか、ワークスペースをソースからビルドします。
slug: ja/0-1/docs/getting-started/installation
---

## 推奨：リリース版インストーラー

Chronicle のリリース版バイナリは Linux の `x86_64` と `aarch64`／`arm64` に対応しています。リポジトリのインストーラーは GitHub Release の最新の安定リリースを取得し、対応するアーカイブを選択し、`SHA256SUMS` を検証してから、検証に成功した場合だけインストールします。

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
```

デフォルトのインストール先は `$HOME/.local/bin` です。スクリプトはシェル設定を変更しません。ディレクトリが `PATH` に含まれていない場合は、実行すべき `export PATH=...` の指示を表示します。

バイナリとホストを確認します。

```bash
chronicle --version
chronicle doctor
```

`doctor` は非破壊的です。プラットフォーム、アーキテクチャ、cgroup v2、BTF、組み込みキャプチャプログラム、アタッチ状態、capability、WAL／出力、プロトコル、リプレイポリシーの準備状態と、必要な対処を報告します。

### リリースを固定する

バージョンの先頭に `v` を付けても省略しても構いません。

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_VERSION=v0.1.0 sh
```

### インストール先を指定する

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_INSTALL_DIR=/some/path sh
```

## リリース版を手動でインストールする

インストーラーを実行できない場合は、次の手順を使います。

1. GitHub Release から対応する `chronicle-<version>-<target>.tar.gz` と `SHA256SUMS` をダウンロードします。
2. 展開する前にアーカイブを検証します。

   ```bash
   sha256sum -c SHA256SUMS
   ```

3. アーカイブを展開し、最上位にある `chronicle` バイナリを `PATH` に配置します。
4. `chronicle --version` と `chronicle doctor` を実行します。

リリースワークフローが公開するアーカイブは `x86_64-unknown-linux-gnu` と `aarch64-unknown-linux-gnu` です。他のプラットフォームの target 名を推測しないでください。

## ソースからビルドする

ワークスペースは `rust-toolchain.toml` で Rust ツールチェーンを固定しています。

```bash
git clone https://github.com/r05323028/chronicle
cd chronicle
cargo build --release --locked
```

Linux ビルドにはリポジトリにチェックイン済みの eBPF キャプチャオブジェクトが含まれます。その他のプラットフォームでは、ライブキャプチャを除くポータブルな機能（一覧表示、検査、リプレイの計画と検証、doctor、fixture の記録）を利用できます。

eBPF パイプラインの開発だけは、リポジトリ README に記載された別の nightly 再ビルドが必要です。ビルド後に `chronicle doctor` を実行し、現在のホストで利用できる機能を確認してください。

## ライブキャプチャのホスト要件

- Linux 6.1 以降。
- cgroup v2 が有効であること。
- `/sys/kernel/btf/vmlinux` に BTF があること。
- リトルエンディアンの `x86_64` または `aarch64`。
- 記録プロセスに `CAP_BPF` と `CAP_NET_ADMIN` があること。
- バイナリに組み込みキャプチャプログラムが含まれること。

アプリケーションは平文 HTTP/1.1 通信を公開する必要があります。現在のキャプチャ経路では TLS 暗号文を解釈できません。

:::note
Docker と Kubernetes のパッケージは未実装です。Chronicle が現在公開する artifact はローカルファイルシステム上のものです。PostgreSQL と S3 互換の永続化は今後の課題です。
:::
