Publish a new version of the Electron desktop app to GitHub Releases.

Steps:
1. Read the current version from `app/package.json`
2. Ask the user what the new version should be (suggest patch, minor, major bumps)
3. Update the `version` field in `app/package.json`
4. Commit the version bump: `chore: bump app version to <version>`
5. Create a git tag: `app-v<version>`
6. Push the commit and tag to origin: `git push origin main && git push origin app-v<version>`
7. Confirm the tag was pushed — GitHub Actions (.github/workflows/build-app.yml) will automatically build macOS/Windows/Linux binaries and publish them to GitHub Releases

Notes:
- The tag MUST match the `app-v*` pattern to trigger the CI workflow
- macOS code signing requires `CSC_LINK` and `CSC_KEY_PASSWORD` secrets configured in the repo
- Do NOT push without user confirmation
