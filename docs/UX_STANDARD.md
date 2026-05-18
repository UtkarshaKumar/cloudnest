# iCloud Recovery Desktop UX Standard

## Product Intent

This app helps a stressed Mac user recover deleted iCloud Drive files and folders when iCloud.com becomes unreliable. The default experience should feel calm, local, and reversible. Technical details are available, but never lead the interface.

Working brand direction: calm recovery.

## Core User

The primary user is someone who wants deleted iCloud Drive content back. They may not know what an API, tombstone, batch, or checkpoint is. They do understand plain phases: sign in, find deleted items, review, restore, done.

## Flow

```text
Welcome
  -> Sign in with iCloud in Chrome
  -> Scan recently deleted iCloud Drive items
  -> Review restore summary
  -> Restore with progress and pause/cancel
  -> Completion / retry failed items
```

## Screens

### Welcome

Purpose: set trust and explain the app in one breath.

Primary copy:

- Title: `Recover deleted iCloud Drive files`
- Body: `Sign in with Apple in Chrome, scan recently deleted items, and restore them safely in batches.`
- Primary button: `Start Recovery`
- Secondary link: `How this keeps your data private`

### Sign In

Purpose: explain why Chrome opens and what the app does not see.

Primary copy:

- Title: `Sign in with Apple`
- Body: `A Chrome window will open for iCloud. Apple handles your password, Keychain, and two-factor code. This app only watches for the restore session needed to continue.`
- Primary button: `Open iCloud Sign In`
- Secondary button: `Cancel`

States:

- Waiting: `Waiting for iCloud sign-in...`
- Detected: `Sign-in detected. Preparing scan.`
- Timeout: `We could not detect a completed iCloud sign-in.`

### Scan

Purpose: find recoverable items without making the user watch a terminal log.

Primary copy:

- Title: `Finding deleted items`
- Body: `Scanning recently deleted iCloud Drive files and folders. Large accounts can take a few minutes.`
- Progress label: `Page {page} scanned. {count} items found.`
- Secondary button: `Cancel and Save Progress`

### Review

Purpose: confirm intent before restore.

Primary copy:

- Title: `{count} items ready to restore`
- Body: `They will be restored to iCloud Drive using Apple’s own recovery endpoint.`
- Primary button: `Restore {count} Items`
- Secondary buttons: `Back`, `Cancel`

### Restore

Purpose: show trustworthy progress while preserving calm.

Primary copy:

- Title: `Restoring your files`
- Status: `{restored} restored, {failed} need another try`
- ETA: `About {eta} remaining`
- Primary running button: `Pause After Current Batch`
- Secondary button: `Cancel and Save Progress`
- Details disclosure: `Details`

Behavior:

- The visible progress bar reflects completed plus failed items over total items.
- Retry messages stay inline, not as blocking alerts.
- The details log is append-only, collapsible, and copyable.

### Done

Purpose: end with clarity and a next action if needed.

Success copy:

- Title: `Recovery complete`
- Body: `{restored} items were restored to iCloud Drive.`
- Primary button: `Done`

Partial success copy:

- Title: `Mostly recovered`
- Body: `{restored} items were restored. {failed} items need another try.`
- Primary button: `Retry Failed Items`
- Secondary button: `Done`

## Button Behavior

- Primary actions are disabled until all prerequisites are met.
- Starting a long operation immediately disables duplicate triggers.
- `Cancel and Save Progress` stops new work, preserves checkpoint/progress data, and returns to a resumable state.
- `Pause After Current Batch` lets in-flight network calls settle before pausing.
- `Retry Failed Items` appears only when failed item IDs exist.
- Destructive styling is reserved for abandoning progress, not for restore.

## Message System

Messages use plain language and a clear next step.

| Situation | Message | Actions |
| --- | --- | --- |
| Empty trash | `No recently deleted iCloud Drive items were found.` | `Done`, `Scan Again` |
| Chrome missing | `Chrome is needed for secure Apple sign-in. Install Chrome, then try again.` | `Try Again`, `Open Chrome Download` |
| Chrome launch failed | `Chrome did not open. Close any stuck Chrome windows, then try again.` | `Try Again`, `Cancel` |
| Login timeout | `We could not detect a completed iCloud sign-in.` | `Keep Waiting`, `Restart Sign In`, `Cancel` |
| Auth expired | `iCloud needs you to sign in again. Your restore is paused and progress is saved.` | `Resume After Sign In` |
| Network retry | `iCloud is taking longer than usual. Retrying this batch automatically.` | None |
| Corrupt progress | `Saved progress could not be read. You can start over without affecting your iCloud files.` | `Start Over`, `Reveal File` |
| Partial success | `Restored {restored} items. {failed} items need another try.` | `Retry Failed Items`, `Done` |
| Success | `Recovery complete. {restored} items were restored to iCloud Drive.` | `Done` |

## Visual Direction

- Minimal white or warm-gray canvas.
- Soft blue/teal action color.
- Restrained red for blocking failures only.
- Rounded cards, spacious rhythm, no dense tables on the happy path.
- Font: Google `Instrument Sans`, bundled locally through the app build.
- Icons: Google `Material Symbols Rounded`, bundled locally through the app build.

## Privacy Copy

Use this explanation consistently:

`Your Apple ID password and two-factor code stay inside Apple’s iCloud sign-in page. Recovery progress is saved locally on this Mac. Session credentials are kept in memory only and are not written to disk.`
