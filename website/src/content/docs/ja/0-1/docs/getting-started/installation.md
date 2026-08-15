---
title: インストール
description: Chronicle のリリースバイナリをインストールするか、ワークスペースをソースからビルドします。
slug: ja/0-1/docs/getting-started/installation
---

## 推奨：リリースインストーラー

リリースバイナリは Linux の `x86_64` と `aarch64`／`arm64` に対応します。インストーラーは最新の安定版を解決し、対応するアーカイブを選択し、`SHA256SUMS` を検証してからインストールします。

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
```

デフォルトの保存先は `$HOME/.local/bin` です。シェル設定は変更しません。確認：

```bash
chronicle --version
chronicle doctor
```

バージョンを固定する場合：

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_VERSION=v0.1.0 sh
```

保存先を指定する場合：

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_INSTALL_DIR=/some/path sh
```

## ソースからビルド

```bash
git clone https://github.com/r05323028/chronicle
cd chronicle
cargo build --release --locked
```

Linux ビルドにはチェックイン済みの eBPF capture オブジェクトが含まれます。その他のプラットフォームでは live capture を除く portable surface を利用できます。

:::note
Docker、Kubernetes、PostgreSQL、S3 ストレージは未実装です。
:::
