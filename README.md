# beep-orchestra

Linux の PC スピーカー (`beep` コマンド) で音楽を鳴らすツール群。
単体マシンでは MIDI をモノフォニック再生し、複数マシンを LAN で絶対時刻同期させて
各ノードに 1 声部ずつ担当させることで和音を再現する **「beep オーケストラ」** モードも
サポートする。GUI は無く、CLI / curl / 内蔵 Web UI で操作する。

## 構成バイナリ

| バイナリ | 役割 |
|---|---|
| `beepd` | HTTP API + LAN 同期を担うデーモン。各マシンで常駐。Web UI も内蔵 |
| `beepctl` | 複数ノードを一括操作する CLI (マイコン OTA 的な遠隔更新も含む) |
| `beep-player` | MIDI を単体マシンでモノフォニック再生する CLI |
| `beep-convert` | MIDI の中身確認 / JSON ダンプ / voice 分割プレビュー / `beep -f ... -l ...` シェルスクリプトから SMF への変換 |

## ビルド

```bash
cargo build --release
# 生成物: target/release/{beepd,beepctl,beep-player,beep-convert}
```

Rust 1.75+ 必須。

## 必要パッケージ (再生ノード側)

```bash
sudo apt install beep        # Debian/Ubuntu 系
```

PC スピーカー (`pcspkr` カーネルモジュール) が読み込まれていること。
`blacklist pcspkr` されていると鳴らないので `/etc/modprobe.d/` 配下を確認する。

`beep` を root 権限なしで叩けるようにするには `sudo chmod u+s /usr/bin/beep` が最も簡単
(evdev バックエンド経由にする方法もあるが、サーバ室運用なら setuid で十分)。

## 単体再生 (1 台で `beep-player`)

```bash
# 素直に再生 (同時発音は最高音を採用)
beep-player song.mid

# 最低音を追う (ベースライン抽出)
beep-player --policy lowest song.mid

# 実際には鳴らさず、鳴らす予定を dry-run でログに
beep-player --dry-run song.mid
```

## 複数台でオーケストラ再生

複数ノードを立てて LAN 時刻同期させ、1 ノード = 1 声部で演奏する。
以下は 3 台構成 (`nodeA` / `nodeB` / `nodeC`) の例。
再生開始は任意の 1 台 (「マスター」) に対して投げると、マスターが各ノードへ声部を配布する。

### 準備

全ノードで `beepd` を起動しておく。LAN discovery が無効なサブネットや WireGuard/Tailscale
越しに繋ぎたい場合は、`static_peers` に相手の HTTP エンドポイントを明示する
(詳細は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#static_peers-別サブネットwireguard-越し))。

```bash
# ノードが見えているか確認 (任意の 1 台を叩けば self + 発見済み peer が返る)
beepctl nodes nodeA:8642
```

### 再生

```bash
# 発見済みの全ノードで自動割当 (self を含む、名前昇順)
beepctl orchestra song.mid nodeA:8642 --lead 3

# どのノードにどの声部を鳴らすか明示指定:
#   voice 0 (最高音=メロディ) → nodeB
#   voice 1                    → nodeC
#   voice 2 (最低音)           → nodeA
beepctl orchestra song.mid nodeA:8642 --nodes nodeB,nodeC,nodeA --lead 3

# DAW で「Track1=メロディ Track2=ハーモニー Track3=ベース」のように分けた MIDI を、
# SMF トラックの意図通り並べる
beepctl orchestra song.mid nodeA:8642 \
    --nodes nodeB,nodeC,nodeA --voicing track --lead 3

# マスター自身は演奏に参加させず、指定ノードだけで鳴らす
beepctl orchestra song.mid nodeA:8642 --nodes nodeB,nodeC
```

**`--nodes` は node_name または UUID のどちらでも解決される**。未知の名前は
`HTTP 400 unknown node in node_order: 'foo' (candidates: ...)` で返る。
`--nodes` を明示した場合は `--include-self` は無視され、そのリストの順序が
そのまま voice 0, 1, 2 に対応する。

### 状態確認 / 停止 / 同期再生

```bash
# 全ノードの現在状態
beepctl status nodeA:8642 nodeB:8642 nodeC:8642

# 全ノード停止 (演奏中の割り込みも可)
beepctl stop nodeA:8642 nodeB:8642 nodeC:8642

# 同じ MIDI を全ノードで同時再生 (start_at = now + 3s)
beepctl play song.mid --start-in 3 nodeA:8642 nodeB:8642 nodeC:8642
```

## Web UI (任意の `beepd` に組み込み済み)

ブラウザで `http://<ホスト>:8642/` (または `/ui`) を開くと、追加インストール無しで
MIDI アップロード・ノード並べ替え・再生/停止ができる。

- 発見済みノード一覧、参加チェック、ドラッグで voice 順並べ替え
- `.mid` のドラッグ&ドロップ
- `lead time` / `voicing` (pitch | track) 設定
- 再生 / 停止 (全ノード一括) / 一時停止 / 再開
- 再生ステータスを 0.5 秒毎に polling して進捗バー表示

単一 HTML (`include_str!` でバイナリ埋め込み) で外部リソース依存なし。Firefox / Chromium /
Safari / モバイル Safari で動作確認済み (モバイルはページスクロールが優先されるので、
並べ替えはデスクトップ推奨)。

## HTTP API 直叩き (curl)

```bash
# ノード一覧
curl -s http://<master>:8642/nodes | jq

# 現在の再生状態
curl -s http://<master>:8642/status | jq

# オーケストラ再生 (マスターに投げると全ノードへ分配される)
curl -X POST http://<master>:8642/orchestra/play \
     -F file=@song.mid \
     -F lead_time_sec=3 \
     -F node_order=nodeB,nodeC,nodeA \
     -F voicing=pitch

# 全ノード停止 (マスターが peer に /stop を代理配信)
curl -X POST http://<master>:8642/stop_all

# 単体再生 / 一時停止 / 再開
curl -X POST http://<master>:8642/play -F file=@song.mid -F policy=highest
curl -X POST http://<master>:8642/pause
curl -X POST http://<master>:8642/resume
```

## 設定ファイル (`beepd.toml`)

主要パラメータ (全フィールドの一覧は [docs/CONFIG.md](docs/CONFIG.md) 参照):

```toml
node_name = "nodeA"              # このノードの表示名
listen_addr = "0.0.0.0"
listen_port = 8642
priority_algorithm = "highest"   # highest | lowest | first | last

[backend]
command = "beep"
dry_run = false
min_duration_ms = 30             # 発音の最短長
inter_note_gap_ms = 20           # 音と音の境目に挿入する休符 (刻み表現用)
gain_percent = 100               # 発音時間を要求長の何 % にするか (疑似音量)
pitch_shift_octaves = 0          # 周波数を 2^N 倍 (-2 ~ +2)

[discovery]
enabled = true                   # LAN multicast による自動発見
multicast_addr = "239.42.42.42"
multicast_port = 4242

[update]                         # OTA アップデート (任意)
enabled = true
token = "<SHARED_SECRET>"        # X-Update-Token の検証に使う共有秘密
binary_path = "/opt/beepd/beepd"
restart_command = ["systemctl", "restart", "beepd"]
```

`gain_percent` / `pitch_shift_octaves` は PC スピーカーの振幅制御路が存在しないため、
発音時間を短縮して「音の面積」を減らすことで体感音量を下げる、音域を下げて高域の
刺々しさを避ける、といった**疑似的な**音量/音色制御として使う
(詳細は [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#発音の刻みと疑似音量制御))。
コイルスピーカーで鳴らしているノードが爆音な場合などに `gain_percent = 35`,
`pitch_shift_octaves = -1` あたりが実用値。

## インストールスクリプト (任意)

`deploy/install.sh` を使うと `beepd` を systemd サービスとして一括セットアップできる。

```bash
scp target/release/beepd deploy/systemd/beepd.service deploy/systemd/pcspkr.conf \
    deploy/install.sh deploy/beepd-nodeA.example.toml \
    somehost:/tmp/beepd-deploy/
ssh somehost 'cd /tmp/beepd-deploy && mv beepd-nodeA.example.toml beepd.toml && \
              SUDO_PASS=<your-sudo-password> bash install.sh'
```

スクリプトは次のことを行う:

1. `/etc/modprobe.d/blacklist.conf` の `blacklist pcspkr` をコメントアウト
2. `/etc/modules-load.d/pcspkr.conf` を配置 (起動時自動ロード)
3. `pcspkr` が未ロードなら `modprobe pcspkr`
4. `beep` が入っていなければ `apt install`
5. `/opt/beepd/beepd` にバイナリ配置、`/etc/beepd/beepd.toml` に設定
6. `/etc/systemd/system/beepd.service` を配置し `enable --now`

## OTA アップデート

マイコンの OTA と同じ発想で、外部から新しいバイナリをまとめて配布 → 自動再起動できる。
各ノードが再起動して `/status` が 200 を返すまで `--wait-ready` で待てる。

```bash
export BEEP_UPDATE_TOKEN=<beepd.toml の [update] token と同じ値>
beepctl update -b target/release/beepd \
    nodeA:8642 nodeB:8642 nodeC:8642 \
    --wait-ready 30
```

内部の仕組み (トークン検証、原子的差し替え、失敗時ロールバック) は
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#ota-アップデート) 参照。

## テスト

```bash
cargo test --workspace
```

## さらに詳しく

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — 内部設計、同期方法、モジュール構成、拡張ポイント、トラブルシューティング
- [docs/CONFIG.md](docs/CONFIG.md) — `beepd.toml` / `player.toml` 全フィールドリファレンス

## ライセンス

MIT OR Apache-2.0
