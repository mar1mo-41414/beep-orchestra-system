# ARCHITECTURE

内部設計のメモ。日常の使い方は [README](../README.md) 参照。

## 全体構成

```
  +---------------+       UDP 239.42.42.42:4242 (announce)
  |    beepd A    | <---------------+------+------> ...
  |  (MASTER)     |                 |      |
  +-------+-------+                 |      |
          |  HTTP  POST /play_part  |      |
          |    (JSON: {score, start_at})   |
          v                         |      |
  +---------------+                 |      |
  |    beepd B    | <---------------+      |
  +---------------+                        |
  +---------------+                        |
  |    beepd C    | <----------------------+
  +---------------+

  MIDI ─┬── monophonic_stream (Highest/Lowest/First/Last)
        │        → 単体モードの発音イベント列
        │
        └── assign_voices(N=nodes)
                 → Voice 0 = 最高音, Voice N-1 = 最低音
                 → 超過音は下位から捨てる (優先切り捨て)
```

マスターは特別な存在ではなく、どのノードもマスターになれる。`/orchestra/play` を
受けたノードが、自身を含む全ノードへ声部を配布する役を担う。

## 同期方法 (絶対時刻)

**前提**: 全ノードが NTP で時刻同期されていること。

再生は「絶対時刻 (unix 秒, 小数可)」で開始する。マスターは
`current_epoch + lead_time_sec` を計算し、全ノードに `start_at` として通知する。
各ノードは自分の壁時計でその時刻まで待ってから一斉に発音する。

```
現在: t = T
     ↓
  start_at = T + 2.0
     ↓
  [beepd A]  ├─── wait 2.0s ───┼── play voice 0 ──┤
  [beepd B]  ├─── wait 2.0s ───┼── play voice 1 ──┤
  [beepd C]  ├─── wait 2.0s ───┼── play voice 2 ──┤
                                ↑
                               全ノードがここで一斉開始
```

LAN 遅延 < lead_time なら、開始時刻のズレは ntpd 精度 (数 ms) の中に収まる。
実測ではローカル同居 3 ノードで μs 級で揃った。

## モジュール構成

```
crates/beep-common/     ライブラリ
  ├─ midi.rs            SMF 解析 → Score
  ├─ score.rs           Note (start, end, key, velocity, channel, track)
  ├─ pitch.rs           MIDI 番号 → Hz (A4=440Hz, 整数丸め)
  ├─ select.rs          モノフォニック選音 (Highest/Lowest/First/Last)
  ├─ voicing.rs         パート分配 (assign_voices)
  └─ node.rs            NodeInfo (Uuid, name, ip, http_port, capabilities)

crates/beep-player/     CLI (単体再生)
  ├─ config.rs          TOML 設定
  ├─ backend.rs         ToneBackend trait + BeepBackend + DryRunBackend
  └─ player.rs          MonoPlayer (ドリフト補正付き)

crates/beepd/           デーモン
  ├─ config.rs          TOML 設定 (listen, node_name, backend, discovery)
  ├─ backend.rs         beep-player と同等の ToneBackend
  ├─ engine.rs          独立スレッドで動く再生エンジン
  │                     (Play/Pause/Resume/Stop を mpsc で受ける)
  ├─ discovery.rs       UDP マルチキャストで告知 & 発見、TTL 管理
  ├─ static_peers.rs    L2 を越える場合の静的ピア
  ├─ orchestra.rs       マスターがボイスを各ノードへ配布
  ├─ updater.rs         OTA アップデート (バイナリ原子的差し替え)
  ├─ ui/index.html      内蔵 Web UI (単一 HTML、include_str! で埋め込み)
  └─ api.rs             axum の HTTP ルータ

crates/beep-convert/    MIDI info / dump / voices / import-beepsh の CLI
crates/beepctl/         複数ノード一括操作 CLI (status/nodes/play/orchestra/stop/update)
```

## 発音の刻みと疑似音量制御

PC スピーカー / コイルスピーカーで鳴らす都合、以下の設計上の工夫が入っている。

### 刻み表現

ナイーブな実装では「同じ音の連打」や「異なる音の連続」が長音に化けやすい。
原因は 2 つあり、それぞれ対処している:

1. **選音アルゴリズム (`select.rs`)**: スイープラインで区間を作る際、
   同じピッチが連続する区間を結合すると「刻み」が長音に化ける。
   `monophonic_stream` は休符 (`None`) 連続だけを結合し、同一ピッチの再アタックは
   別イベントとして保持する。
2. **バッチ発音 (`backend.rs`)**: `engine.rs` が隣接する短い音を 1 プロセスの `beep`
   呼び出しにバッチ化する (プロセス起動オーバヘッド回避)。ナイーブに `-n` だけで
   連結すると音と音の間に隙間が無く、矩形波なので境目が耳で判別できない。
   各音に `-D <gap_ms>` を挟んで `beep -f F1 -l L1 -D G -n -f F2 -l L2 -D G -n ...`
   の形にすることで、1 プロセス内でも刻みが表現される。

### 疑似音量制御

`pcspkr` の低レベル API (`KDMKTONE` / evdev `SND_TONE`) は周波数だけを渡す設計で、
**振幅 (= 音量) の制御路が存在しない**。デューティ比も 50% 固定。
`beep` コマンド自体にも音量オプションは無い。

振幅をいじれないので、代わりに次の 2 つの軸でノード単位の緩和策を実装している
(`BackendConfig`):

| 設定 | 範囲 | 効果 |
|---|---|---|
| `gain_percent` | 0-100 (既定 100) | 発音時間を要求長の `gain_percent %` に切り詰め、残りを休符にする。耳は「音の面積」で音量を感じるので、小さく聞こえる。歯切れは強くなる |
| `pitch_shift_octaves` | -3..3 (既定 0) | 周波数を 2^N 倍。コイルスピーカーは高域 (2-5 kHz) が特に耳に痛いので、-1 ないし -2 にすると劇的に穏やかになる |

内部では `shape_note(requested, gap, min, gain)` が `(sound_ms, delay_ms)` を返し、
**`sound_ms + delay_ms` は常に要求長に一致**するよう設計されている。
よって `gain_percent` を絞っても各音の総所要時間は MIDI 通りで、オーケストラ同期が
壊れない。バッチ発音時は差分を `-D delay_ms` に寄せる。

### 聴覚限界と既定値

`inter_note_gap_ms` の既定値は 20 ms。これより短いと同一ピッチ連打の境目が
人間の耳で判別できない。バッチ内 `-D` にも使われるため、刻みの分解能は
ほぼこの値で決まる。

## OTA アップデート

マイコンの OTA と同じ発想で、外部から新バイナリをまとめて配布 → 自動再起動する。

### クライアント側 (`beepctl update`)

```bash
export BEEP_UPDATE_TOKEN=<beepd.toml の [update] token と同じ値>
beepctl update -b target/release/beepd \
    nodeA:8642 nodeB:8642 nodeC:8642 \
    --wait-ready 30
```

`--wait-ready N` を渡すと、対象ノードの `/status` が再び 200 を返すまで最大 N 秒
ポーリングし、各ノードが復帰したことを表示する。

### サーバ側 (`POST /update`)

1. `X-Update-Token` ヘッダを検証 (`beepd.toml` は `chmod 0600` でトークン保護)
2. 送信バイナリを `<binary_path>.new` に書き出し `0755`
3. `<binary_path>.new --version` を叩いて壊れていないことを確認
4. 既存を `<binary_path>.old` にリネーム、`.new` を `<binary_path>` に原子的入替
5. `setsid sh -c 'sleep 0.5; systemctl restart beepd'` で切り離し起動
6. 応答 (JSON) を返し、その後 systemd が新プロセスへ入れ替える

失敗時 (`--version` が非 0、リネーム失敗 など) は既存バイナリを触らず 5xx で返す。
前回バックアップは `<binary_path>.old` に残るので手動ロールバック可能。

## `static_peers` (別サブネット・WireGuard 越し)

UDP マルチキャストは L2 セグメントを越えないため、別サブネットや
Tailscale/WireGuard 経由のノードとつなぐには `static_peers` で HTTP エンドポイントを
明示する。`beepd` は `static_peer_poll_ms` ごとに `GET /nodes` を叩いて相手の情報を
peer table に流し込む。エンドポイントの `host:port` を到達アドレスとして採用する
(相手の self_node.ip が `0.0.0.0` を返しても正しくルーティングされる)。

```toml
# 例: nodeA 側で nodeB, nodeC を明示ピアに指定 (overlay IP で)
static_peers = ["10.0.0.2:8642", "10.0.0.3:8642"]
static_peer_poll_ms = 3000

[discovery]
enabled = false   # multicast は届かないので無効化
```

## パート分配戦略

`/orchestra/play` の `voicing` パラメータで切り替える。

### `pitch` (既定)

スコアの各瞬間で「同時に鳴っている音」を高い順にランク付けし、最高音を voice 0、
次を voice 1... と順に振る。単一トラックの MIDI や既存曲を勝手にパート分けしたい
ときに使う。

### `track`

SMF のトラック順にそのまま割り当てる。DAW でメロディ / ハーモニー / ベースを別
トラックに書き出した MIDI で、「メインをどの端末に、サブをどの端末に」を意図通り
制御できる。音符 0 個のトラック (テンポメタトラック等) は自動スキップされる。

## 差し替え設計

- **ToneBackend trait**: `beep` 以外 (rodio, ALSA, speaker-test など) に差し替え可能
- **SelectPolicy**: `SelectPolicy` enum に variant を足せばアルゴリズム追加できる
- **assign_voices**: 現状は「高音降順ランク」。voice leading を考慮した割り当ても
  `assign_voices` を別関数として追加すれば差し替えられる
- **Discovery**: `PeerTable` を trait 化していないが、mDNS などへ差し替えるなら
  `discovery.rs` を新モジュールに差し替えるだけで済む

## 拡張候補 (未実装)

- 音量エミュレーション (PWM 的な擬似 velocity)
- リピート / プレイリスト
- OSC / MQTT / TCP 同期
- PTP による μs 精度同期
- 録音 (仮想デバイス経由)
- MIDI 入力デバイスからのリアルタイム演奏

## トラブルシューティング

| 症状 | 原因 / 対処 |
|---|---|
| `beep: Could not open any device` | `pcspkr` が読まれていない、または root/setuid でない |
| API は 200 を返すが音が出ない | `dry_run = true` になっていないか確認 |
| `/nodes` に相手が出てこない | LAN が multicast をブロックしていないか (VLAN, iptables)。`announce_interval` を短く、または `static_peers` を使う |
| ノード間で発音がズレる | NTP 同期状態を `chronyc tracking` で確認。`lead_time_sec` を伸ばす |
| チャキチャキ切れて聞こえる | `inter_note_gap_ms` や `min_duration_ms` を調整 |
| 同じ音が長音に化ける (再発したら) | `select.rs` の同一ピッチ結合ロジックが入っていないか確認。本リポジトリでは廃止済み |
| MIDI 読み込みで失敗 | SMPTE 形式のタイムベース (Timecode) や巨大な RIFF MIDI は未対応 |
