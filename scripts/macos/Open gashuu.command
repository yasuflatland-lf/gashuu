#!/bin/sh
set -eu

APP="/Applications/gashuu.app"

show_dialog() {
  /usr/bin/osascript -e "display dialog $1 buttons {\"OK\"} default button \"OK\"" >/dev/null 2>&1 || true
}

if [ ! -d "$APP" ]; then
  show_dialog '"gashuu.app was not found in /Applications. Please move gashuu.app to the Applications folder first, then run this helper again."'
  echo "gashuu.app was not found at: $APP"
  echo "Move gashuu.app to /Applications, then run this helper again."
  exit 1
fi

/usr/bin/xattr -dr com.apple.quarantine "$APP" || {
  show_dialog '"Failed to remove the quarantine attribute from gashuu.app. Please try running this helper again, or run xattr manually from Terminal."'
  echo "Failed to remove quarantine attribute from: $APP"
  exit 1
}

show_dialog '"gashuu is ready to open. The app will launch now."'
/usr/bin/open "$APP"
