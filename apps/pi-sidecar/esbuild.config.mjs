import * as esbuild from 'esbuild';

await esbuild.build({
  entryPoints: ['src/main.ts'],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  outfile: 'dist/pi-sidecar.bundle.js',
  sourcemap: false,
  minify: false,
  banner: {
    js: '// @busytok/pi-sidecar — auto-generated bundle. Do not edit.',
  },
});

console.log('Built dist/pi-sidecar.bundle.js');
