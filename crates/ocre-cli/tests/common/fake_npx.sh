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
        case "$3" in
          list)
            fail_if migrations_list_fails
            echo "Resource location: ${5#--}"
            if [ -e "$state/pending_migrations" ]; then
              echo "Migrations to be applied:"
              echo "┌───────────┐"
              echo "│ Name      │"
              echo "├───────────┤"
              sed 's/.*/│ & │/' "$state/pending_migrations"
              echo "└───────────┘"
            else
              echo "✅ No migrations to apply!"
            fi
            ;;
          *)
            fail_if migrate_fails
            echo "Migrations applied to $4 ($5)"
            ;;
        esac
        ;;
      execute)
        fail_if execute_fails
        if [ "$4" = "--command" ]; then
          # `--json`: the rows, from execute.json (default: one empty result).
          if [ -e "$state/execute.json" ]; then cat "$state/execute.json"; else echo '[{"results":[],"success":true}]'; fi
        else
          echo "Executed $5 on $3 ($6)"
        fi
        ;;
    esac
    ;;
  dev)
    fail_if dev_fails
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    echo "Ready on http://localhost:$3"
    ;;
  secret)
    # `secret list --format json`: SECRET_KEY_BASE only with has_secret;
    # secret_list.json overrides the output.
    if [ -e "$state/secret_list_fails" ]; then
      echo "✘ [ERROR] Worker \"app\" not found." >&2
      exit 1
    fi
    fail_if secret_list_errors
    if [ -e "$state/secret_list.json" ]; then
      cat "$state/secret_list.json"
    elif [ -e "$state/has_secret" ]; then
      echo '[{"name":"SECRET_KEY_BASE","type":"secret_text"},{"name":"OTHER","type":"secret_text"}]'
    else
      echo '[{"name":"OTHER","type":"secret_text"}]'
    fi
    ;;
  deploy)
    fail_if deploy_fails
    if [ "$2" = "--secrets-file" ]; then
      # The file must hold a fresh SECRET_KEY_BASE, readable by its owner only.
      if ! grep -Eq '^SECRET_KEY_BASE=[0-9a-f]{128}$' "$3" || [ -z "$(find "$3" -perm 600)" ]; then
        echo "✘ [ERROR] bad secrets file $3" >&2
        exit 1
      fi
      echo "secrets file ok" >> "$state/calls.log"
    fi
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    echo "Uploaded app"
    echo "  https://app.example.workers.dev"
    ;;
esac
