import * as esbuild from 'esbuild';

await esbuild.build({
  entryPoints: ['src/spike.ts'],
  bundle: true,
  platform: 'node',
  target: 'node22',
  format: 'esm',
  outfile: 'dist/spike.bundle.js',
  // Pi SDK may ship native deps or dynamic imports; mark them external
  // so the spike focuses on whether the SDK's public API bundles.
  external: [],
  logLevel: 'info',
});

console.log('spike bundle written to dist/spike.bundle.js');
