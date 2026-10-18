# Blog articles

One directory per article, `content/blog/<slug>/` (the slug is the URL:
`/blog/<slug>`; `a-z`, `0-9`, `-`), with one Markdown file per locale of the
app: `<locale>.md` for each `locales/<locale>.yml` (`__DEFAULT_LOCALE__.md`
alone without translations). Every file is required.

```markdown
---
title: The article's title, also the page's <h1>
description: One or two sentences for search results, about 150 characters.
published: 2026-10-18
updated: 2026-10-20
---
The introduction (a paragraph or two), then sections.

## A section

Text, **bold**, [links](/pricing), lists, tables, `code`, > quotes,
![images](/images/blog/<slug>/<name>.webp) (files in the assets directory).
```

- `published` (and `updated`, when the article changes) go in
  `__DEFAULT_LOCALE__.md` only.
- Sections start at `## `: the title is the page's `<h1>`, a `# ` fails the build.
- Raw HTML is shown as text, not markup.

build.rs compiles the files at build time: a missing locale, field or date
fails `cargo check`, and pages parse nothing at request time.
