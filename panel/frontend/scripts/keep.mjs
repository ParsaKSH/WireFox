// vite empties the output directory on every build; put back the placeholder
// that keeps the directory in git so the Go embed compiles without a build.
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

writeFileSync(fileURLToPath(new URL("../../internal/web/dist/.gitkeep", import.meta.url)), "");
