// Ambient declarations for node-only test utilities.
// The tokens contract test reads tokens.css at runtime via node:fs so the
// CSS source itself (not a transformed JS module) can be asserted against.
declare module "node:fs" {
  export function readFileSync(path: string | URL, encoding: string): string;
}
declare module "node:url" {
  export function pathToFileURL(path: string): URL;
}
