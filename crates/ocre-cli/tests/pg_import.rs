use super::*;

/// What `pg_dump --no-owner --no-acl` writes for two tables (PostgreSQL 16).
const DUMP: &str = r#"--
-- PostgreSQL database dump
--

SET statement_timeout = 0;
SET client_encoding = 'UTF8';
SELECT pg_catalog.set_config('search_path', '', false);

CREATE EXTENSION IF NOT EXISTS citext WITH SCHEMA public;

CREATE TYPE public.video_status AS ENUM (
    'uploaded',
    'processing',
    'done'
);

CREATE FUNCTION public.touch() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.updated_at = now(); -- keep it fresh;
  RETURN NEW;
END;
$$;

CREATE TABLE public.users (
    id bigint NOT NULL,
    email public.citext NOT NULL,
    admin boolean DEFAULT false NOT NULL
);

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;

CREATE TABLE public.videos (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id bigint,
    title character varying(255) NOT NULL,
    status public.video_status DEFAULT 'uploaded'::public.video_status NOT NULL,
    price numeric(10,2),
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    tags text[],
    thumbnail bytea,
    duration interval,
    inserted_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);

COPY public.users (id, email, admin) FROM stdin;
1	ada@example.com	t
2	bob@example.com	f
\.

COPY public.videos (id, user_id, title, status, price, settings, tags, thumbnail, duration, inserted_at) FROM stdin;
3f1c0a52-0000-4000-8000-000000000001	1	Ada's\t"first"\ttake	done	19.99	{"fps": 60}	{4k,"slow mo",NULL}	\\x00ff	00:01:30	2026-01-02 01:30:00+02
3f1c0a52-0000-4000-8000-000000000002	\N	Line\nbreak	uploaded	\N	{}	{}	\N	\N	2025-12-31 23:00:00-05:30
\.

SELECT pg_catalog.setval('public.users_id_seq', 2, true);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_key UNIQUE (email);

ALTER TABLE ONLY public.videos
    ADD CONSTRAINT videos_pkey PRIMARY KEY (id);

CREATE INDEX index_videos_on_user_id ON public.videos USING btree (user_id);

CREATE INDEX index_videos_on_lower_title ON public.videos USING btree (lower((title)::text));

CREATE TRIGGER videos_touch BEFORE UPDATE ON public.videos FOR EACH ROW EXECUTE FUNCTION public.touch();

ALTER TABLE ONLY public.videos
    ADD CONSTRAINT videos_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
"#;

#[test]
fn a_dump_becomes_sqlite_tables_rows_and_notes() {
    let out = convert(DUMP).unwrap();
    assert_eq!(
        out.schema,
        "CREATE TABLE users (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    email TEXT COLLATE NOCASE NOT NULL,\n    \
         admin INTEGER NOT NULL DEFAULT 0,\n    UNIQUE (email)\n);\n\n\
         CREATE TABLE videos (\n    id TEXT NOT NULL,\n    user_id INTEGER,\n    title TEXT NOT NULL,\n    \
         status TEXT NOT NULL DEFAULT 'uploaded' CHECK (status IN ('uploaded', 'processing', 'done')),\n    price TEXT,\n    \
         settings TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(settings)),\n    tags TEXT,\n    thumbnail BLOB,\n    \
         duration TEXT,\n    inserted_at TEXT NOT NULL DEFAULT (datetime('now')),\n    PRIMARY KEY (id),\n    \
         FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE\n);\n\n\
         CREATE INDEX index_videos_on_user_id ON videos (user_id);\n"
    );
    assert_eq!(
        out.data,
        "INSERT INTO users (id, email, admin) VALUES\n(1, 'ada@example.com', 1),\n(2, 'bob@example.com', 0);\n\
         INSERT INTO videos (id, user_id, title, status, price, settings, tags, thumbnail, duration, inserted_at) VALUES\n\
         ('3f1c0a52-0000-4000-8000-000000000001', 1, 'Ada''s\t\"first\"\ttake', 'done', '19.99', '{\"fps\": 60}', \
         '[\"4k\",\"slow mo\",null]', X'00ff', '00:01:30', '2026-01-01 23:30:00'),\n\
         ('3f1c0a52-0000-4000-8000-000000000002', NULL, 'Line\nbreak', 'uploaded', NULL, '{}', '[]', NULL, NULL, '2026-01-01 04:30:00');\n"
    );
    assert_eq!(out.rows, BTreeMap::from([("users".to_owned(), 2), ("videos".to_owned(), 2)]));
    for expected in [
        "users.email: citext -> TEXT COLLATE NOCASE (ASCII case only)",
        "videos.id: uuid -> TEXT; new rows need an id from the app",
        "videos.id: default `gen_random_uuid()` dropped (no SQLite equivalent): set it when inserting",
        "videos.status: enum video_status -> TEXT checked against its values",
        "videos.price: numeric(10,2) -> TEXT, exact",
        "videos.settings: jsonb -> TEXT checked with json_valid",
        "videos.tags: text[] -> TEXT holding a JSON array of strings",
        "videos.duration: interval has no SQLite equivalent -> TEXT",
        "videos.inserted_at: timestamp with time zone -> TEXT `YYYY-MM-DD HH:MM:SS`, converted to UTC",
        "index skipped (expression or method SQLite lacks): CREATE INDEX index_videos_on_lower_title",
        "1 functions skipped",
        "1 triggers skipped",
    ] {
        assert!(
            out.notes.iter().any(|note| note.starts_with(expected)),
            "missing note {expected:?} in {:#?}",
            out.notes
        );
    }
}

#[test]
fn large_tables_are_cut_into_inserts_d1_accepts() {
    let rows: String = (0..250).map(|n| format!("{n}\tx\n")).collect();
    let dump = format!(
        "CREATE TABLE public.t (\n    id integer,\n    s text\n);\nCOPY public.t (id, s) FROM stdin;\n{rows}\\.\n"
    );
    let out = convert(&dump).unwrap();
    assert_eq!(out.data.matches("INSERT INTO t (id, s) VALUES").count(), 3, "100 rows per INSERT");
    let wide = format!(
        "CREATE TABLE public.w (\n    s text\n);\nCOPY public.w (s) FROM stdin;\n{}\\.\n",
        format!("{}\n", "y".repeat(40_000)).repeat(5)
    );
    let out = convert(&wide).unwrap();
    assert_eq!(out.data.matches("INSERT INTO w").count(), 3, "at most 90 KB per INSERT");
}

#[test]
fn broken_or_partial_dumps_are_explained() {
    assert!(convert("SET x = 1;").unwrap_err().message.starts_with("no CREATE TABLE in the dump"));
    assert_eq!(convert("CREATE TABLE t;").unwrap_err().message, "cannot read `CREATE TABLE t`");
    assert_eq!(
        convert("CREATE TABLE t (a integer);\nCOPY t FROM stdin;\n\\.\n").unwrap_err().message,
        "cannot read `COPY t FROM stdin`"
    );
    let short = "CREATE TABLE t (a integer, b integer);\nCOPY t (a, b) FROM stdin;\n1\n\\.\n";
    assert_eq!(convert(short).unwrap_err().message, "t: row 1 has 1 values for 2 columns");
    let orphan = "CREATE TABLE t (a integer);\nCOPY u (a) FROM stdin;\n1\n\\.\nINSERT INTO public.t VALUES (1);\nDO $x$ BEGIN END $x$;\nALTER TABLE t ENABLE ROW LEVEL SECURITY;\nALTER TABLE missing ADD CONSTRAINT c UNIQUE (a);";
    let out = convert(orphan).unwrap();
    assert_eq!(out.data, "INSERT INTO t VALUES (1);\n");
    assert!(out.notes.contains(&"u: data skipped, no CREATE TABLE before it".to_owned()), "{:?}", out.notes);
    assert!(out.notes.iter().any(|note| note.starts_with("statement skipped: DO $x$")), "{:?}", out.notes);
    assert!(
        out.notes.iter().any(|note| note.starts_with("t: skipped `ALTER TABLE ... ENABLE ROW LEVEL SECURITY")),
        "{:?}",
        out.notes
    );
}

#[test]
fn copy_values_and_times_convert_exactly() {
    assert_eq!(unescape(r"a\\b\rc\bd\fe\vf\"), "a\\b\rc\u{8}d\u{c}e\u{b}f\\");
    assert_eq!(utc("2026-03-01 00:15:00.5+01:30").as_deref(), Some("2026-02-28 22:45:00.5"));
    assert_eq!(utc("2024-02-29 23:00:00-01").as_deref(), Some("2024-03-01 00:00:00"));
    assert_eq!(utc("2026-01-01"), None);
    assert_eq!(utc("2026-01-01 xx:00:00+00"), None);
    assert_eq!(literal("abc", Kind::Integer).unwrap(), "'abc'", "a value that is not a number stays text");
    assert_eq!(literal("1.5", Kind::Real).unwrap(), "1.5");
    assert_eq!(literal("plain", Kind::Bytes).unwrap(), "'plain'");
    assert_eq!(literal("bad", Kind::TimestampTz).unwrap(), "'bad'");
    assert_eq!(array_json(r#"{"a\"b","c\\d"}"#), r#"["a\"b","c\\d"]"#);
    assert_eq!(index("CREATE INDEX i ON public.t USING gin (tags);"), None);
    assert_eq!(index("CREATE UNIQUE INDEX i ON ONLY public.t USING btree (a DESC) WHERE (a > 0);"), None);
    assert_eq!(
        index("CREATE UNIQUE INDEX i ON public.t USING btree (a DESC, b);").as_deref(),
        Some("CREATE UNIQUE INDEX i ON t (a DESC, b);")
    );
    assert_eq!(first_line(&"x".repeat(120)), format!("{}...", "x".repeat(100)));
}

#[test]
fn hand_written_tables_keep_inline_keys_references_and_checks() {
    let dump = "CREATE TABLE IF NOT EXISTS public.orders (\n    id serial PRIMARY KEY,\n    \
                owner_id integer NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,\n    code text UNIQUE,\n    \
                placed timestamp without time zone,\n    total numeric DEFAULT 0 NOT NULL,\n    \
                CONSTRAINT positive CHECK (total >= 0),\n    UNIQUE (owner_id, code)\n);\nPREPARE q AS SELECT $1;\n\
                CREATE TABLE public.tags (\n    id integer DEFAULT nextval('public.tags_id_seq'::regclass) NOT NULL\n);\n\
                ALTER TABLE ONLY public.tags ADD CONSTRAINT tags_pkey PRIMARY KEY (id);\n";
    let out = convert(dump).unwrap();
    assert_eq!(
        out.schema,
        "CREATE TABLE orders (\n    id INTEGER PRIMARY KEY,\n    \
         owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,\n    code TEXT UNIQUE,\n    placed TEXT,\n    \
         total TEXT NOT NULL DEFAULT 0,\n    CHECK (total >= 0),\n    UNIQUE (owner_id, code)\n);\n\n\
         CREATE TABLE tags (\n    id INTEGER PRIMARY KEY AUTOINCREMENT\n);\n\n"
    );
    assert!(out.notes.iter().any(|note| note == "statement skipped: PREPARE q AS SELECT $1"), "{:?}", out.notes);
}
