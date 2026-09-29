// How the Rust code is built and where the static files are, read by cf
// (`cf dev`, `cf deploy`) through the app's wrangler. Bindings and triggers
// live in cloudflare.config.ts.
import { defineWranglerConfig } from "wrangler/experimental-config";

export default defineWranglerConfig({
	// `ocre dev` sets OCRE_BUILD=--dev (fast, unoptimized); deploys build --release.
	build: { command: 'cargo install -q "worker-build@^0.8" && worker-build ${OCRE_BUILD:---release}' },
	assetsDirectory: "public",
	types: { generate: false },
});
