#!/usr/bin/env bash
# Regenerates the dependency map and migration/manifest.tsv from the original app.
# Naming rules: migration/RULEBOOK.md section 4. Tests are not translated units.
set -euo pipefail
cd "$(dirname "$0")/.."
KIT="${MIGRATION_KIT:-C:/Users/hi/Desktop/skills/migration-kit}"
bun migration/depmap_ts.mjs --root .. --out migration/depmap
grep -v '^#' migration/depmap/order.txt | tr '\t' '\n' | grep -vE '\.test\.(ts|tsx)$' > migration/depmap/order.port.txt
# MSYS_NO_PATHCONV: Git Bash would rewrite the '/Name=...' arguments as Windows paths.
MSYS_NO_PATHCONV=1 python "$KIT/scripts/make_manifest.py" \
  --order migration/depmap/order.port.txt --out migration/manifest.ui.tsv \
  --sub 'package.json=crates/prism-app/Cargo.toml' \
  --sub 'src/=crates/prism-ui/src/' \
  --sub 'crates/prism-ui/src/assets/=crates/prism-ui/assets/' \
  --sub 'crates/prism-ui/src/styles/tokens.css=crates/prism-ui/src/theme.rs' \
  --sub 'crates/prism-ui/src/main.tsx=crates/prism-app/src/main.rs' \
  --sub 'crates/prism-ui/src/App.tsx=crates/prism-ui/src/app.rs' \
  --sub 'PaletteSearchInput=palette_search_input' --sub '/Palette.tsx=/palette.rs' \
  --sub 'ResultContextMenu=result_context_menu' --sub 'SettingsSheet=settings_sheet' \
  --sub 'TaskbarCustomization=taskbar_customization' --sub 'PowerMenu=power_menu' --sub '/Toast.tsx=/toast.rs' \
  --sub 'UpdateControl=update_control' --sub 'update-policy=update_policy' --sub 'update-progress=update_progress' \
  --sub 'iconLoading=icon_loading' --sub 'launcherMotion=launcher_motion' --sub 'transientUi=transient_ui' \
  --sub '.tsx=.rs' --sub '.ts=.rs'
# Subsystem 0 (RULEBOOK §5, A4): prism-core extraction and the lib.rs command bodies come
# first. The TS dependency map cannot see them, so migration/backend-units.tsv lists them.
# LF only: queue_runner.mjs rejects a CRLF header.
{ head -n 1 migration/manifest.ui.tsv; tail -n +2 migration/backend-units.tsv; tail -n +2 migration/manifest.ui.tsv; } \
  | tr -d '\r' > migration/manifest.tsv
rm migration/manifest.ui.tsv
