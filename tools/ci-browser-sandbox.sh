#!/usr/bin/env bash
set -euo pipefail
sudo tee /etc/apparmor.d/aex-playwright >/dev/null <<EOF
abi <abi/4.0>,
profile aex-playwright "$HOME/.cache/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell" flags=(unconfined) {
  userns,
}
EOF
sudo apparmor_parser -r /etc/apparmor.d/aex-playwright
