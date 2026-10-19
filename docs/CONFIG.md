# CONFIG リファレンス

`beepd.toml` および `player.toml` の全フィールド。

## `beepd.toml` (デーモン)

### トップレベル

| フィールド | 型 | 既定 | 意味 |
|---|---|---|---|
| `node_name` | string | ホスト名 | ノードの表示名。`beepctl --nodes` でここに書いた名前が使える |
| `master_mode` | bool | `false` | 現状は情報用途のみで挙動に差はない (全ノードがマスターになれる) |
| `listen_addr` | string | `"0.0.0.0"` | HTTP サーバの bind アドレス |
| `listen_port` | u16 | `8642` | HTTP サーバのポート |
| `priority_algorithm` | enum | `"highest"` | 単体再生時のモノフォニック選音ポリシー。`highest` \| `lowest` \| `first` \| `last` |
| `static_peers` | `[string]` | `[]` | L2 を越える経路の静的ピア (`"host:port"`)。multicast が届かない相手にはこれで繋ぐ |
| `static_peer_poll_ms` | u32 | `3000` | `static_peers` を `GET /nodes` でポーリングする間隔 |

### `[backend]`

| フィールド | 型 | 既定 | 意味 |
|---|---|---|---|
| `command` | string | `"beep"` | 外部呼び出しコマンド |
| `extra_args` | `[string]` | `[]` | `beep` に渡す追加引数 (周波数・長さは自動付与) |
| `dry_run` | bool | `false` | `true` にすると実際には鳴らさずログだけ |
| `min_duration_ms` | u32 | `30` | 発音の最短長 (短すぎるとカチカチするだけになる) |
| `inter_note_gap_ms` | u32 | `20` | 音と音の境目に挿入する休符 (ms)。これが刻みの分解能になる。20 未満は人間の耳で判別困難 |
| `gain_percent` | u8 | `100` | 発音時間を要求長の何 % にするか (疑似音量)。100 未満のときは `min_duration_ms` は無視される |
| `pitch_shift_octaves` | i8 | `0` | 周波数を 2^N 倍。コイルスピーカーで高域が痛いときの緩和に有効。周波数は 20-20000 Hz にクランプ |

### `[discovery]`

| フィールド | 型 | 既定 | 意味 |
|---|---|---|---|
| `enabled` | bool | `true` | UDP multicast による自動発見 |
| `multicast_addr` | string | `"239.42.42.42"` | announce を送るマルチキャストアドレス |
| `multicast_port` | u16 | `4242` | announce ポート |
| `announce_interval_ms` | u32 | `1000` | 自己 announce の送信間隔 |

### `[update]` (任意)

OTA アップデートを有効化する場合のみ記述する。

| フィールド | 型 | 既定 | 意味 |
|---|---|---|---|
| `enabled` | bool | `false` | 有効化。`false` の場合 `/update` は 403 |
| `token` | string | — | `X-Update-Token` 検証用の共有秘密。**必ず `chmod 0600`** |
| `binary_path` | string | — | 差し替え対象の実バイナリ (例: `/opt/beepd/beepd`) |
| `restart_command` | `[string]` | — | 入替後に走らせる再起動コマンド (例: `["systemctl", "restart", "beepd"]`) |
| `verify_with_version` | bool | `true` | 新バイナリに `--version` を叩いて疎通確認してから差し替える |

### 設定例 (3 ノード構成の 1 台)

```toml
node_name = "nodeA"
listen_addr = "0.0.0.0"
listen_port = 8642
priority_algorithm = "highest"

# multicast が届かない overlay 経由の他ノード
static_peers = [
    "10.0.0.2:8642",
    "10.0.0.3:8642",
]
static_peer_poll_ms = 3000

[backend]
command = "beep"
dry_run = false
min_duration_ms = 30
inter_note_gap_ms = 20
# コイルスピーカーのノードなら gain_percent = 35 など
gain_percent = 100
pitch_shift_octaves = 0

[discovery]
enabled = false   # overlay 経由のときは multicast を無効化しておく

[update]
enabled = true
token = "<SHARED_SECRET>"
binary_path = "/opt/beepd/beepd"
restart_command = ["systemctl", "restart", "beepd"]
verify_with_version = true
```

## `player.toml` (`beep-player` 単体 CLI)

### `[backend]`

`beepd` と同一フィールド (`command`, `extra_args`, `dry_run`, `min_duration_ms`,
`inter_note_gap_ms`, `gain_percent`, `pitch_shift_octaves`)。

### トップレベル

| フィールド | 型 | 既定 | 意味 |
|---|---|---|---|
| `priority_algorithm` | enum | `"highest"` | モノフォニック選音ポリシー |

### 設定例

```toml
priority_algorithm = "highest"

[backend]
command = "beep"
dry_run = false
min_duration_ms = 30
inter_note_gap_ms = 20
```
