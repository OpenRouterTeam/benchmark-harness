import * as esbuild from 'esbuild'
import { fileURLToPath } from 'node:url'
const stub = fileURLToPath(new URL('./stub-workers.mjs', import.meta.url))
await esbuild.build({
  entryPoints: ['entry.ts'],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node24',
  outfile: 'capture.mjs',
  banner: { js: "import { createRequire } from 'node:module'; const require = createRequire(import.meta.url);" },
  plugins: [{ name: 'cf', setup: (b) => b.onResolve({ filter: /^cloudflare:/ }, () => ({ path: stub })) }]
})
