#!/bin/sh
# Removes linux-mouse-fix. Your settings in ~/.config/linux-mouse-fix are kept.
set -eu
sudo systemctl disable --now linux-mouse-fix 2>/dev/null || true
sudo rm -f /usr/local/bin/linux-mouse-fix /etc/systemd/system/linux-mouse-fix.service
sudo systemctl daemon-reload
rm -f "$HOME/.local/bin/linux-mouse-fix-settings" \
      "$HOME/.local/share/applications/local.LinuxMouseFix.desktop"
echo "Uninstalled. Delete ~/.config/linux-mouse-fix to remove your settings too."
