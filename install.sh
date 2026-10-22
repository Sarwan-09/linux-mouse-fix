#!/bin/sh
# Installs linux-mouse-fix for the current user. Run it as yourself, not as root;
# it asks for sudo for the parts that live outside your home folder.
set -eu
cd "$(dirname "$0")"
if [ "$(id -u)" -eq 0 ]; then
    echo "Run this as your normal user, not root." >&2
    exit 1
fi

missing=""
python3 -c 'import tomllib' 2>/dev/null || { echo "Needs Python 3.11 or newer." >&2; exit 1; }
python3 -c 'import evdev' 2>/dev/null || missing="python3-evdev"
python3 -c 'import gi; gi.require_version("Gtk", "4.0"); gi.require_version("Adw", "1")' \
    2>/dev/null || missing="$missing python3-gi gir1.2-gtk-4.0 gir1.2-adw-1"
if [ -n "$missing" ]; then
    echo "Install these first:  sudo apt install$missing" | sed 's/  */ /g' >&2
    exit 1
fi

CONFIG="$HOME/.config/linux-mouse-fix/config.toml"
BIN="$HOME/.local/bin"
mkdir -p "$(dirname "$CONFIG")" "$BIN" "$HOME/.local/share/applications"
[ -e "$CONFIG" ] || cp config.example.toml "$CONFIG"
python3 -I ./linux-mouse-fix --check --config "$CONFIG"

install -m 755 linux-mouse-fix-settings "$BIN/linux-mouse-fix-settings"
sed "s|@BIN@|$BIN|" local.LinuxMouseFix.desktop.in \
    > "$HOME/.local/share/applications/local.LinuxMouseFix.desktop"

sudo install -m 755 linux-mouse-fix /usr/local/bin/linux-mouse-fix
sed "s|@CONFIG@|$CONFIG|" linux-mouse-fix.service.in \
    | sudo tee /etc/systemd/system/linux-mouse-fix.service > /dev/null
sudo systemctl daemon-reload
sudo systemctl enable linux-mouse-fix
sudo systemctl restart linux-mouse-fix

echo "Installed. Open \"Mouse Fix\" from your apps to change settings."
