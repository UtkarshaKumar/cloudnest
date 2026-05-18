# App Store Readiness

CloudNest is designed to become store-ready, but every release must pass this checklist before submission.

## Current Status

- Distribution today: GitHub DMG, ad-hoc signed.
- Store status: not submitted.
- Localization baseline: `src/i18n/en.js`; all user-facing copy should flow through message IDs.
- Privacy baseline: Apple ID password and 2FA stay inside iCloud sign-in; session credentials stay in memory; restore progress is stored locally.

## Required Before Any Store Submission

### Identity And Signing

- Use a real Apple Developer signing identity, not ad-hoc `"-"`.
- Enable hardened runtime for notarized direct distribution.
- For Mac App Store, review sandbox entitlements and file/network access.
- Notarize non-App-Store DMG builds.

### Privacy

- Publish a privacy policy URL before submission.
- Explain:
  - what data is processed locally
  - what iCloud session data is held in memory
  - what progress is written to disk
  - that CloudNest is not affiliated with Apple
- Do not log cookies, Apple IDs, DSIDs, or raw iCloud URLs.

### Localization

- Keep `en` complete.
- Add new languages by adding catalog files, not by editing screen code.
- Localize every button, status, aria label, log line, and user-facing error.
- Screenshot and metadata copy must match the app locale.

### UX And Branding

- Do not use Apple logos, San Francisco assets, or copy that implies Apple endorsement.
- Keep iCloud references factual and compatibility-oriented.
- Use failure states that are calm but explicit about limits and partial recovery.

### Technical Review

- Run:

```bash
npm run build
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings
cd .. && npm audit --omit=dev
npm run tauri -- build
```

- Re-run the workspace `security-testing` skill before submission.
- Verify app launch, sign-in, scan, pause, resume, retry, and reset on a clean macOS user account.

## Release Blockers

Do not submit to any app store if:

- user-facing English is hardcoded in primary code
- credentials or raw cookies can reach logs, disk, screenshots, or UI errors
- cancellation can leave a Chrome child process or temp profile behind
- restore progress can be lost silently
- signing/notarization/sandbox status is unknown
