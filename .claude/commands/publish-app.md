Publish a new version of the Electron desktop app (Tesseract) to GitHub Releases.

The app lives in the `app/` git submodule, which is its OWN repository: `github.com/geckse/tesseract-md-app`. Its release workflow (`app/.github/workflows/build-app.yml`) triggers on `v*` tags pushed to THAT repo. Tags in the parent markdown-vdb repo (including the old `app-v*` scheme) do NOT trigger it.

Steps:

1. Preflight — the app working tree must be clean:
   - `git -C app status --porcelain` must produce EMPTY output. If it prints anything, ABORT and tell the user to commit or stash their in-progress app work first.
   - `git -C app branch --show-current` must print `main` (submodules are often in detached-HEAD state; committing there would strand the release commit). If not on `main`, ABORT and ask the user.
2. Read the current version from `app/package.json` and ask the user what the new version should be (suggest patch, minor, major bumps).
3. Update the `version` field in `app/package.json`.
4. Commit INSIDE the app submodule:
   - `git -C app add package.json`
   - `git -C app commit -m "chore: bump version to <version>"`
5. Tag `v<version>` in the app repo and push branch + tag to the app's origin (`github.com/geckse/tesseract-md-app`):
   - `git -C app tag v<version>`
   - `git -C app push origin main`
   - `git -C app push origin v<version>`
   That repo's GitHub Actions builds mac (signed + notarized), win (unsigned for beta), and linux, and publishes a DRAFT GitHub release.
6. Wait for all three matrix jobs to succeed (watch with `gh run list -R geckse/tesseract-md-app --workflow build-app.yml`), then publish the draft:
   - `gh release edit v<version> -R geckse/tesseract-md-app --draft=false --generate-notes`
7. In the parent markdown-vdb repo, record the new submodule commit:
   - `git add app`
   - `git commit -m "chore: bump app submodule to v<version>"`
   - `git push`

Notes:
- The tag MUST match the `v*` pattern and MUST be pushed to the APP repo (`geckse/tesseract-md-app`) to trigger the CI workflow. Do not tag the parent repo.
- macOS signing + notarization use the `CSC_LINK`, `CSC_KEY_PASSWORD`, `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`, `APPLE_TEAM_ID` secrets configured in geckse/tesseract-md-app (mac job only). Windows ships unsigned for beta.
- Do NOT push without user confirmation
