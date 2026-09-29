#!/bin/sh
# Stand-in for `npx --yes wrangler@4 ...` in CLI tests. Behaviour is driven by
# marker files in $FAKE_WRANGLER_STATE; every call is appended to calls.log.
state="$FAKE_WRANGLER_STATE"
shift 2 # drop `--yes wrangler@4`
echo "$*" >> "$state/calls.log"

fail_if() {
  if [ -e "$state/$1" ]; then
    echo "stdout before failure"
    echo "✘ [ERROR] $1" >&2
    exit 1
  fi
}

case "$1" in
  whoami)
    if [ -e "$state/logged_in" ]; then cat "$state/whoami.json"; else echo "Not logged in" >&2; exit 1; fi
    ;;
  login)
    fail_if login_fails
    if [ ! -e "$state/login_does_nothing" ]; then touch "$state/logged_in"; fi
    echo "Successfully logged in."
    ;;
  d1)
    case "$2" in
      list)
        fail_if d1_list_fails
        cat "$state/d1_list.json"
        ;;
      migrations)
        fail_if migrate_fails
        echo "Migrations applied to $4 ($5)"
        ;;
    esac
    ;;
  dev)
    fail_if dev_fails
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    echo "Ready on http://localhost:$3"
    ;;
  deploy)
    fail_if deploy_fails
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    echo "Uploaded app"
    echo "  https://app.example.workers.dev"
    ;;
esac
