import { build } from "esbuild";
import { copyFile, mkdir } from "node:fs/promises";

await mkdir("dist", { recursive: true });
await build({
  entryPoints: { app: "src/main.tsx" },
  outdir: "dist",
  bundle: true,
  minify: true,
  format: "iife",
  target: "es2022",
  jsx: "automatic",
  jsxImportSource: "preact",
  loader: { ".css": "css" },
  legalComments: "none",
});
// Fonts are emitted as /geist.woff2 and /geist-mono.woff2; src/server.rs embeds and serves them.
await build({
  entryPoints: { app: "src/style.css" },
  outdir: "dist",
  bundle: true,
  minify: true,
  loader: { ".woff2": "file" },
  assetNames: "[name]",
  publicPath: "/",
});
await copyFile("src/index.html", "dist/index.html");
