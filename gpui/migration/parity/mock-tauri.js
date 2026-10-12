// Mocked Tauri layer for the original UI in headless Chrome. Injected before
// the bundle loads (Page.addScriptToEvaluateOnNewDocument) with the fixture in
// `window.__PRISM_FIXTURE__`. The answers follow the same rules as the port's
// `crates/prism-ui/src/lib/fixture.rs`, so both sides see the same data.
(() => {
  const fixture = window.__PRISM_FIXTURE__;
  const callbacks = new Map();
  const listeners = new Map(); // event -> [{ eventId, handlerId }]
  let nextId = 1;
  const state = { visible: false, zoom: 1, width: null, calls: [] };
  window.__prismMock = state;

  const lower = (s) => s.trim().toLowerCase();
  const status = () =>
    fixture.fileStatus ?? {
      ready: true,
      indexing: false,
      volumes: [{ drive: "C:\\", state: "ready", indexedCount: 1 }],
      totalIndexed: 1,
    };

  function searchFiles(query) {
    const canned = fixture.files?.[lower(query)] ?? { items: [], pathBrowse: false };
    return { ...status(), items: [], pathBrowse: false, ...canned };
  }

  function phase1(query) {
    const canned = fixture.phase1?.[lower(query)] ?? { recents: [], windows: [], apps: [], actions: [] };
    return { ...canned, query };
  }

  function emit(event, payload) {
    for (const { handlerId, eventId } of listeners.get(event) ?? []) {
      const callback = callbacks.get(handlerId);
      if (callback) callback({ event, id: eventId, payload });
    }
  }
  window.__prismEmit = emit;

  const commands = {
    get_apps: () => fixture.apps.map(({ icon: _icon, ...rest }) => rest),
    refresh_apps: () => fixture.apps.map(({ icon: _icon, ...rest }) => rest),
    get_app_icons: ({ ids }) => {
      const out = {};
      for (const app of fixture.apps) if (ids.includes(app.appId) && app.icon) out[app.appId] = app.icon;
      return out;
    },
    get_quick_access: () => fixture.quickAccess ?? [],
    existing_paths: ({ paths }) => paths.filter((p) => (fixture.existingPaths ?? []).includes(p)),
    get_file_thumbnails: ({ paths }) => paths.map((p) => fixture.thumbnails?.[p] ?? null),
    search_files: ({ query }) => searchFiles(query),
    query_phase1: ({ query }) => phase1(query),
    query_power_plans: ({ query }) => fixture.powerPlans?.[lower(query)] ?? [],
    get_taskbar_settings: () => fixture.taskbar,
    get_system_theme: () => fixture.systemTheme || "dark",
    load_state: () => fixture.state ?? null,
    is_pinned_to_taskbar: ({ path }) => (fixture.pinnedToTaskbar ?? []).includes(path),
    present_palette: () => {
      state.visible = true;
      return true;
    },
    hide_palette: () => {
      state.visible = false;
      return null;
    },
    set_window_width: ({ width }) => {
      state.width = width;
      return null;
    },
    "plugin:app|version": () => fixture.version,
    "plugin:window|is_visible": () => state.visible,
    "plugin:webview|set_webview_zoom": ({ value }) => {
      state.zoom = value;
      return null;
    },
    "plugin:updater|check": () => null,
    "plugin:event|listen": ({ event, handler }) => {
      const eventId = nextId++;
      const list = listeners.get(event) ?? [];
      list.push({ eventId, handlerId: handler });
      listeners.set(event, list);
      return eventId;
    },
    "plugin:event|unlisten": ({ event, eventId }) => {
      listeners.set(event, (listeners.get(event) ?? []).filter((l) => l.eventId !== eventId));
      return null;
    },
  };

  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(event, eventId) {
      listeners.set(event, (listeners.get(event) ?? []).filter((l) => l.eventId !== eventId));
    },
  };
  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
    transformCallback(callback, once) {
      const id = nextId++;
      callbacks.set(id, (response) => {
        if (once) callbacks.delete(id);
        return callback?.(response);
      });
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    convertFileSrc: (path) => path,
    async invoke(cmd, args = {}) {
      state.calls.push({ cmd, args });
      const handler = commands[cmd];
      if (handler) return handler(args);
      // Every other command or plugin call succeeds with no value.
      return null;
    },
  };
})();
