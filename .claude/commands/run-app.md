Run the Electron desktop app in dev mode.

Steps:
1. `cd` to the `app/` directory
2. Run `env -u ELECTRON_RUN_AS_NODE npm run dev` in the background (ELECTRON_RUN_AS_NODE must be unset or Electron runs as plain Node.js)
3. Confirm the dev server and Electron process started successfully
