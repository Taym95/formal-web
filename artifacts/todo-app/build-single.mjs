#!/usr/bin/env node
/**
 * Build the React todo app.
 *
 * - Bundles src/main.jsx into dist/app.js (for the dev server)
 * - Emits todo.html: the app as a single self-contained HTML file with the
 *   bundle inlined, openable directly from file:// with no server.
 */
import { build } from "esbuild";
import { readFile, writeFile } from "node:fs/promises";

const outfile = "dist/app.js";
await build({
  entryPoints: ["src/main.jsx"],
  bundle: true,
  minify: true,
  format: "iife",
  jsx: "automatic",
  define: { "process.env.NODE_ENV": '"production"' },
  outfile,
});

const html = await readFile("index.html", "utf8");
const js = await readFile(outfile, "utf8");
// Escape `</script` inside the bundle so it cannot terminate the inline
// <script> element early (React DOM emits this sequence in a string), and
// inline via a replacement FUNCTION — a replacement string would have its
// `$` sequences (e.g. React's "$&/") interpreted as replacement patterns.
const escaped = js.replace(/<\/script/g, "<\\/script");
const single = html.replace(
  '<script src="dist/app.js"></script>',
  () => `<script>\n${escaped}\n</script>`
);
await writeFile("todo.html", single);
console.log("wrote dist/app.js and todo.html");
