//! Palette state (`src/state/palette.tsx`): the query, the backend sources,
//! the derived sections, the selection, and running results.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, EventEmitter, Task, Timer};
use prism_core::catalog::THUMBNAIL_BATCH_CAP;
use prism_core::catalog::search::MAX_LIMIT as MAX_FILE_RESULTS;

use crate::features::palette::sections::{
    BuiltSections, PaletteSources, Section, build_sections, history_file_path, is_clipboard_kind,
    quick_access_palette_item,
};
use crate::js;
use crate::lib::bridge::{self, BackendEvent};
use crate::lib::icon_loading::{AppIconRequestState, app_icon_retry_delay, select_app_icon_request_ids};
use crate::lib::launch_gate::LaunchGate;
use crate::lib::query::{Phase1Response, PowerPlanHit};
use crate::lib::search::dedupe_apps;
use crate::lib::types::{
    AppEntry, FileEntry, FileSearchError, FileSearchResponse, PaletteItem, QuickAccessEntry, RowAction,
    VolumeCoverage,
};
use crate::state::app::{AppEvent, AppState, ToastKind, app_state, error_string};

/// Bounds the retained thumbnail data URLs; the palette lives for weeks. It
/// holds a full result list and the recent rows: a smaller cache evicted the
/// images of a long list and fetched them again on each page.
const THUMBNAIL_CACHE_LIMIT: usize = MAX_FILE_RESULTS + 128;
/// File results per page: the first request asks for one page, and each
/// scroll to the end of the list asks for one more.
const FILE_PAGE: usize = 50;
/// Rows from the end of the list at which keyboard selection loads the next page.
const LOAD_MORE_ROWS: usize = 5;
const POWER_PLAN_QUERY_DELAY_MS: u64 = 50;
const FILE_SEARCH_DELAY_MS: u64 = 35;
const THUMBNAIL_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "bmp", "webp"];

/// Insertion-ordered thumbnail cache; a cached `None` marks a known-iconless file.
#[derive(Default)]
struct ThumbnailCache {
    map: HashMap<String, Option<String>>,
    order: VecDeque<String>,
    /// Entries from before the last index update: still shown, fetched again.
    /// Clearing them instead made every visible image fall back to its tile
    /// icon on each watcher tick (750 ms), a flash.
    stale: HashSet<String>,
}

impl ThumbnailCache {
    /// Returns true when the cached value changed.
    fn remember(&mut self, path: &str, thumbnail: Option<String>) -> bool {
        self.stale.remove(path);
        // Refresh insertion order so recently shown images survive eviction.
        let previous = self.map.remove(path);
        if previous.is_some() {
            self.order.retain(|p| p != path);
        }
        let changed = previous.as_ref() != Some(&thumbnail);
        self.map.insert(path.to_owned(), thumbnail);
        self.order.push_back(path.to_owned());
        while self.map.len() > THUMBNAIL_CACHE_LIMIT {
            let Some(oldest) = self.order.pop_front() else { break };
            self.map.remove(&oldest);
            self.stale.remove(&oldest);
        }
        changed
    }

    fn get(&self, path: &str) -> Option<&Option<String>> {
        self.map.get(path)
    }

    fn has(&self, path: &str) -> bool {
        self.map.contains_key(path)
    }

    fn is_fresh(&self, path: &str) -> bool {
        self.has(path) && !self.stale.contains(path)
    }

    fn mark_stale(&mut self) {
        self.stale.extend(self.map.keys().cloned());
    }
}

fn picture_extension(path: &str, split_dots_and_slashes: bool) -> Option<String> {
    let last = if split_dots_and_slashes {
        path.rsplit(['\\', '/', '.']).next()
    } else {
        path.rsplit('.').next()
    }?;
    let extension = js::lower(last);
    THUMBNAIL_EXTENSIONS.contains(&extension.as_str()).then_some(extension)
}

/// Volume status fields that matter to the rendered UI.
fn same_volume_coverage(current: &[VolumeCoverage], next: &[VolumeCoverage]) -> bool {
    current.len() == next.len()
        && current
            .iter()
            .zip(next)
            .all(|(a, b)| a.drive == b.drive && a.state == b.state && a.indexed_count == b.indexed_count)
}

pub enum PaletteEvent {
    /// The selected index changed (the view keeps it in view).
    SelectionChanged,
}

pub struct PaletteState {
    app: Entity<AppState>,
    /// A file-index update arrived while the palette was hidden.
    index_update_pending: bool,
    pub query: String,
    apps: Vec<AppEntry>,
    apps_revision: u64,
    pub apps_loaded: bool,
    pub apps_error: bool,
    quick_access: Vec<QuickAccessEntry>,
    file_results: Vec<FileEntry>,
    /// The file results the current query asks for (grows by `FILE_PAGE`).
    file_limit: usize,
    /// The last reply filled its page: more matches may exist.
    file_more: bool,
    pub volumes: Vec<VolumeCoverage>,
    pub total_indexed: f64,
    file_result_query: String,
    files_searching: bool,
    pub files_error: bool,
    pub file_error: Option<FileSearchError>,
    file_index_ready: bool,
    pub file_indexing: bool,
    file_path_browse: bool,
    phase1: Option<Phase1Response>,
    power_plans: Vec<PowerPlanHit>,
    existing_history_paths: HashSet<String>,
    app_icons: HashMap<String, String>,
    pub selected: usize,
    selection_target: Option<String>,
    thumbnails: ThumbnailCache,
    thumbnails_in_flight: HashSet<String>,
    thumbnail_epoch: u64,
    file_status_known: bool,
    index_status: (bool, bool),
    icon_settled: HashSet<String>,
    icon_in_flight: HashSet<String>,
    icon_attempts: HashMap<String, u32>,
    icon_retry: Option<Task<()>>,
    // Derived (the useMemo values).
    visible_apps: Vec<AppEntry>,
    quick_items: Vec<PaletteItem>,
    /// Shared with the view, which reads it on every frame without a copy.
    pub sections: Rc<[Section]>,
    pub flat_items: Vec<PaletteItem>,
    // In-flight requests: replacing a task drops (cancels) the older one, as the
    // request counters in the original ignore stale replies.
    phase1_task: Option<Task<()>>,
    power_task: Option<Task<()>>,
    file_task: Option<Task<()>>,
    history_paths_task: Option<Task<()>>,
    /// One launch at a time: repeat presses during a slow launch are ignored.
    launch_gate: LaunchGate,
    _subscriptions: Vec<gpui::Subscription>,
}

impl EventEmitter<PaletteEvent> for PaletteState {}

impl PaletteState {
    fn refresh_after_index_update(&mut self, cx: &mut Context<Self>) {
        self.index_update_pending = false;
        // Replies started before the update may hold old images: drop them.
        self.thumbnail_epoch += 1;
        self.thumbnails.mark_stale();
        self.thumbnails_in_flight.clear();
        // A refresh may have flipped ready/indexing; re-query the status.
        self.file_status_known = false;
        self.validate_history_paths(cx);
        self.load_history_thumbnails(cx);
        self.search_files(cx);
        self.rebuild(cx);
    }
}

impl PaletteState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let app = app_state(cx);
        let app_sub = cx.subscribe(&app, |this, _, event: &AppEvent, cx| match event {
            AppEvent::SettingsChanged { .. } => {
                this.rederive_inputs(cx);
                this.rebuild(cx);
            }
            AppEvent::HistoryChanged => {
                this.validate_history_paths(cx);
                this.load_history_thumbnails(cx);
                this.rebuild(cx);
            }
            AppEvent::OpenSettingsChanged => {}
        });
        let events = bridge::event_hub(cx);
        let event_sub = cx.subscribe(&events, |this, _, event: &BackendEvent, cx| match event {
            // The native index refreshes when a shortcut or install folder shows up
            // after the process started. Pull the new list and rerun the open query.
            BackendEvent::AppsUpdated => {
                cx.spawn(async move |this, cx| {
                    let result = bridge::get_apps().await;
                    let _ = this.update(cx, |this, cx| {
                        match result {
                            Ok(list) => {
                                this.set_apps(list, cx);
                                this.apps_error = false;
                                this.apps_loaded = true;
                                this.apps_revision += 1;
                                this.query_phase1(cx);
                            }
                            Err(_) => this.apps_error = true,
                        }
                        this.rebuild(cx);
                    });
                })
                .detach();
            }
            BackendEvent::FileIndexUpdated => {
                // The watcher reports disk activity every 750 ms. While the
                // palette is hidden nobody sees the refresh: defer it to the
                // next open instead of re-validating and re-querying each time.
                // "Open" is the toggle state: the window shows one task after
                // the open, and an update in between must not be deferred.
                let shell = bridge::shell(cx);
                if shell.palette_held_open() || shell.is_window_visible(cx) {
                    this.refresh_after_index_update(cx);
                } else {
                    this.index_update_pending = true;
                }
            }
            BackendEvent::Toggle { open: true } if this.index_update_pending => {
                this.refresh_after_index_update(cx);
            }
            BackendEvent::WindowFocused(true) => this.validate_history_paths(cx),
            _ => {}
        });
        let mut this = Self {
            app,
            query: String::new(),
            apps: Vec::new(),
            apps_revision: 0,
            apps_loaded: false,
            apps_error: false,
            quick_access: Vec::new(),
            file_results: Vec::new(),
            file_limit: FILE_PAGE,
            file_more: false,
            volumes: Vec::new(),
            total_indexed: 0.0,
            file_result_query: String::new(),
            files_searching: false,
            files_error: false,
            file_error: None,
            file_index_ready: false,
            index_update_pending: false,
            file_indexing: true,
            file_path_browse: false,
            phase1: None,
            power_plans: Vec::new(),
            existing_history_paths: HashSet::new(),
            app_icons: HashMap::new(),
            selected: 0,
            selection_target: None,
            thumbnails: ThumbnailCache::default(),
            thumbnails_in_flight: HashSet::new(),
            thumbnail_epoch: 0,
            file_status_known: false,
            index_status: (false, true),
            icon_settled: HashSet::new(),
            icon_in_flight: HashSet::new(),
            icon_attempts: HashMap::new(),
            icon_retry: None,
            visible_apps: Vec::new(),
            quick_items: Vec::new(),
            sections: Rc::from(Vec::new()),
            flat_items: Vec::new(),
            phase1_task: None,
            power_task: None,
            file_task: None,
            history_paths_task: None,
            launch_gate: LaunchGate::default(),
            _subscriptions: vec![app_sub, event_sub],
        };
        this.load_initial(cx);
        this.search_files(cx);
        this.validate_history_paths(cx);
        this.rebuild(cx);
        this
    }

    fn load_initial(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = bridge::get_apps().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.set_apps(list, cx);
                        this.apps_error = false;
                    }
                    Err(_) => this.apps_error = true,
                }
                this.apps_loaded = true;
                this.rebuild(cx);
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            match bridge::get_quick_access().await {
                Ok(items) => {
                    let _ = this.update(cx, |this, cx| {
                        this.quick_access = items;
                        this.rederive_inputs(cx);
                        this.rebuild(cx);
                    });
                }
                Err(error) => eprintln!("Prism could not load Quick Access folders: {error}"),
            }
        })
        .detach();
    }

    fn set_apps(&mut self, list: Vec<AppEntry>, cx: &mut Context<Self>) {
        self.apps = list;
        self.visible_apps = dedupe_apps(&self.apps);
        let _ = cx;
    }

    /// Recomputes the memoized inputs that depend on settings.
    fn rederive_inputs(&mut self, cx: &mut Context<Self>) {
        let settings = self.app.read(cx).settings.clone();
        let entries_by_kind: HashMap<&str, &QuickAccessEntry> =
            self.quick_access.iter().map(|entry| (entry.kind.as_str(), entry)).collect();
        self.quick_items = settings
            .quick_access
            .iter()
            .filter_map(|kind| entries_by_kind.get(kind.as_str()))
            .map(|entry| quick_access_palette_item(entry))
            .collect();
    }

    pub fn apps(&self) -> &[AppEntry] {
        &self.visible_apps
    }

    pub fn app_icons(&self) -> &HashMap<String, String> {
        &self.app_icons
    }

    pub fn files_busy(&self) -> bool {
        self.files_searching || (!self.file_path_browse && !self.file_index_ready && self.file_indexing)
    }

    pub fn path_browsing(&self) -> bool {
        self.file_path_browse
    }

    /// Rebuilds the sections (the `buildSections` memo) and runs the effects
    /// that depend on the flat list.
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let settings = &app.settings;
        let apps_by_id: HashMap<&str, &AppEntry> =
            self.visible_apps.iter().map(|entry| (entry.app_id.as_str(), entry)).collect();
        let pinned_apps: Vec<&AppEntry> =
            settings.pinned_apps.iter().filter_map(|id| apps_by_id.get(id.as_str()).copied()).collect();
        let sources = PaletteSources {
            query: &self.query,
            apps: &self.visible_apps,
            pinned_apps,
            quick_items: &self.quick_items,
            quick_access_collapsed: settings.quick_access_collapsed,
            app_groups: &settings.app_groups,
            section_order: Some(&settings.section_order),
            pinned_app_ids: &settings.pinned_apps,
            history: &app.history,
            existing_history_paths: &self.existing_history_paths,
            file_thumbnails: &self.thumbnails.map,
            app_icons: &self.app_icons,
            file_results: &self.file_results,
            file_result_query: &self.file_result_query,
            file_path_browse: self.file_path_browse,
            files_busy: self.files_busy(),
            files_error: self.files_error,
            file_indexing: Some(self.file_indexing || self.volumes.iter().any(|v| v.state == "indexing")),
            file_index_ready: Some(self.file_index_ready || self.volumes.iter().any(|v| v.state == "ready")),
            phase1: self.phase1.as_ref(),
            power_plans: &self.power_plans,
        };
        let BuiltSections { sections, flat_items } = build_sections(&sources);
        let pinned_ids = settings.pinned_apps.clone();
        self.sections = sections.into();
        self.flat_items = flat_items;

        // Clamp the selection to the list, then honor a pending selection target.
        let clamped = self.selected.min(self.flat_items.len().saturating_sub(1));
        if clamped != self.selected {
            self.selected = clamped;
            cx.emit(PaletteEvent::SelectionChanged);
        }
        if let Some(target) = self.selection_target.clone()
            && let Some(index) = self.flat_items.iter().position(|item| item.id == target)
        {
            self.selection_target = None;
            self.selected = index;
            cx.emit(PaletteEvent::SelectionChanged);
        }
        self.request_icons(pinned_ids, cx);
        cx.notify();
    }

    /// Lazily fills app icons for the rows that are actually rendered.
    fn request_icons(&mut self, pinned_ids: Vec<String>, cx: &mut Context<Self>) {
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for id in self.flat_items.iter().filter_map(|item| item.app_id.clone()).chain(pinned_ids) {
            if seen.insert(id.clone()) {
                ids.push(id);
            }
        }
        let missing = select_app_icon_request_ids(
            &ids,
            &AppIconRequestState {
                apps_loaded: self.apps_loaded,
                icons: &self.app_icons,
                settled: &self.icon_settled,
                in_flight: &self.icon_in_flight,
                attempts: &self.icon_attempts,
            },
        );
        if missing.is_empty() {
            return;
        }
        for id in &missing {
            self.icon_in_flight.insert(id.clone());
            *self.icon_attempts.entry(id.clone()).or_insert(0) += 1;
        }
        cx.spawn(async move |this, cx| {
            let result = bridge::get_app_icons(missing.clone()).await;
            let _ = this.update(cx, |this, cx| {
                for id in &missing {
                    this.icon_in_flight.remove(id);
                }
                match result {
                    Ok(icons) => {
                        // The app cache is ready at this point, so omitted ids are
                        // genuinely iconless and should not issue another request.
                        this.icon_settled.extend(missing.iter().cloned());
                        if !icons.is_empty() {
                            this.app_icons.extend(icons);
                            this.rebuild(cx);
                        }
                    }
                    Err(_) => {
                        let attempt =
                            missing.iter().map(|id| this.icon_attempts.get(id).copied().unwrap_or(1)).max().unwrap_or(1);
                        if let Some(delay) = app_icon_retry_delay(attempt as f64)
                            && this.icon_retry.is_none()
                        {
                            this.icon_retry = Some(cx.spawn(async move |this, cx| {
                                Timer::after(Duration::from_millis(delay as u64)).await;
                                let _ = this.update(cx, |this, cx| {
                                    this.icon_retry = None;
                                    let pinned = this.app.read(cx).settings.pinned_apps.clone();
                                    this.request_icons(pinned, cx);
                                });
                            }));
                        }
                    }
                }
            });
        })
        .detach();
    }

    fn validate_history_paths(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .app
            .read(cx)
            .history
            .iter()
            .filter_map(|entry| history_file_path(&entry.id).map(str::to_owned))
            .collect();
        // Keep the previous rows mounted while re-validating so the Recent
        // section doesn't blink out on every window focus.
        if paths.is_empty() {
            self.history_paths_task = None;
            if !self.existing_history_paths.is_empty() {
                self.existing_history_paths.clear();
                self.rebuild(cx);
            }
            return;
        }
        self.history_paths_task = Some(cx.spawn(async move |this, cx| {
            match bridge::existing_paths(paths).await {
                Ok(existing) => {
                    let _ = this.update(cx, |this, cx| {
                        this.existing_history_paths = existing.into_iter().collect();
                        this.load_history_thumbnails(cx);
                        this.rebuild(cx);
                    });
                }
                Err(error) => eprintln!("Prism could not check recent file paths: {error}"),
            }
        }));
    }

    fn load_history_thumbnails(&mut self, cx: &mut Context<Self>) {
        let mut image_paths: Vec<String> = Vec::new();
        for entry in &self.app.read(cx).history {
            if !entry.id.starts_with("file::f::") {
                continue;
            }
            let Some(path) = history_file_path(&entry.id) else { continue };
            if !self.existing_history_paths.contains(path) {
                continue;
            }
            if picture_extension(path, true).is_some() && !image_paths.iter().any(|p| p == path) {
                image_paths.push(path.to_owned());
            }
        }
        let pending: Vec<String> = image_paths
            .into_iter()
            .filter(|path| !self.thumbnails.is_fresh(path) && !self.thumbnails_in_flight.contains(path))
            .collect();
        if pending.is_empty() {
            return;
        }
        self.thumbnails_in_flight.extend(pending.iter().cloned());
        let epoch = self.thumbnail_epoch;
        cx.spawn(async move |this, cx| {
            let result = bridge::get_file_thumbnails(pending.clone()).await;
            let _ = this.update(cx, |this, cx| {
                if epoch != this.thumbnail_epoch {
                    return;
                }
                for path in &pending {
                    this.thumbnails_in_flight.remove(path);
                }
                if let Ok(thumbnails) = result {
                    let mut changed = false;
                    for (path, thumbnail) in pending.iter().zip(thumbnails) {
                        changed |= this.thumbnails.remember(path, thumbnail);
                    }
                    if changed {
                        this.rebuild(cx);
                    }
                }
            });
        })
        .detach();
    }

    fn query_phase1(&mut self, cx: &mut Context<Self>) {
        let search_text = js::trim(&self.query).to_owned();
        if search_text.is_empty() {
            self.phase1_task = None;
            self.phase1 = None;
            return;
        }
        self.phase1_task = Some(cx.spawn(async move |this, cx| {
            let result = bridge::query_phase1(search_text).await;
            let _ = this.update(cx, |this, cx| {
                this.phase1 = result.ok().filter(|response| !response.query.is_empty());
                this.rebuild(cx);
            });
        }));
    }

    fn query_power_plans(&mut self, cx: &mut Context<Self>) {
        let search_text = js::trim(&self.query).to_owned();
        self.power_plans.clear();
        if search_text.is_empty() {
            self.power_task = None;
            return;
        }
        self.power_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(POWER_PLAN_QUERY_DELAY_MS)).await;
            let result = bridge::query_power_plans(search_text).await;
            let _ = this.update(cx, |this, cx| {
                this.power_plans = result.unwrap_or_default();
                this.rebuild(cx);
            });
        }));
    }

    /// Re-queries the power plans for a submitted query (after a plan change).
    fn refresh_power_plans(&mut self, search_text: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.power_task = None;
        if search_text.is_empty() {
            self.power_plans.clear();
            return Task::ready(Ok(()));
        }
        cx.spawn(async move |this, cx| {
            let result = bridge::query_power_plans(search_text).await;
            let _ = this.update(cx, |this, cx| {
                this.power_plans = result.clone().unwrap_or_default();
                this.rebuild(cx);
            });
            result.map(|_| ())
        })
    }

    fn apply_file_status(&mut self, response: &FileSearchResponse) {
        self.file_status_known = true;
        self.index_status = (response.ready, response.indexing);
        self.file_index_ready = response.ready;
        self.file_indexing = response.indexing;
        if !same_volume_coverage(&self.volumes, &response.volumes) {
            self.volumes = response.volumes.clone();
        }
        self.total_indexed = response.total_indexed;
    }

    fn search_files(&mut self, cx: &mut Context<Self>) {
        self.run_file_search(false, cx);
    }

    /// Asks the backend for `file_limit` results. A page load (`append`)
    /// keeps the rows already shown and adds the new ones after them: the
    /// backend ranks a candidate pool that grows with the limit, so a larger
    /// reply can rank new rows above rows the user already scrolled past.
    fn run_file_search(&mut self, append: bool, cx: &mut Context<Self>) {
        let search_text = js::trim(&self.query).to_owned();
        let normalized = js::lower(&search_text);
        if js::len(&normalized) < 2 {
            self.file_results.clear();
            self.file_more = false;
            self.file_result_query = normalized;
            self.files_searching = false;
            self.files_error = false;
            self.file_error = None;
            self.file_path_browse = false;
            // Status is already known and no index refresh happened since - skip
            // the round trip on every backspace/reset.
            let (ready, indexing) = self.index_status;
            if self.file_status_known && ready && !indexing {
                self.file_task = None;
                return;
            }
            self.file_task = Some(cx.spawn(async move |this, cx| {
                match bridge::search_files(String::new(), 1).await {
                    Ok(response) => {
                        let _ = this.update(cx, |this, cx| {
                            this.apply_file_status(&response);
                            this.rebuild(cx);
                        });
                    }
                    Err(error) => eprintln!("Prism could not read the file index status: {error}"),
                }
            }));
            return;
        }

        self.files_searching = true;
        self.files_error = false;
        self.file_error = None;
        let limit = self.file_limit;
        self.file_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(FILE_SEARCH_DELAY_MS)).await;
            let result = bridge::search_files(search_text, limit).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(response) => {
                        this.apply_file_status(&response);
                        let items: Vec<FileEntry> = response
                            .items
                            .iter()
                            .cloned()
                            .map(|mut entry| {
                                if let Some(Some(thumbnail)) = this.thumbnails.get(&entry.path)
                                    && !thumbnail.is_empty()
                                {
                                    entry.thumbnail = Some(thumbnail.clone());
                                }
                                entry
                            })
                            .collect();
                        this.file_more = items.len() >= limit;
                        let items = if append && this.file_result_query == normalized {
                            let shown: HashSet<String> =
                                this.file_results.iter().map(|entry| entry.path.to_lowercase()).collect();
                            let mut merged = std::mem::take(&mut this.file_results);
                            merged.extend(items.into_iter().filter(|entry| !shown.contains(&entry.path.to_lowercase())));
                            merged.truncate(MAX_FILE_RESULTS);
                            merged
                        } else {
                            items
                        };
                        this.file_result_query = normalized;
                        this.file_path_browse = response.path_browse;
                        this.file_error = response.error.clone();
                        this.files_error = response.error.is_some()
                            || (!response.path_browse && !response.ready && !response.indexing);
                        this.request_result_thumbnails(&items, cx);
                        this.file_results = items;
                    }
                    Err(error) => {
                        this.file_status_known = true;
                        this.index_status = (false, false);
                        this.file_results.clear();
                        this.file_more = false;
                        this.file_result_query = normalized;
                        this.file_index_ready = false;
                        this.file_indexing = false;
                        this.file_path_browse = false;
                        this.file_error =
                            Some(FileSearchError { kind: "indexQuery".into(), message: error_string(&error) });
                        this.files_error = true;
                    }
                }
                this.files_searching = false;
                this.rebuild(cx);
            });
        }));
    }

    fn request_result_thumbnails(&mut self, items: &[FileEntry], cx: &mut Context<Self>) {
        let pending: Vec<String> = items
            .iter()
            .filter(|entry| {
                // A thumbnail on an uncached entry came with the search reply.
                !entry.is_directory
                    && (entry.thumbnail.as_deref().is_none_or(str::is_empty) || self.thumbnails.has(&entry.path))
                    && !self.thumbnails.is_fresh(&entry.path)
                    && picture_extension(&entry.name, false).is_some()
                    && !self.thumbnails_in_flight.contains(&entry.path)
            })
            .map(|entry| entry.path.clone())
            .collect();
        if pending.is_empty() {
            return;
        }
        self.thumbnails_in_flight.extend(pending.iter().cloned());
        let epoch = self.thumbnail_epoch;
        cx.spawn(async move |this, cx| {
            // The backend answers at most THUMBNAIL_BATCH_CAP paths per call
            // and pads the rest with None, which would mark them iconless.
            for batch in pending.chunks(THUMBNAIL_BATCH_CAP) {
                let result = bridge::get_file_thumbnails(batch.to_vec()).await;
                let current = this.update(cx, |this, cx| {
                    if epoch != this.thumbnail_epoch {
                        return false;
                    }
                    let Ok(thumbnails) = result else {
                        for path in batch {
                            this.thumbnails_in_flight.remove(path);
                        }
                        return true;
                    };
                    let mut by_path = HashMap::new();
                    for (path, thumbnail) in batch.iter().zip(thumbnails) {
                        this.thumbnails.remember(path, thumbnail.clone());
                        this.thumbnails_in_flight.remove(path);
                        by_path.insert(path, thumbnail.filter(|t| !t.is_empty()));
                    }
                    // An unchanged image keeps its data URL, so its row does not change.
                    let mut changed = false;
                    for entry in &mut this.file_results {
                        if let Some(thumbnail) = by_path.get(&entry.path)
                            && entry.thumbnail != *thumbnail
                        {
                            entry.thumbnail = thumbnail.clone();
                            changed = true;
                        }
                    }
                    if changed {
                        this.rebuild(cx);
                    }
                    true
                });
                if !matches!(current, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn set_query(&mut self, next: &str, cx: &mut Context<Self>) {
        self.selection_target = None;
        self.power_plans.clear();
        let changed = self.query != next;
        self.query = next.to_owned();
        if self.selected != 0 {
            self.selected = 0;
            cx.emit(PaletteEvent::SelectionChanged);
        }
        if changed {
            self.file_limit = FILE_PAGE;
            self.query_phase1(cx);
            self.query_power_plans(cx);
            self.search_files(cx);
        }
        self.rebuild(cx);
    }

    pub fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.selection_target = None;
        let next = if self.flat_items.is_empty() {
            0
        } else {
            (self.selected as i64 + delta as i64).clamp(0, self.flat_items.len() as i64 - 1) as usize
        };
        if next != self.selected {
            self.selected = next;
            cx.emit(PaletteEvent::SelectionChanged);
            cx.notify();
        }
        if next + LOAD_MORE_ROWS >= self.flat_items.len() {
            self.load_more_files(cx);
        }
    }

    /// Asks for the next page of file results. Does nothing while a search
    /// runs, after a reply that did not fill its page (no more matches), or at
    /// the cap. The current rows stay, and the reply adds rows after them.
    pub fn load_more_files(&mut self, cx: &mut Context<Self>) {
        let query = js::lower(js::trim(&self.query));
        if self.files_searching
            || self.file_result_query != query
            || !self.file_more
            || self.file_limit >= MAX_FILE_RESULTS
        {
            return;
        }
        self.file_limit = (self.file_limit + FILE_PAGE).min(MAX_FILE_RESULTS);
        self.run_file_search(true, cx);
    }

    pub fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selection_target = None;
        if index != self.selected {
            self.selected = index;
            cx.emit(PaletteEvent::SelectionChanged);
            cx.notify();
        }
    }

    pub fn run_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.flat_items.get(self.selected).cloned() {
            self.run_item(item, cx);
        }
    }

    pub fn run_item(&mut self, item: PaletteItem, cx: &mut Context<Self>) {
        let Some(ticket) = self.launch_gate.try_begin() else { return };
        let submitted_query = js::trim(&self.query).to_owned();
        let query = self.query.clone();
        let clipboard_item = is_clipboard_kind(&item.id);
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let toast = |title: &str, detail: String, kind: ToastKind, cx: &mut gpui::AsyncApp| {
                let title = title.to_owned();
                let _ = app.update(cx, |app, cx| app.show_toast(title, Some(detail), kind, cx));
            };
            if item.power_plan_guid.is_some() {
                let result = match cx.update(|cx| run_action(item.run.clone(), cx)) {
                    Ok(task) => task.await,
                    Err(error) => Err(error.to_string()),
                };
                // The plan is set: the list refresh below needs no gate.
                drop(ticket);
                if let Err(error) = result {
                    toast("Power plan not changed", error_string(&error), ToastKind::Error, cx);
                    return;
                }
                let current = this.update(cx, |this, _| js::trim(&this.query).to_owned()).unwrap_or_default();
                if current != submitted_query {
                    toast("Power plan changed", item.title.clone(), ToastKind::Success, cx);
                    return;
                }
                let refresh = this.update(cx, |this, cx| this.refresh_power_plans(submitted_query.clone(), cx));
                let refreshed = match refresh {
                    Ok(task) => task.await,
                    Err(error) => Err(error.to_string()),
                };
                match refreshed {
                    Ok(()) => {
                        let _ = this.update(cx, |this, cx| {
                            if js::trim(&this.query) == submitted_query {
                                this.selection_target = Some(item.id.clone());
                                this.rebuild(cx);
                            }
                        });
                        toast("Power plan changed", item.title.clone(), ToastKind::Success, cx);
                    }
                    Err(error) => {
                        toast(
                            "Power plan changed",
                            format!("Could not refresh the list: {}", error_string(&error)),
                            ToastKind::Success,
                            cx,
                        );
                    }
                }
                return;
            }
            let result: Result<(), String> = async {
                let run = cx.update(|cx| run_action(item.run.clone(), cx)).map_err(|e| e.to_string())?;
                run.await?;
                // A fresh palette open resets the gate. Hide only the palette
                // that this launch still owns.
                if !clipboard_item && ticket.is_current() {
                    cx.update(|cx| bridge::shell(cx).hide_palette(false, cx)).map_err(|e| e.to_string())??;
                }
                Ok(())
            }
            .await;
            // The launch and hide are done: history and intent writes need no gate.
            drop(ticket);
            match result {
                Ok(()) => {
                    let _ = app.update(cx, |app, cx| {
                        app.push_history(&item.id, &item.history_title, cx);
                        if clipboard_item {
                            let detail = item.toast_detail.clone().unwrap_or_else(|| item.title.clone());
                            app.show_toast("Copied to clipboard", Some(detail), ToastKind::Success, cx);
                        }
                    });
                    if let Some(action_id) = item.id.strip_prefix("action::") {
                        let _ = bridge::accept_intent(query, action_id.to_owned()).await;
                    }
                }
                Err(_) => toast("Couldn\u{2019}t open item", item.title.clone(), ToastKind::Error, cx),
            }
        })
        .detach();
    }

    pub fn run_item_as_admin(&mut self, item: PaletteItem, cx: &mut Context<Self>) {
        let Some(action) = item.run_as_admin.clone() else { return };
        let Some(ticket) = self.launch_gate.try_begin() else { return };
        let app = self.app.clone();
        cx.spawn(async move |_, cx| {
            let _ticket = ticket;
            // Same ordering as run_item: elevate while foreground, then hide.
            let result: Result<(), String> = async {
                let run = cx.update(|cx| run_action(action, cx)).map_err(|e| e.to_string())?;
                run.await?;
                if _ticket.is_current() {
                    cx.update(|cx| bridge::shell(cx).hide_palette(false, cx)).map_err(|e| e.to_string())??;
                }
                Ok(())
            }
            .await;
            let _ = app.update(cx, |app, cx| match result {
                Ok(()) => app.push_history(&item.id, &item.history_title, cx),
                Err(error) => app.show_toast(
                    "Could not run as administrator",
                    Some(error_string(&error)),
                    ToastKind::Error,
                    cx,
                ),
            });
        })
        .detach();
    }

    pub fn rebuild_index(&mut self, cx: &mut Context<Self>) {
        self.file_indexing = true;
        cx.spawn(async move |this, cx| {
            if let Err(error) = bridge::rebuild_file_index().await {
                eprintln!("Prism could not rebuild the file index: {error}");
                let _ = this.update(cx, |this, cx| {
                    this.file_indexing = false;
                    this.rebuild(cx);
                });
            }
        })
        .detach();
        self.rebuild(cx);
    }

    pub fn retry_file_search(&mut self, cx: &mut Context<Self>) {
        self.search_files(cx);
        self.load_history_thumbnails(cx);
    }

    pub fn refresh_apps(&mut self, cx: &mut Context<Self>) {
        self.apps_loaded = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = bridge::refresh_apps().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        this.icon_retry = None;
                        this.icon_settled.clear();
                        this.icon_attempts.clear();
                        // Icons captured before the rescan can outlive their apps on disk;
                        // dropping them lets the icon effect re-request the visible set.
                        this.app_icons.clear();
                        this.set_apps(list, cx);
                        this.apps_error = false;
                    }
                    Err(_) => this.apps_error = true,
                }
                this.apps_loaded = true;
                this.rebuild(cx);
            });
        })
        .detach();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        // A fresh open may launch again, even if an older launch never returned.
        self.launch_gate.reset();
        self.set_query("", cx);
        self.selected = 0;
        cx.emit(PaletteEvent::SelectionChanged);
    }
}

/// Runs a row action: the clipboard on the UI thread, backend calls through
/// [`RowAction::execute`] on a background thread (A7).
pub fn run_action(action: RowAction, cx: &mut App) -> Task<Result<(), String>> {
    match action {
        RowAction::None => Task::ready(Ok(())),
        RowAction::Clipboard(text) => {
            bridge::copy_text(&text, cx);
            Task::ready(Ok(()))
        }
        action => cx.background_spawn(async move { action.execute(&mut bridge::CoreInvoker) }),
    }
}

#[derive(Clone)]
pub struct PaletteStateHandle(pub Entity<PaletteState>);

impl gpui::Global for PaletteStateHandle {}

pub fn palette_state(cx: &App) -> Entity<PaletteState> {
    cx.global::<PaletteStateHandle>().0.clone()
}

pub fn install(cx: &mut App) -> Entity<PaletteState> {
    let entity = cx.new(PaletteState::new);
    cx.set_global(PaletteStateHandle(entity.clone()));
    entity
}

// PORT STATUS: confidence=medium todos=0
