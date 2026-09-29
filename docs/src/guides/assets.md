# Assets: CSS, JavaScript and images

Files in an Ocre app's `public/` directory are served by [Workers Static Assets](https://developers.cloudflare.com/workers/static-assets/): Cloudflare answers them from its edge before the Worker runs, so they cost no Worker request, no CPU and no free-plan quota. There is no asset pipeline to run: a file in `public/` is deployed as it is by `ocre deploy`, and build tools (Tailwind, esbuild) are optional steps that write into `public/`. This page covers serving, caching, CSS and JavaScript tooling, and single-page apps.

## Before you start

- An app from `ocre new`. Its `wrangler.config.ts` has:

  ```ts
  assetsDirectory: "public",
  ```

- Free plan (September 2026): static asset requests are free and unlimited, and do not count toward the Worker's 100,000 requests a day. Each file can be up to 25 MiB, and a Worker version up to 20,000 files ([limits](https://developers.cloudflare.com/workers/platform/limits/#static-assets)).

## Serving files

`public/robots.txt` is `/robots.txt`, `public/css/app.css` is `/css/app.css`. A file wins over a route with the same path, and requests no file matches go to the Worker. Reference files by their path in templates:

```html
<link rel="stylesheet" href="/css/app.css">
<script src="/js/app.js" defer></script>
<link rel="icon" href="/favicon.ico">
<img src="/images/logo.svg" alt="Logo" width="120" height="40">
```

These are Rails' `stylesheet_link_tag`, `javascript_include_tag`, `favicon_link_tag` and `image_tag`: templates are HTML, and there is no digest to resolve (see Caching below). `ocre dev` serves `public/` the same way, uncached, and picks up changes without a rebuild.

Uploaded files (avatars, attachments) do not belong in `public/`: they go to R2 (see [File storage](files.md)). Static asset responses do not pass through the Worker, so they do not get Ocre's security headers; add headers to them with `_headers` below.

## Caching

By default Cloudflare serves every static file with `Cache-Control: public, max-age=0, must-revalidate` and an `ETag` (a hash of the file). Browsers keep the file and revalidate it on each use; Cloudflare answers `304 Not Modified` from its edge while the file is unchanged, and the new version as soon as you deploy. That is why Ocre needs no fingerprinted file names (Rails' digests and `.manifest.json`): a deploy is never hidden by a stale cache, and a revalidation is a small free request.

For files that never change under a given name (vendored libraries with a version in the path, fonts), long caching saves even the revalidation. A `public/_headers` file sets headers per path; it is not served itself:

```text
/vendor/*
  Cache-Control: public, max-age=31536000, immutable

/fonts/*
  Cache-Control: public, max-age=31536000, immutable
  Access-Control-Allow-Origin: *
```

Put a version in those paths (`/vendor/chart-4.4.1.min.js`) so a new version gets a new URL. Cloudflare compresses text files (HTML, CSS, JavaScript, JSON, SVG) with Brotli or gzip on its own, for static files and Worker responses alike; there is no compression to configure. Cloudflare is also the CDN: there is no `asset_host` to set.

## CSS

Plain CSS in `public/` needs no tooling. The generated layout keeps its few rules in a `<style>` block; move them to `public/css/app.css` as they grow.

Tailwind CSS runs without Node through its standalone CLI (Rails' `tailwindcss-rails`). Download the binary for your platform from the [Tailwind releases](https://github.com/tailwindlabs/tailwindcss/releases), then:

```css
/* assets/app.css */
@import "tailwindcss";
@source "../templates";
```

```sh
./tailwindcss -i assets/app.css -o public/css/app.css --minify           # once
./tailwindcss -i assets/app.css -o public/css/app.css --watch            # while `ocre dev` runs
```

`@source "../templates"` makes Tailwind scan the askama templates for class names. To build CSS on every build, chain the command before the Worker build in `wrangler.config.ts`:

```ts
build: { command: './tailwindcss -i assets/app.css -o public/css/app.css --minify && cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}' },
```

Sass, PostCSS or Bootstrap builds (Rails' `cssbundling-rails`) work the same way: any command that writes into `public/`, chained in `build.command`. Commit the tool's configuration, not its output, if you build on deploy; commit the output if you would rather not install the tool on every machine.

## JavaScript

htmx covers most interactivity with no JavaScript of your own (see [htmx](htmx.md)). The layout loads it from unpkg; to serve it yourself, download `htmx.min.js` into `public/vendor/htmx-2.0.4.min.js` and change the `<script src>`.

For your own scripts, plain ES modules need no bundler. An import map (Rails' `importmap-rails`) maps bare names to files in `public/`:

```html
<script type="importmap" nonce="{{ nonce }}">
{ "imports": { "chart.js": "/vendor/chart-4.4.1.js", "app": "/js/app.js" } }
</script>
<script type="module" src="/js/main.js"></script>
```

The generated Content-Security-Policy (`content_security_policy()` in `src/lib.rs`) allows scripts only from the app and `https://unpkg.com`, and no inline scripts, which includes inline import maps. Put the module entry point in a file (`/js/main.js` above, which starts with `import "app";`), and for the import map add `NONCE` to `script_src` and pass `nonce: ocre::security::CspNonce` from the handler to the template, as the page's `nonce` field. A CDN other than unpkg goes into `script_src` too (see [Security](security.md)). To bundle npm packages (Rails' `jsbundling-rails`), run esbuild, Bun or Rollup in `build.command` of `wrangler.config.ts`, writing into `public/js/` (add the bundler to `package.json` with `npm install --save-dev esbuild`):

```ts
build: { command: 'npx esbuild assets/js/app.js --bundle --minify --outfile=public/js/app.js && cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}' },
```

## Single-page apps

An app whose front end is a single-page app (React, Vue, Svelte) builds it into `public/`, serves JSON from the Worker (see [JSON APIs](json-apis.md)), and lets client-side routes survive a reload with Cloudflare's SPA fallback: every navigation request no file matches gets `public/index.html`.

```ts
// cloudflare.config.ts, in worker
assets: { notFoundHandling: "single-page-application" },
```

Requests to the API still reach the Worker: they are not navigations (`fetch()` sends `Sec-Fetch-Mode: cors`). Ocre's generators produce htmx pages, not SPA code.

## Rails comparison

| Rails | Ocre |
|---|---|
| `public/` files | `public/`, served by Cloudflare before the Worker |
| Propshaft load paths | one directory, `public/` |
| Digests, `.manifest.json`, `assets:precompile` | not needed: ETag revalidation after each deploy; `_headers` for long caching of versioned paths |
| `asset_host` / CDN | Cloudflare's edge |
| `stylesheet_link_tag`, `javascript_include_tag`, `image_tag` | `<link>`, `<script>`, `<img>` with the path |
| `tailwindcss-rails`, `cssbundling-rails`, `jsbundling-rails` | the tool's CLI in `[build] command` |
| `importmap-rails` | an `<script type="importmap">` |
| Development serving | `ocre dev` serves `public/` uncached |

## See also

- [Views, helpers and forms](views.md): templates and the layout
- [htmx](htmx.md): interactivity without writing JavaScript
- [Security](security.md): Content Security Policy for scripts and styles
- [Deployment](deployment.md): what `ocre deploy` uploads
