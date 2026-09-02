# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Prism is for Windows 10 and Windows 11 users who want to open applications, files, folders, and frequent locations without moving through the Start menu or File Explorer. Its primary audience is comfortable with keyboard shortcuts but should not need technical knowledge to install or use it.

## Product Purpose

Prism is a fast, keyboard-first command palette and Windows taskbar companion. It puts local search, path browsing, calculations, pinned applications, Quick Access folders, and taskbar controls in one surface that can open from anywhere.

Success means a visitor understands the product within the first viewport, sees the real interface in use, and can reach the latest Windows installer without ambiguity.

## Positioning

Prism combines a local Windows launcher with direct taskbar customization. It does not require StartAllBack and does not send search queries to a hosted service.

## Operating Context

Prism runs on Windows 10 and Windows 11 x64. People open it with the Windows key or a configured global shortcut, type a query, move through results with the arrow keys, and press Enter to open or copy the selected result. It starts hidden at sign-in so the shortcut is ready.

## Capabilities and Constraints

- Search installed desktop and packaged Windows applications.
- Search local files and folders with fuzzy matching and direct path browsing.
- Calculate expressions and copy results.
- Pin and reorder applications and keep up to six Quick Access folders.
- Run eligible local applications and scripts as administrator.
- Control taskbar alignment, icon density, button grouping, auto-hide, and the Start button icon.
- Follow the Windows theme or use light, dark, acrylic, mica, or solid appearances.
- Install signed in-app updates.
- The current installer is not Windows Authenticode-signed, so Windows SmartScreen may show a warning.
- The installer targets Windows x64, installs for the current user, and does not require administrator access.

## Brand Commitments

The product name is Prism. The existing violet prism icon, near-black interface, Segoe-based typography, precise Windows controls, and calm direct voice are established brand assets. Product claims remain factual and compact.

## Evidence on Hand

- Product behavior and installation facts in `README.md`.
- Current interface implementation and tokens in `src/`.
- Application icon in `prism-video/public/prism-icon.png` and `src-tauri/icons/`.
- Editable product promo and rendered still frames in `prism-video/`.
- Public demo attachment linked from `README.md`.
- GitHub Releases provides the current installer and release history.
- No testimonials, usage metrics, customer logos, or press quotes are available and none should be fabricated.

## Product Principles

- Keep the keyboard path short.
- Search local content locally.
- Make Windows controls direct and reversible.
- Show the real product before asking for a download.
- State platform and installer limitations plainly.

## Accessibility & Inclusion

The landing page must support keyboard navigation, visible focus, semantic landmarks, useful alternative text, reduced motion, sufficient color contrast, and responsive layouts from small phones through wide desktop screens.
