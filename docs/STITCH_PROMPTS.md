# Google Stitch Mockup Prompts

Use these prompts in Google Stitch after the app shell is ready. The goal is a calm macOS utility, not a dashboard.

## Prompt 1: Welcome And Sign-In

Design a minimalist macOS desktop app screen for a cute utility called "CloudNest" that helps users recover deleted iCloud Drive files. Calm, trustworthy, privacy-first. White warm-gray background, centered rounded card, soft blue primary button, subtle cloud nest/folder restore icon, Google Font Inter or Instrument Sans. Screen includes title "Recover deleted iCloud Drive files", supporting copy "CloudNest gently finds recently deleted items and brings them back home safely in batches.", primary button "Start Recovery", secondary link "How this keeps your data private". Native Mac spacing, no dark terminal styling, no clutter.

## Prompt 2: Recovery Stepper

Design a macOS utility workflow screen for iCloud deleted file recovery. A left vertical stepper with five steps: Start, Sign In, Scan, Review, Restore. Main card shows the Sign In step. Copy explains that Chrome opens for Apple sign-in and that Apple handles password, Keychain, and two-factor code. Include primary button "Open iCloud Sign In" and secondary "Cancel". Calm blue/teal accent, warm white surfaces, rounded 16px cards, Google Material Symbols style icons, Inter or Instrument Sans.

## Prompt 3: Restore Progress

Design a calm restore progress screen for a macOS app recovering deleted iCloud Drive files. Large headline "Restoring your files", progress bar at 62%, stats cards for Restored, Need another try, ETA. Include soft status message "iCloud is taking longer than usual. Retrying this batch automatically." Include buttons "Pause After Current Batch" and "Cancel and Save Progress". Add collapsed "Details" row for technical log. Minimal, reassuring, no terminal-like wall of text, soft blue accent, accessible contrast.

## Prompt 4: Partial Success

Design a completion screen for a calm iCloud recovery app. Headline "Mostly recovered", body "{restored} items were restored. {failed} items need another try." Include primary button "Retry Failed Items", secondary "Done", and a small details link "View restore details". Use soft success green for the restored count, muted warning amber for failed count, warm white background, rounded card, Inter or Instrument Sans, Google Material Symbols icons.

## Font Direction

Start with:

- `Inter` for pragmatic Mac utility readability.
- `Instrument Sans` if the mockups need more editorial softness.

Bundling rule: use local `@font-face` assets before release. Do not load fonts from Google CDN at runtime.

## Icon Direction

Use Material Symbols concepts:

- `cloud_sync` for recovery in progress.
- `lock` for privacy.
- `folder_open` for scan/review.
- `check_circle` for success.
- `error` or `warning` only for blocking or partial failures.

Bundling rule: export icons as local SVGs before release. Do not load icons from Google CDN at runtime.

## Design Tokens Draft

```css
:root {
  --font-sans: "Inter", -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif;
  --color-canvas: #f7f5f1;
  --color-surface: #fffdf9;
  --color-surface-muted: #f0ede7;
  --color-text: #1f2933;
  --color-text-muted: #68717d;
  --color-border: #ded8cf;
  --color-action: #2f80ed;
  --color-action-hover: #256fd0;
  --color-success: #2f855a;
  --color-warning: #b7791f;
  --color-error: #c2413a;
  --radius-card: 18px;
  --radius-control: 12px;
  --shadow-card: 0 24px 70px rgba(31, 41, 51, 0.12);
}
```
