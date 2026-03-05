Build the Electron desktop app locally without publishing.

Steps:
1. Ask the user which platform to build for: macOS, Windows, Linux, or all
2. `cd` to the `app/` directory
3. Run `npm ci` if `node_modules/` looks stale or missing
4. Run the appropriate build command:
   - macOS: `npm run build:mac`
   - Windows: `npm run build:win`
   - Linux: `npm run build:linux`
   - All: `npm run build:all`
5. Report the output artifacts in `app/dist/`

Notes:
- This does NOT push or publish anything — output stays in `app/dist/`
- Cross-platform builds may not work (e.g., building Windows on macOS requires Wine)
- macOS code signing requires `CSC_LINK` and `CSC_KEY_PASSWORD` environment variables for a signed build; without them, the build will be unsigned
