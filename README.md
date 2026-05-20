# CloudNest

Recover accidentally deleted iCloud Drive files and folders in bulk — something Apple's own recovery tool fails at nearly every time. Restores items back to their original folder structure.

![CloudNest welcome screen](docs/assets/cloudnest-welcome.png)

## Install

Download the latest DMG from [Releases](https://github.com/UtkarshaKumar/cloudnest/releases), open it, and drag CloudNest to your Applications folder.

The app is ad-hoc signed and not notarized, so macOS will quarantine it on first run. Clear the quarantine before launching:

```bash
xattr -cr /Applications/CloudNest.app
```

## What It Does

- Opens Chrome for Apple-managed iCloud sign-in — your credentials stay inside Apple's login page.
- Scans your recently deleted iCloud Drive items in batches.
- Restores everything back to where it was, with retry and resume if the connection drops.
- Keeps session credentials in memory only. Nothing is written to disk.

## Privacy

Your Apple ID password and two-factor code are entered inside Apple's own iCloud sign-in page. Session cookies are held in memory during recovery and never persisted. There is no server, no telemetry, no analytics.

## Build from Source

Prefer not to run a downloaded binary? Build it yourself:

```bash
npm install
npm run tauri -- build
```

Find the DMG under `src-tauri/target/release/bundle/dmg/`.

Requires: Node.js, Rust, and Xcode Command Line Tools.

## License

The app is free to download and use. The source code is not licensed for reuse or redistribution.
