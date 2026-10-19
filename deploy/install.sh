#!/usr/bin/env bash
# beepd installer — expects the following files present alongside this script:
#   beepd            (release binary)
#   beepd.toml       (rendered config for THIS host)
#   beepd.service    (systemd unit)
#   pcspkr.conf      (modules-load.d entry)
set -euo pipefail

STAGE_DIR="$(dirname "$(readlink -f "$0")")"
echo "== beepd installer =="
echo "stage dir: $STAGE_DIR"

SUDO=""
if [[ $EUID -ne 0 ]]; then
    SUDO="sudo -S"
fi

do_sudo() {
    if [[ -n "$SUDO" ]]; then
        printf '%s\n' "${SUDO_PASS:-}" | $SUDO "$@"
    else
        "$@"
    fi
}

# 1. blacklist pcspkr を無効化 (存在すれば)
BL=/etc/modprobe.d/blacklist.conf
if [[ -f $BL ]] && grep -qE '^\s*blacklist\s+pcspkr' "$BL"; then
    echo "-- unblocking pcspkr in $BL --"
    do_sudo cp "$BL" "$BL.bak.beepd"
    do_sudo sed -i 's/^\s*\(blacklist\s\+pcspkr\)/# \1  # commented out by beep-orchestra installer/' "$BL"
fi

# 2. modules-load.d で自動ロード指定
echo "-- writing /etc/modules-load.d/pcspkr.conf --"
do_sudo install -m 0644 "$STAGE_DIR/pcspkr.conf" /etc/modules-load.d/pcspkr.conf

# 3. 今すぐロード (既にロード済みならスキップ)
if ! lsmod | grep -q '^pcspkr'; then
    echo "-- modprobe pcspkr --"
    do_sudo modprobe pcspkr || echo "modprobe failed (module may not exist on this kernel)"
fi

# 4. beep 導入確認
if ! command -v beep >/dev/null 2>&1; then
    echo "-- installing beep --"
    do_sudo apt-get update -qq
    do_sudo apt-get install -y -qq beep
fi

# 5. beepd バイナリ配置
echo "-- installing /opt/beepd/beepd --"
do_sudo install -d -m 0755 /opt/beepd
do_sudo install -m 0755 "$STAGE_DIR/beepd" /opt/beepd/beepd

# 6. 設定ファイル配置
echo "-- installing /etc/beepd/beepd.toml --"
do_sudo install -d -m 0755 /etc/beepd
do_sudo install -m 0644 "$STAGE_DIR/beepd.toml" /etc/beepd/beepd.toml

# 7. systemd unit 配置
echo "-- installing /etc/systemd/system/beepd.service --"
do_sudo install -m 0644 "$STAGE_DIR/beepd.service" /etc/systemd/system/beepd.service
do_sudo systemctl daemon-reload
do_sudo systemctl enable --now beepd.service

# 8. ステータス表示
echo "-- systemctl status --"
do_sudo systemctl --no-pager status beepd.service | head -25 || true
echo "-- listening --"
sleep 1
ss -tln 2>/dev/null | grep ":8642" || echo "(port 8642 not yet listening)"

echo "== done =="
