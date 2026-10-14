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
await build({ entryPoints: { app: "src/style.css" }, outdir: "dist", minify: true });
await copyFile("src/index.html", "dist/index.html");
