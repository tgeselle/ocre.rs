# cf fixtures (recorded against the real API)

Recorded 2026-09-29, once, on a real Cloudflare account, with `cf` **1.0.0-beta.5**
(project-local `node_modules/.bin/cf`, wrangler 4.144.0 delegate), `CF_SEND_TELEMETRY=false`,
stdin `/dev/null`, stdout/stderr captured to files (not a TTY: no banner is printed).

Each check `<name>` has `<name>.stdout`, `<name>.stderr`, `<name>.exit` (and `<name>.request.json`
for the body passed with `--batch @…`, `--body @…` or `--file`).

Redactions (shapes kept): account id → `0123456789abcdef0123456789abcdef`, email →
`user@example.com`, workers.dev subdomain → `example`, home directory → `/Users/user`.
Resource names are the real ones: Worker `ocre-cfcheck-demo` (D1 `ocre-cfcheck-demo`, queues
`ocre-cfcheck-demo-jobs` / `-failed`, KV `ocre-cfcheck-demo-cache`) and `ocre-cfcheck-legacy`.
Every resource was deleted afterwards (account inventory identical before and after).

The account has R2 disabled, so R2 only shows `[10042]`.
Commands run from the app root (`cloudflare.config.ts`, `wrangler.config.ts`, `package.json`).

| Fixture | Command | Exit | What it shows |
|---|---|---|---|
| `auth_whoami` | `cf auth whoami` | 0 | authenticated whoami (exit 0). |
| `d1_list_name_missing` | `cf d1 list --name ocre-cfcheck-demo` | 0 | §4.1 no match → `[]`. |
| `d1_create` | `cf d1 create --name ocre-cfcheck-demo` | 0 | §4.1 create → object with `uuid`, `name`. |
| `d1_list_name_found` | `cf d1 list --name ocre-cfcheck-demo` | 0 | §4.1 exact name. |
| `d1_list_name_partial` | `cf d1 list --name ocre-cfcheck-dem` | 0 | `--name` is a partial match: `ocre-cfcheck-dem` returns `ocre-cfcheck-demo`. |
| `d1_get_missing` | `cf d1 get 00000000-0000-4000-8000-000000000000` | 1 | missing database → `[7404]` 404. |
| `d1_migrations_list_pending` | `cf d1 migrations list 71338291-825e-49d2-bea8-99b13f9db18a` | 0 | remote, 6 pending → `[{Name}]`. |
| `d1_migrations_apply` | `cf d1 migrations apply 71338291-825e-49d2-bea8-99b13f9db18a` | 0 | §4.2 remote apply: exits (4 s), `[{name,status}]`, prompt text on stderr. |
| `d1_migrations_list_none` | `cf d1 migrations list 71338291-825e-49d2-bea8-99b13f9db18a` | 0 | remote, none pending → `[]`. |
| `d1_migrations_apply_none` | `cf d1 migrations apply 71338291-825e-49d2-bea8-99b13f9db18a` | 0 | remote apply with nothing pending → `[]`, exit 0, no prompt. |
| `d1_query_batch_seed` | `cf d1 query 71338291-825e-49d2-bea8-99b13f9db18a --batch @.wrangler/ocre-batch.json` | 0 | §4.8 ONE batch item holding 3 statements (leading `-- seed` comment) → 3 results. Body: `d1_query_batch_seed.request.json`. |
| `d1_query_batch_multi` | `cf d1 query 71338291-825e-49d2-bea8-99b13f9db18a --batch @.wrangler/ocre-batch-multi.json` | 0 | two batch items → 2 results. Body: `d1_query_batch_multi.request.json`. |
| `d1_query_batch_error` | `cf d1 query 71338291-825e-49d2-bea8-99b13f9db18a --batch @.wrangler/ocre-batch-error.json` | 1 | SQL error → `[7500]` 400. Body: `d1_query_batch_error.request.json`. |
| `d1_query_sql_eq` | `cf d1 query 71338291-825e-49d2-bea8-99b13f9db18a --sql=-- c⏎SELECT count(*) AS n FROM posts` | 0 | `--sql=<value starting with -- comment>` works remotely. |
| `d1_delete_no_force` | `cf d1 delete 71338291-825e-49d2-bea8-99b13f9db18a` | 0 | without --force: prompt, `Aborted.`, EXIT 0, nothing deleted. |
| `d1_delete` | `cf d1 delete 71338291-825e-49d2-bea8-99b13f9db18a --force` | 0 | with --force: empty stdout. |
| `d1_delete_missing` | `cf d1 delete 71338291-825e-49d2-bea8-99b13f9db18a --force` | 1 | already deleted → `[7404]`. |
| `queues_list_empty` | `cf queues list` | 0 | §4.5 no queues → `[]`. |
| `queues_create` | `cf queues create --queue-name ocre-cfcheck-demo-jobs` | 0 | create → `{queue_id, queue_name, …}`. |
| `queues_create_500` | `cf queues create --queue-name ocre-cfcheck-demo-jobs-failed` | 1 | transient failure seen once (16 s): `[10013]` 500; nothing created; retry succeeded. |
| `queues_create_dlq` | `cf queues create --queue-name ocre-cfcheck-demo-jobs-failed` | 0 | retry of the above. |
| `queues_create_duplicate` | `cf queues create --queue-name ocre-cfcheck-demo-jobs` | 1 | name taken → `[11009]` 409. |
| `queues_list_one` | `cf queues list` | 0 | one queue, full list item shape. |
| `queues_list` | `cf queues list` | 0 | §4.5 two queues; plain array, no pagination info. |
| `queues_delete_referenced` | `cf queues delete f5e8b72398e342838aa4d707c39cb476 --force` | 1 | queue bound to a Worker → `[11005]` 400. |
| `queues_delete_dlq_referenced` | `cf queues delete 6d31ae5d12b6463ca19d65ea2cb2e83a --force` | 1 | DLQ of a consumer → `[11005]` 400. |
| `queues_consumer_delete` | `cf queues consumers delete 3030f312-929e-4521-8d12-5522af0c5ac8 --queue-id f5e8b72398e342838aa4d707c39cb476 --force` | 0 | consumer delete. |
| `queues_delete` | `cf queues delete f5e8b72398e342838aa4d707c39cb476 --force` | 0 | delete (after Worker deleted). |
| `queues_delete_dlq` | `cf queues delete 6d31ae5d12b6463ca19d65ea2cb2e83a --force` | 0 | delete DLQ. |
| `kv_list_empty` | `cf kv namespaces list` | 0 | §4.6 → `[]`. |
| `kv_create` | `cf kv namespaces create --title ocre-cfcheck-demo-cache` | 0 | §4.6 create → `{id, title, supports_url_encoding}`. |
| `kv_list` | `cf kv namespaces list` | 0 | plain array, no result_info. |
| `kv_get_missing` | `cf kv namespaces get 00000000000000000000000000000000` | 1 | missing namespace → `[10013]` 404 (not 10009). |
| `kv_delete_no_force` | `cf kv namespaces delete 30fc3a1ff6374cdea057902f16092ce8` | 0 | without --force: `Aborted.`, EXIT 0, nothing deleted. |
| `kv_delete` | `cf kv namespaces delete 30fc3a1ff6374cdea057902f16092ce8 --force` | 0 | with --force. |
| `kv_list_after_deploy` | `cf kv namespaces list` | 0 | namespace auto-provisioned by cf deploy, titled `<worker>-cache`. |
| `kv_delete_autoprovisioned` | `cf kv namespaces delete d04d78a9f0af47d5b82f16ab737779d9 --force` | 0 | cleanup. |
| `r2_get_missing` | `cf r2 buckets get ocre-cfcheck-missing` | 1 | §4.5 on an account without R2 → `[10042]` 403 (10006 not observable). |
| `r2_create` | `cf r2 buckets create --name ocre-cfcheck-demo-storage` | 1 | R2 not enabled → `[10042]` 403. |
| `secrets_list_missing_worker` | `cf workers secrets list --worker ocre-cfcheck-missing` | 1 | §4.3 → `[10007]` 404 `This Worker does not exist on your account.`. |
| `secrets_list_existing` | `cf workers secrets list --worker ocre-cfcheck-demo` | 0 | `[{name,type}]` after deploy with --secrets-file. |
| `secrets_bulk_unwrapped_file` | `cf workers secrets bulk --worker ocre-cfcheck-demo --file .wrangler/s.json` | 0 | §4.4 spec body `{N:{type,text}}` via --file: exit 0 but NO secret created. Body: `secrets_bulk_unwrapped_file.request.json`. |
| `secrets_list_after_unwrapped_file` | `cf workers secrets list --worker ocre-cfcheck-demo` | 0 | proof: unchanged. |
| `secrets_bulk_delete_unwrapped_file` | `cf workers secrets bulk --worker ocre-cfcheck-demo --file .wrangler/s-del.json` | 0 | `{N:null}` via --file: exit 0, no-op. Body: `secrets_bulk_delete_unwrapped_file.request.json`. |
| `secrets_bulk_unwrapped_body` | `cf workers secrets bulk --worker ocre-cfcheck-demo --body @.wrangler/s.json` | 0 | same unwrapped body via `--body @file`: exit 0, no-op. Body: `secrets_bulk_unwrapped_body.request.json`. |
| `secrets_bulk` | `cf workers secrets bulk --worker ocre-cfcheck-demo --body @.wrangler/s.json` | 0 | §4.4 WORKING body `{"secrets":{N:{name,type,text}}}` via `--body @file` → map of ALL bindings incl. plain_text vars. Body: `secrets_bulk.request.json`. |
| `secrets_list_after_bulk` | `cf workers secrets list --worker ocre-cfcheck-demo` | 0 | proof: A and B created. |
| `secrets_bulk_file_wrapped` | `cf workers secrets bulk --worker ocre-cfcheck-demo --file .wrangler/s-c.json` | 0 | wrapped body via `--file` also works. Body: `secrets_bulk_file_wrapped.request.json`. |
| `secrets_bulk_delete` | `cf workers secrets bulk --worker ocre-cfcheck-demo --body @.wrangler/s-del.json` | 0 | `{"secrets":{B:null}}` deletes B. Body: `secrets_bulk_delete.request.json`. |
| `secrets_list_after_delete` | `cf workers secrets list --worker ocre-cfcheck-demo` | 0 | proof: B gone. |
| `secrets_bulk_missing_worker` | `cf workers secrets bulk --worker ocre-cfcheck-missing --file .wrangler/s.json` | 1 | → `[10007]` 404. |
| `secrets_list_after_redeploy` | `cf workers secrets list --worker ocre-cfcheck-demo` | 0 | secrets kept by a later `cf deploy` without --secrets-file, as recorded on 2026-09-29. **Not reliable:** on 2026-10-01 (cf 1.0.0-beta.5, MigraSon Worker, secrets from `cf workers secrets bulk`) a deploy without --secrets-file left the Worker with no secret; cf uploads with `keepSecrets: keepVars \|\| !!secretsFile`. `ocre deploy` passes --secrets-file on every deploy (`{}` when it adds nothing). |
| `deploy_autoprovision_conflict` | `cf deploy --secrets-file .wrangler/ocre-secrets.json` | 1 | §4.7 KV binding without id while a namespace titled `<worker>-cache` exists: cf tries to create it → `[10014]`, exit 1 (after assets upload, before Worker upload). |
| `deploy` | `cf deploy --secrets-file .wrangler/ocre-secrets.json` | 0 | §4.7 fresh Worker, --secrets-file, KV without id (auto-provisioned), D1/queues pre-created; URL line `│    https://<worker>.<subdomain>.workers.dev`; DO `Created: OcreChannel`. |
| `deploy_redeploy` | `cf deploy` | 0 | second deploy, no --secrets-file: bindings `(inherited)`, no provisioning. The secrets survived here (see `secrets_list_after_redeploy`), but not on 2026-10-01: never deploy without --secrets-file. |
| `legacy_wrangler_deploy` | `wrangler deploy (wrangler.toml with [[migrations]] tag ocre-realtime-v1)` | 0 | setup for next: wrangler 4.144.0 deploy of a wrangler.toml with DO + `[[migrations]] tag = "ocre-realtime-v1" new_sqlite_classes = ["OcreChannel"]`. |
| `deploy_do_existing_migrations` | `cf deploy` | 0 | §4.7 cf deploy of the same Worker with `exports: { OcreChannel: exports.durableObject({ storage: "sqlite" }) }`: exit 0, no `Durable Object exports reconciliation` block, afterwards exactly one DO namespace `ocre-cfcheck-legacy_OcreChannel` (sqlite); its id before this deploy was not captured. |
| `workers_delete_no_force` | `cf workers delete ocre-cfcheck-demo` | 0 | without --force: `Aborted.`, EXIT 0. |
| `workers_delete_queue_consumer` | `cf workers delete ocre-cfcheck-demo --force` | 1 | Worker consuming a queue → `[10064]` 403. |
| `workers_delete` | `cf workers delete ocre-cfcheck-demo --force` | 0 | by NAME, --force, after consumer removed. |
| `secrets_list_deleted_worker` | `cf workers secrets list --worker ocre-cfcheck-demo` | 1 | after delete → `[10007]`. |
| `wrangler_tail_auth` | `wrangler tail <worker> --format pretty` (wrangler 4.144.0, local, proxy blocked) | 1 | recorded 2026-09-29 without network: wrangler keeps its own login, separate from cf; an expired login it cannot refresh fails before any API call. The tail stream itself is not recorded (fake_cf.sh prints wrangler's documented pretty format). |
