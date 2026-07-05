import * as esbuild from 'esbuild';

// CJS format is required because the Pi SDK's transitive dep `cross-spawn`
// uses `require('child_process')`, which throws "Dynamic require of
// 'child_process' is not supported" under an ESM bundle (proven by the
// Plan 4 spike). CJS lets cross-spawn's require resolve natively.
await esbuild.build({
  entryPoints: ['src/main.ts'],
  bundle: true,
  platform: 'node',
  format: 'cjs',
  target: 'node22',
  outfile: 'dist/pi-sidecar.bundle.js',
  sourcemap: false,
  minify: false,
  external: ['@earendil-works/pi-coding-agent'],
  banner: {
    js: '// @busytok/pi-sidecar — auto-generated CJS bundle. Do not edit.',
  },
});

console.log('Built dist/pi-sidecar.bundle.js');
