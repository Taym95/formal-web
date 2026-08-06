#!/usr/bin/env node
/**
 * Minimal static file server for the React todo app.
 *
 * - Serves the project root directory (index.html + dist/app.js)
 * - Adds permissive CORS headers so the app can also be embedded
 *   from file:// pages (e.g. the startup demo page)
 * - Logs every request to stdout and to requests.log so that
 *   navigation from other webviews (popups, iframes) is observable
 *
 * Usage: node serve.mjs [port]   (default port 8080)
 */
import { createServer } from "node:http";
import { readFile, appendFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL(".", import.meta.url));
const PORT = Number.parseInt(process.argv[2] ?? process.env.PORT ?? "8080", 10);
const LOG_FILE = join(ROOT, "requests.log");

const MIME_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
};

async function logRequest(request, status, note = "") {
  const line = `${new Date().toISOString()} ${request.method} ${request.url} -> ${status}${note ? " " + note : ""}`;
  console.log(line);
  try {
    await appendFile(LOG_FILE, line + "\n");
  } catch {
    // Log file is best-effort only.
  }
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? "localhost"}`);
  response.setHeader("Access-Control-Allow-Origin", "*");
  response.setHeader("Cache-Control", "no-store");

  if (url.pathname === "/health") {
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(JSON.stringify({ ok: true, app: "react-todo", port: PORT }));
    logRequest(request, 200, "(health)");
    return;
  }

  let pathName = url.pathname === "/" ? "/index.html" : url.pathname;
  const filePath = normalize(join(ROOT, pathName));
  if (!filePath.startsWith(ROOT)) {
    response.writeHead(403);
    response.end("Forbidden");
    logRequest(request, 403);
    return;
  }

  try {
    const body = await readFile(filePath);
    response.writeHead(200, {
      "Content-Type": MIME_TYPES[extname(filePath)] ?? "application/octet-stream",
    });
    response.end(body);
    logRequest(request, 200);
  } catch {
    response.writeHead(404);
    response.end("Not found");
    logRequest(request, 404);
  }
});

server.listen(PORT, () => {
  console.log(`react-todo server listening on http://localhost:${PORT}`);
  console.log(`serving ${ROOT}`);
});
