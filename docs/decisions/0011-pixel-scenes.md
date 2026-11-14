# ADR-0011 — Main screens drawn in pixels over the grid

- Status: accepted (revises ADR-0008)
- Date: 2026-11-14

## Context

ADR-0008 put the M5's interface on Ratatui: every screen is a grid of monospace cells, with pixel overlays only for big digits, QR codes and later the map. The result works but looks like a terminal:
- no icons and no proportional text;
- lists show names and counts but no previews;
- conversations are plain text, with nothing to tell yours from theirs;
- the compact grid has no bold font.

Tanguy asked for a better-looking interface and chose, on mockups drawn at the panel's real resolution, to draw the main screens in pixels. Settings and tools stay as they are.

## Decision

- **A screen may return a scene** (`mc_ui::scene::Scene`): a list of pixel primitives. These are text in proportional fonts, icons, boxes with rounded corners, circles, lines and a dither. The app paints it over the screen's body on the panel, in cells it marks skipped, as it already did for the clock and QR codes.
- **The grid version stays.** It is what the terminal emulator (`mc ui`) and the text snapshots show. Gestures and navigation do not change.
- **The header and footer of every screen are scenes on the panel**: a black bar with icons, and key caps with the hints.
- **Pixel screens:** Home, the message list, conversations, compose, the new-message banner and the contact card (U10, U11).
- **Fonts and icons come from U8g2 through `u8g2-fonts`:**
  - Helvetica from the X11 fonts, under the Adobe/DEC licence;
  - Spleen 16×32 for big digits, BSD-2-Clause;
  - Open Iconic for icons, SIL OFL.

  They are credited in `NOTICE`; no GPL font is used (ADR-0007).
- **Text is laid out by our own code** (`scene::width`, `wrap`, `ellipsize`). `embedded-text` was the other choice; one of its dependencies is not available offline here.
- **Scenes are data**, so tests can read them: their texts and items, and pixel frames through `mc ui --png`.

## Consequences

- Each pixel screen has two renderings, the grid and the scene, built from the same data. Navigation and their tests are unaffected.
- The fonts add to the image size (each Helvetica size is a few KB). Heap use per frame is small: a scene is a short list of items.
- ADR-0008's budget, measured on the grid, has to be checked again once the pixel screens land (U10).
- Rejected: dropping Ratatui. Settings, tools and the terminal emulator rely on it, and they work.
- Rejected: our own bitmap fonts. Drawing and maintaining them would cost more than U8g2's permissive ones.
