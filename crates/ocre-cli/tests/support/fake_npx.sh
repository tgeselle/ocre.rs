#!/bin/sh
# Stand-in for `npx --yes wrangler@4 ...` in CLI tests. Behaviour is driven by
# marker files in $FAKE_WRANGLER_STATE; every call is appended to calls.log.
state="$FAKE_WRANGLER_STATE"
if [ "$1" = "--version" ]; then echo "10.9.0"; exit 0; fi
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
      create)
        fail_if d1_create_fails
        echo "Created D1 database '$3'"
        ;;
      execute)
        fail_if execute_fails
        if [ -e "$state/execute_no_table" ]; then echo "✘ [ERROR] no such table: d1_migrations: SQLITE_ERROR" >&2; exit 1; fi
        if [ "$4" = "--command" ]; then
          # `--json`: the rows, from the first file of execute_queue/ (used
          # once), else execute.json (default: one empty result).
          next="$(ls "$state/execute_queue" 2>/dev/null | head -n 1)"
          if [ -n "$next" ]; then
            cat "$state/execute_queue/$next"
            rm "$state/execute_queue/$next"
          elif [ -e "$state/execute.json" ]; then cat "$state/execute.json"; else echo '[{"results":[],"success":true}]'; fi
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
    # `secret bulk <file>`: logs the uploaded JSON as a call.
    if [ "$2" = "bulk" ]; then
      fail_if secret_bulk_fails
      echo "uploaded $(cat "$3")" >> "$state/calls.log"
      echo "✨ Finished processing secrets file"
      exit 0
    fi
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
  queues)
    # `queues info <name>` succeeds for queues created before (state file
    # queue_<name>); `queues create <name>` creates one.
    case "$2" in
      info)
        fail_if queues_info_fails
        if [ -e "$state/queue_$3" ]; then echo "Queue Name: $3"; else echo "✘ [ERROR] Queue \"$3\" does not exist. To create it, run: wrangler queues create $3" >&2; exit 1; fi
        ;;
      create)
        fail_if queues_create_fails
        touch "$state/queue_$3"
        echo "Created queue $3"
        ;;
    esac
    ;;
  r2)
    # `r2 bucket info <name> --json` succeeds for buckets created before
    # (state file bucket_<name>); r2_not_enabled makes it fail like an
    # account without R2; `r2 bucket create <name>` creates one.
    case "$3" in
      info)
        if [ -e "$state/r2_not_enabled" ]; then echo "✘ [ERROR] A request to the Cloudflare API failed. Please enable R2 through the Cloudflare Dashboard. [code: 10042]" >&2; exit 1; fi
        if [ -e "$state/bucket_$4" ]; then echo "{\"name\": \"$4\"}"; else echo "✘ [ERROR] The specified bucket does not exist. [code: 10006]" >&2; exit 1; fi
        ;;
      create)
        fail_if r2_create_fails
        touch "$state/bucket_$4"
        echo "Created bucket '$4' with default storage class of Standard."
        ;;
    esac
    ;;
  kv)
    # `kv namespace list` prints the namespaces created before (state files
    # kvns_<title> holding the id) as JSON; `kv namespace create <title>`
    # creates one (but does not list it with kv_create_unlisted).
    case "$3" in
      list)
        fail_if kv_list_fails
        if [ -e "$state/kv_list_garbage" ]; then echo "not json"; exit 0; fi
        printf '['
        sep=''
        for file in "$state"/kvns_*; do
          [ -e "$file" ] || continue
          printf '%s{"id":"%s","title":"%s","supports_url_encoding":true}' "$sep" "$(cat "$file")" "${file##*/kvns_}"
          sep=','
        done
        echo ']'
        ;;
      create)
        fail_if kv_create_fails
        if [ ! -e "$state/kv_create_unlisted" ]; then echo "id-$4" > "$state/kvns_$4"; fi
        echo "Creating namespace with title \"$4\""
        ;;
    esac
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
