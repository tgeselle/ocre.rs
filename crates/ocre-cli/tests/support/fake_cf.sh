#!/bin/sh
# Stand-in for cf, wrangler, npx, npm, node and tsc in CLI tests: one script
# installed under several names; `npm install` links node_modules/.bin/{cf,
# wrangler,tsc} in the app to it. Behaviour is driven by marker files in
# $FAKE_CF_STATE; every cf, wrangler and `npm install` call is appended to
# calls.log as `cf <args>`, `wrangler <args>` or `npm install`.
state="$FAKE_CF_STATE"
tool="${0##*/}"

# cf's error box on stderr (no banner: stderr is not a terminal), exit 1.
# Only for answers the recorded fixtures do not have.
api_error() {
  printf '\n┌ APIError\n│ %s\n│ %s\n└\n' "$1" "$2" >&2
  exit 1
}

# Replays a fixture recorded against the real API (tests/support/cf_fixtures):
# its stdout, stderr and exit code, with the sed expressions given after it
# (e.g. `-e s/ocre-cfcheck-demo/shop/g`).
replay() {
  fixture="$FAKE_CF_FIXTURES/$1"
  shift
  sed -e '' "$@" "$fixture.stdout"
  sed -e '' "$@" "$fixture.stderr" >&2
  exit "$(cat "$fixture.exit")"
}

# The recorded object of a fixture, renamed, without the exit.
recorded() {
  fixture="$FAKE_CF_FIXTURES/$1"
  shift
  sed -e '' "$@" "$fixture.stdout"
}

# The D1 id the fake gives database `<name>`, in place of the recorded one.
uuid_of() { echo "-e s/ocre-cfcheck-demo/$1/g -e s/71338291-825e-49d2-bea8-99b13f9db18a/uuid-$1/g"; }

fail_if() {
  if [ -e "$state/$1" ]; then
    echo "stdout before failure"
    printf '┌ Error\n│ %s\n└\n' "$1" >&2
    exit 1
  fi
}

# `[{"<key>":"<name>"}, ...]` for the state files `<prefix><name>`.
list_json() {
  printf '['
  sep=''
  for file in "$state/$1"*; do
    [ -e "$file" ] || continue
    printf '%s{"%s":"%s"}' "$sep" "$2" "${file##*/"$1"}"
    sep=','
  done
  echo ']'
}

# The rows of the next query: the first file of execute_queue/ (used once),
# else execute.json, else one empty result.
query_rows() {
  next="$(ls "$state/execute_queue" 2>/dev/null | head -n 1)"
  if [ -n "$next" ]; then
    cat "$state/execute_queue/$next"
    rm "$state/execute_queue/$next"
  elif [ -e "$state/execute.json" ]; then cat "$state/execute.json"; else echo '[{"results":[],"success":true}]'; fi
}

case "$tool" in
  npx)
    if [ "$1" = "--version" ]; then echo "10.9.0"; exit 0; fi
    # `npx --yes cf@<version> ...`: cf outside an app.
    shift 2
    tool=cf
    ;;
  npm)
    if [ "$1" = "--version" ]; then echo "10.9.0"; exit 0; fi
    echo "npm $1" >> "$state/calls.log"
    fail_if npm_install_fails
    mkdir -p node_modules/.bin node_modules/cf node_modules/wrangler node_modules/@cloudflare/config
    for bin in cf wrangler tsc; do ln -sf "$0" "node_modules/.bin/$bin"; done
    pin() { sed -n "s/.*\"$1\": \"\\([^\"]*\\)\".*/\\1/p" package.json; }
    cf_version="$(cat "$state/installed_cf" 2>/dev/null || pin cf)"
    wrangler_version="$(cat "$state/installed_wrangler" 2>/dev/null || pin wrangler)"
    printf '{"name":"cf","version":"%s"}\n' "$cf_version" > node_modules/cf/package.json
    printf '{"name":"wrangler","version":"%s"}\n' "$wrangler_version" > node_modules/wrangler/package.json
    echo '{"lockfileVersion":3}' > package-lock.json
    echo "added 3 packages"
    exit 0
    ;;
  node)
    if [ "$1" = "--version" ]; then cat "$state/node_version" 2>/dev/null || echo "v22.23.2"; exit 0; fi
    # `node --input-type=module -e <loader> <cloudflare.config.ts>`: cf's config loader.
    if [ -e "$state/config_invalid" ]; then
      echo '[{"path":["worker","env","CACHE"],"message":"Invalid input"}]' >&2
      exit 1
    fi
    exit 0
    ;;
  tsc)
    if [ -e "$state/tsc_fails" ]; then
      echo "cloudflare.config.ts(12,4): error TS2322: Type '30' is not assignable to type '10 | 60'."
      exit 2
    fi
    exit 0
    ;;
esac

echo "$tool $*" >> "$state/calls.log"

if [ "$tool" = "wrangler" ]; then
  # The test server of `ocre test --e2e`: `wrangler dev --x-new-config --port N --persist-to .wrangler/test-state`.
  if [ "$1" = "dev" ]; then
    fail_if dev_fails
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    echo "[custom build] Running: cargo install -q \"worker-build@^0.8\" && worker-build \${OCRE_BUILD:---release}" >&2
    echo "[wrangler:info] Ready on http://localhost:$4"
    exit 0
  fi
  # `ocre logs`: `wrangler tail <worker> --format <f> [--status s] [--search t]`.
  # The auth failure is wrangler 4.144.0's recorded stderr with an expired,
  # unrefreshable login; the stream is NOT recorded (it needs a deployed
  # Worker): wrangler's documented pretty format, then an end instead of Ctrl-C.
  if [ "$1" = "tail" ]; then
    if [ -e "$state/tail_auth_fails" ]; then replay wrangler_tail_auth; fi
    echo "Successfully created tail, expires at 2026-09-30T05:00:00Z"
    echo "Connected to $2, waiting for logs..."
    echo "GET https://$2.example.workers.dev/posts - Ok @ 9/29/2026, 11:00:00 PM"
    echo "  (info) {\"level\":\"info\",\"message\":\"GET /posts 200\",\"request_id\":\"8c2f1a0b9d3e4f5a-CDG\"}"
    exit 0
  fi
  # The local D1 fallback: `wrangler d1 ... DB --local <args> -c .wrangler/ocre-d1.json --persist-to .wrangler/state`.
  if [ ! -e .wrangler/ocre-d1.json ]; then echo "✘ [ERROR] no derived config" >&2; exit 1; fi
  case "$2 $3" in
    "migrations list")
      fail_if migrations_list_fails
      echo "Resource location: local"
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
    "migrations apply")
      # `ocre test --e2e` then cannot start its server: a non-executable wrangler.
      if [ -e "$state/server_unstartable" ]; then
        cp node_modules/.bin/wrangler "$state/wrangler.copy" && rm node_modules/.bin/wrangler
        mv "$state/wrangler.copy" node_modules/.bin/wrangler && chmod -x node_modules/.bin/wrangler
      fi
      fail_if migrate_fails
      echo "Migrations applied to $4 ($5)"
      ;;
    execute*)
      fail_if execute_fails
      if [ -e "$state/execute_no_table" ]; then echo "✘ [ERROR] no such table: d1_migrations: SQLITE_ERROR" >&2; exit 1; fi
      case " $* " in
        *" --json "*) query_rows ;;
        *) echo "Executed $6 on $3 ($4)" ;;
      esac
      ;;
  esac
  exit 0
fi

case "$1 $2" in
  "auth whoami")
    fail_if whoami_fails
    if [ -e "$state/logged_in" ]; then cat "$state/whoami.json"; else echo '{"authenticated":false,"error":"Not logged in"}'; fi
    ;;
  "auth login")
    fail_if login_fails
    if [ ! -e "$state/login_does_nothing" ]; then touch "$state/logged_in"; fi
    echo "Successfully logged in."
    ;;
  "d1 list")
    # `d1 list --name <name>`: databases created before (state files
    # d1db_<name>). Like the real API, `--name` matches part of a name.
    fail_if d1_list_fails
    for file in "$state"/d1db_*; do
      [ -e "$file" ] || continue
      db="${file##*/d1db_}"
      case "$db" in *"$4"*) replay d1_list_name_found $(uuid_of "$db") ;; esac
    done
    replay d1_list_name_missing
    ;;
  "d1 create")
    fail_if d1_create_fails
    touch "$state/d1db_$4"
    if [ -e "$state/d1_create_no_uuid" ]; then replay d1_create $(uuid_of "$4") -e '/"uuid"/d'; fi
    replay d1_create $(uuid_of "$4")
    ;;
  "d1 migrations")
    case "$3" in
      list)
        fail_if migrations_list_fails
        if [ ! -e "$state/pending_migrations" ]; then replay d1_migrations_list_none; fi
        # The recorded format, with the names of pending_migrations.
        printf '['
        sep=''
        while read -r name; do printf '%s\n  {\n    "Name": "%s"\n  }' "$sep" "$name"; sep=','; done < "$state/pending_migrations"
        printf '\n]\n'
        ;;
      apply)
        fail_if migrate_fails
        if [ -e "$state/pending_migrations" ]; then replay d1_migrations_apply; fi
        replay d1_migrations_apply_none
        ;;
    esac
    ;;
  "d1 query")
    # `d1 query <id> --batch @<file>`: logs the SQL sent as a call. The file
    # must hold the recorded request shape: `[{"sql": "..."}]`.
    fail_if query_fails
    batch="$(cat "${5#@}")"
    case "$batch" in
      '[{"sql":'*) ;;
      *) api_error "[7400] the batch must be an array of {\"sql\": ...} objects" "400 Bad Request" ;;
    esac
    # printf, not echo: sh's echo would expand the JSON's `\n` escapes.
    printf 'batch %s\n' "$batch" >> "$state/calls.log"
    if [ -e "$state/execute_no_table" ]; then
      replay d1_query_batch_error $(uuid_of "${4#uuid-}") -e s/missing_table/d1_migrations/
    fi
    query_rows
    ;;
  "queues list")
    # The queues created before (state files queue_<name>), in the recorded shape.
    fail_if queues_list_fails
    set -- "$state"/queue_*
    if [ ! -e "$1" ]; then replay queues_list_empty; fi
    printf '['
    sep=''
    for file in "$@"; do
      printf '%s' "$sep"
      recorded queues_create -e "s/ocre-cfcheck-demo-jobs/${file##*/queue_}/g" -e 's/^/  /'
      sep=','
    done
    echo ']'
    ;;
  "queues create")
    # queues_create_fails: the transient 500 recorded once.
    if [ -e "$state/queues_create_fails" ]; then replay queues_create_500; fi
    touch "$state/queue_$4"
    replay queues_create -e "s/ocre-cfcheck-demo-jobs/$4/g"
    ;;
  "r2 buckets")
    # Recorded on an account without R2: 10042 only. A missing bucket (10006),
    # other errors and creation are not recorded.
    case "$3" in
      get)
        if [ -e "$state/r2_not_enabled" ]; then replay r2_get_missing -e "s/ocre-cfcheck-missing/$4/g"; fi
        if [ -e "$state/r2_get_fails" ]; then api_error "[10000] Authentication error" "401 Unauthorized"; fi
        if [ -e "$state/bucket_$4" ]; then echo "{\"name\":\"$4\"}"; else api_error "[10006] The specified bucket does not exist." "404 Not Found"; fi
        ;;
      create)
        fail_if r2_create_fails
        touch "$state/bucket_$5"
        echo "{\"name\":\"$5\"}"
        ;;
    esac
    ;;
  "kv namespaces")
    # `list --per-page 100 --page <n>`: kv_page_<n>.json when present, else
    # the namespaces created before (state files kvns_<title> holding the id)
    # on page 1, in the recorded shape. `create --title <title>` creates one.
    case "$3" in
      list)
        fail_if kv_list_fails
        if [ -e "$state/kv_list_garbage" ]; then echo "not json"; exit 0; fi
        if [ -e "$state/kv_page_$7.json" ]; then cat "$state/kv_page_$7.json"; exit 0; fi
        set -- "$7" "$state"/kvns_*
        if [ "$1" != "1" ] || [ ! -e "$2" ]; then replay kv_list_empty; fi
        shift
        printf '['
        sep=''
        for file in "$@"; do
          printf '%s' "$sep"
          recorded kv_create -e "s/ocre-cfcheck-demo-cache/${file##*/kvns_}/g" \
            -e "s/30fc3a1ff6374cdea057902f16092ce8/$(cat "$file")/g" -e 's/^/  /'
          sep=','
        done
        echo ']'
        ;;
      create)
        fail_if kv_create_fails
        echo "id-$5" > "$state/kvns_$5"
        set -- -e "s/ocre-cfcheck-demo-cache/$5/g" -e "s/30fc3a1ff6374cdea057902f16092ce8/id-$5/g"
        if [ -e "$state/kv_create_no_id" ]; then replay kv_create "$@" -e '/"id"/d'; fi
        replay kv_create "$@"
        ;;
    esac
    ;;
  "workers secrets")
    case "$3" in
      list)
        # secret_list.json overrides the output; else the Worker's deployed
        # secrets: OTHER (until a deploy drops them), SECRET_KEY_BASE with
        # has_secret, and those uploaded by `bulk` (deployed_secret_<name>).
        if [ -e "$state/secret_list_fails" ]; then replay secrets_list_missing_worker -e "s/ocre-cfcheck-missing/$5/g"; fi
        if [ -e "$state/secret_list_errors" ]; then api_error "[10000] Authentication error" "401 Unauthorized"; fi
        if [ -e "$state/secret_list.json" ]; then cat "$state/secret_list.json"; exit 0; fi
        names=""
        if [ ! -e "$state/secrets_dropped" ]; then names="OTHER"; fi
        if [ -e "$state/has_secret" ]; then names="SECRET_KEY_BASE $names"; fi
        for file in "$state"/deployed_secret_*; do [ -e "$file" ] && names="$names ${file##*/deployed_secret_}"; done
        printf '['
        sep=''
        for name in $names; do printf '%s\n  {\n    "name": "%s",\n    "type": "secret_text"\n  }' "$sep" "$name"; sep=','; done
        printf '\n]\n'
        ;;
      bulk)
        # `bulk --worker <name> --file <file>`: logs the uploaded JSON as a
        # call. Like the real API, only `{"secrets":{"N":{"name":"N",...}}}`
        # creates secrets; the unwrapped map is accepted and ignored.
        if [ -e "$state/secret_bulk_fails" ]; then replay secrets_bulk_missing_worker -e "s/ocre-cfcheck-missing/$5/g"; fi
        body="$(cat "$7")"
        printf 'uploaded %s\n' "$body" >> "$state/calls.log"
        case "$body" in
          '{"secrets":{'*)
            for name in $(printf '%s' "$body" | grep -o '"name":"[^"]*"' | cut -d'"' -f4); do touch "$state/deployed_secret_$name"; done
            ;;
        esac
        replay secrets_bulk -e s/OCRE_CFCHECK_A/UPLOADED_A/g -e s/OCRE_CFCHECK_B/UPLOADED_B/g
        ;;
    esac
    ;;
  "deploy "* | "deploy")
    fail_if deploy_fails
    if [ "$2" = "--secrets-file" ]; then
      # Readable by its owner only: JSON (`{}`, or a fresh SECRET_KEY_BASE),
      # or a non-empty .env (cf rejects an empty one). The Worker keeps its
      # secrets and gets those of the file.
      body="$(cat "$3")"
      if ! printf '%s' "$body" | grep -Eqx '\{\}|\{"SECRET_KEY_BASE":"[0-9a-f]{128}"\}|SECRET_KEY_BASE=[0-9a-f]{128}' || [ -z "$(find "$3" -perm 600)" ]; then
        echo "✘ [ERROR] bad secrets file $3" >&2
        exit 1
      fi
      echo "secrets file ok" >> "$state/calls.log"
      cp "$3" "$state/uploaded_secrets"
      secrets="keep"
      case "$body" in *SECRET_KEY_BASE*) secrets="add" ;; esac
    else
      secrets="drop"
    fi
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    worker="$(sed -n 's/^		name: "\(.*\)",$/\1/p' cloudflare.config.ts)"
    # cf provisions a KV binding left without an id itself, titled
    # `<worker>-<binding>`, and fails (10014) when that title exists already.
    for binding in $(sed -n 's/^[[:space:]]*\([A-Z_]*\): bindings\.kv(\({}\)\{0,1\}),.*/\1/p' cloudflare.config.ts); do
      title="$worker-$(echo "$binding" | tr 'A-Z_' 'a-z-')"
      if [ -e "$state/kvns_$title" ]; then replay deploy_autoprovision_conflict -e "s/ocre-cfcheck-demo/$worker/g"; fi
      echo "id-$title" > "$state/kvns_$title"
      echo "autoprovisioned $title" >> "$state/calls.log"
    done
    # The new version is uploaded: cf 1.0.0-beta.5 keeps the deployed
    # secrets only with --secrets-file (`keepSecrets: keepVars || !!secretsFile`).
    case "$secrets" in
      add) touch "$state/has_secret" ;;
      drop) rm -f "$state/has_secret" "$state"/deployed_secret_*; touch "$state/secrets_dropped" ;;
    esac
    if [ -e "$state/deploy_no_url" ]; then replay deploy -e "s/ocre-cfcheck-demo/$worker/g" -e '/workers\.dev/d'; fi
    replay deploy -e "s/ocre-cfcheck-demo/$worker/g"
    ;;
  "dev --port")
    fail_if dev_fails
    echo "build $OCRE_BUILD" >> "$state/calls.log"
    echo "[wrangler:info] Ready on http://localhost:$3"
    ;;
esac
