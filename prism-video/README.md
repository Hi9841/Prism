# Prism Video

Editable Remotion compositions for Prism product marketing.

## Preview

```powershell
npm run dev -- --no-open
```

Open the `PrismPromo-16x9` composition for the full 23.4-second launch video. Each scene is also registered separately under `Prism-Promo-Scenes` for focused editing.

The composition sidebar exposes the product name, tagline, call to action, and accent color. The scene timeline exposes named product windows, headlines, and key motion values for direct editing in Remotion Studio.

## Validate

```powershell
npm run lint
npm run still
```

## Render

Rendering is intentionally separate from previewing:

```powershell
npm run render
```

The MP4 is written to `out/prism-promo.mp4`.
