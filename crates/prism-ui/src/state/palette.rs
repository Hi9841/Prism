//! Palette state (`src/state/palette.tsx`): the query, the backend sources,
//! the derived sections, the selection, and running results.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
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
use crate::lib::bridge::{self, BackendEvent, FileStamp, Thumbnail};
use crate::lib::launch_gate::LaunchGate;
use crate::lib::query::{Phase1Response, PowerPlanHit};
use crate::lib::search::dedupe_apps;
use crate::lib::types::{
    AppEntry, FileEntry, FileSearchError, FileSearchResponse, PaletteIcon, PaletteItem, QuickAccessEntry, RowAction,
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
/// How long the list of the last query stays while the Phase 1 reply of a new
/// query is on its way. The reply takes a few ms; this bounds a slow backend.
const PHASE1_WAIT_MS: u64 = 150;
/// The rows that get previews before the view reports the rows it shows.
const ROWS_BEFORE_PAINT: Range<usize> = 0..12;

/// Insertion-ordered thumbnail cache; a cached `None` marks a known-iconless file.
#[derive(Default)]
struct ThumbnailCache {
    map: HashMap<String, Option<String>>,
    /// The file version that each entry was read from (`None`: unreadable).
    stamps: HashMap<String, Option<FileStamp>>,
    order: VecDeque<String>,
    /// Entries that the file may have changed since: still shown, and
    /// checked against the file's stamp when their row shows. Clearing them
    /// instead made every visible image fall back to its tile icon on each
    /// index update, a flash.
    stale: HashSet<String>,
}

impl ThumbnailCache {
    /// Returns true when the cached value changed.
    fn remember(&mut self, path: &str, stamp: Option<FileStamp>, thumbnail: Option<String>) -> bool {
        self.stale.remove(path);
        // Refresh insertion order so recently shown images survive eviction.
        let previous = self.map.remove(path);
        if previous.is_some() {
            self.order.retain(|p| p != path);
        }
        let changed = previous.as_ref() != Some(&thumbnail);
        self.map.insert(path.to_owned(), thumbnail);
        self.stamps.insert(path.to_owned(), stamp);
        self.order.push_back(path.to_owned());
        while self.map.len() > THUMBNAIL_CACHE_LIMIT {
            let Some(oldest) = self.order.pop_front() else { break };
            self.map.remove(&oldest);
            self.stamps.remove(&oldest);
            self.stale.remove(&oldest);
        }
        changed
    }

    /// The file still has `stamp`: the entry is current again. A stamp that
    /// is not the entry's any more (the entry was read again since the
    /// check was asked for) proves nothing.
    fn confirm(&mut self, path: &str, stamp: Option<FileStamp>) {
        if self.stamps.get(path) == Some(&stamp) {
            self.stale.remove(path);
        }
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

    /// The stamp to send with a check of a cached entry.
    fn stamp(&self, path: &str) -> Option<FileStamp> {
        self.stamps.get(path).copied().flatten()
    }

    fn mark_stale(&mut self) {
        self.stale.extend(self.map.keys().cloned());
    }
}

/// What a file-index update asks the palette to read again. A later level
/// includes the earlier ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum IndexUpdate {
    #[default]
    None,
    /// Only the status (ready, indexing, coverage) may have changed.
    Status,
    /// Entries that a search can return changed.
    Content,
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
    /// The file-index updates that arrived while the palette was hidden.
    index_update_pending: IndexUpdate,
    /// The text in the search field.
    pub query: String,
    /// The query that `sections` show. It trails `query` while the Phase 1
    /// reply of a new query is on its way (see `set_query`).
    list_query: String,
    /// Set while the list waits for that reply: shows the list without it
    /// after `PHASE1_WAIT_MS`.
    list_wait: Option<Task<()>>,
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
    /// When the last file search was asked for (see `run_file_search`).
    last_file_search: Option<std::time::Instant>,
    existing_history_paths: HashSet<String>,
    pub selected: usize,
    selection_target: Option<String>,
    thumbnails: ThumbnailCache,
    thumbnails_in_flight: HashSet<String>,
    /// Counts content updates of the index: a preview reply from before one
    /// may hold an old image.
    thumbnail_epoch: u64,
    /// The rows in view, and the rows within half a view of it (see
    /// [`PaletteState::show_rows`]).
    rows_shown: Range<usize>,
    rows_ahead: Range<usize>,
    file_status_known: bool,
    index_status: (bool, bool),
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
    status_task: Option<Task<()>>,
    history_paths_task: Option<Task<()>>,
    /// One launch at a time: repeat presses during a slow launch are ignored.
    launch_gate: LaunchGate,
    _subscriptions: Vec<gpui::Subscription>,
}

impl EventEmitter<PaletteEvent> for PaletteState {}

impl PaletteState {
    fn apply_index_update(&mut self, update: IndexUpdate, cx: &mut Context<Self>) {
        match update {
            IndexUpdate::None => {}
            IndexUpdate::Status => self.refresh_index_status(cx),
            IndexUpdate::Content => self.refresh_after_index_update(cx),
        }
    }

    fn refresh_after_index_update(&mut self, cx: &mut Context<Self>) {
        self.index_update_pending = IndexUpdate::None;
        // Replies started before the update may hold old images: drop them.
        self.thumbnail_epoch += 1;
        self.thumbnails.mark_stale();
        self.thumbnails_in_flight.clear();
        // A refresh may have flipped ready/indexing; re-query the status.
        self.file_status_known = false;
        self.status_task = None;
        self.validate_history_paths(cx);
        self.search_files(cx);
        self.rebuild(cx);
    }

    /// An update that changed no entry (build churn in excluded folders, a
    /// start from the snapshot): reads the status only, and the results and
    /// previews stay. A change of ready or indexing can change what a search
    /// answers, so it refreshes the results too.
    fn refresh_index_status(&mut self, cx: &mut Context<Self>) {
        self.index_update_pending = IndexUpdate::None;
        self.status_task = Some(cx.spawn(async move |this, cx| {
            match bridge::file_index_status().await {
                Ok(response) => {
                    let _ = this.update(cx, |this, cx| {
                        let state_changed =
                            !this.file_status_known || (response.ready, response.indexing) != this.index_status;
                        let coverage_changed = !same_volume_coverage(&this.volumes, &response.volumes)
                            || this.total_indexed != response.total_indexed;
                        this.apply_file_status(&response);
                        if state_changed {
                            this.refresh_after_index_update(cx);
                        } else if coverage_changed {
                            this.rebuild(cx);
                        }
                    });
                }
                Err(error) => eprintln!("Prism could not read the file index status: {error}"),
            }
        }));
    }

    /// The files may have changed while nobody looked (the palette was
    /// hidden, or another app had the focus): each cached preview is checked
    /// against its file when its row shows. The journal reports no edit in
    /// place, so an index update alone would miss one.
    fn revalidate_thumbnails(&mut self, cx: &mut Context<Self>) {
        self.thumbnails.mark_stale();
        self.request_visible_thumbnails(cx);
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
            BackendEvent::FileIndexUpdated { content } => {
                // The index reports changes at most every 2 s. While the
                // palette is hidden nobody sees the refresh: defer it to the
                // next open instead of re-validating and re-querying each time.
                // "Open" is the toggle state: the window shows one task after
                // the open, and an update in between must not be deferred.
                let update = if *content { IndexUpdate::Content } else { IndexUpdate::Status };
                let shell = bridge::shell(cx);
                if shell.palette_held_open() || shell.is_window_visible(cx) {
                    this.apply_index_update(update, cx);
                } else {
                    this.index_update_pending = this.index_update_pending.max(update);
                }
            }
            BackendEvent::Toggle { open: true } => {
                this.apply_index_update(this.index_update_pending, cx);
                this.revalidate_thumbnails(cx);
            }
            BackendEvent::WindowFocused(true) => {
                this.validate_history_paths(cx);
                this.revalidate_thumbnails(cx);
            }
            _ => {}
        });
        let mut this = Self {
            app,
            query: String::new(),
            list_query: String::new(),
            list_wait: None,
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
            index_update_pending: IndexUpdate::None,
            file_indexing: true,
            file_path_browse: false,
            phase1: None,
            power_plans: Vec::new(),
            last_file_search: None,
            existing_history_paths: HashSet::new(),
            selected: 0,
            selection_target: None,
            thumbnails: ThumbnailCache::default(),
            thumbnails_in_flight: HashSet::new(),
            thumbnail_epoch: 0,
            rows_shown: ROWS_BEFORE_PAINT,
            rows_ahead: ROWS_BEFORE_PAINT,
            file_status_known: false,
            index_status: (false, true),
            visible_apps: Vec::new(),
            quick_items: Vec::new(),
            sections: Rc::from(Vec::new()),
            flat_items: Vec::new(),
            phase1_task: None,
            power_task: None,
            file_task: None,
            status_task: None,
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

    /// The query of the shown list. It can trail the search field for a few ms.
    pub fn list_query(&self) -> &str {
        &self.list_query
    }

    pub fn files_busy(&self) -> bool {
        self.files_searching || (!self.file_path_browse && !self.file_index_ready && self.file_indexing)
    }

    pub fn path_browsing(&self) -> bool {
        self.file_path_browse
    }

    /// Rebuilds the sections (the `buildSections` memo) and runs the effects
    /// that depend on the flat list.
    ///
    /// While the list waits for the Phase 1 reply of a new query, the list of
    /// the last query stays as it is: `show_query` rebuilds when the reply
    /// comes. A list built before it lacks the Phase 1 rows (settings, tools,
    /// windows, ranked apps), and they would vanish and come back on each key.
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        if self.list_wait.is_some() {
            return;
        }
        self.list_query.clone_from(&self.query);
        // While the next file search runs, the rows of the last one stay, for
        // the same reason.
        let file_result_query = if self.files_searching {
            js::lower(js::trim(&self.query))
        } else {
            self.file_result_query.clone()
        };
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
            file_results: &self.file_results,
            file_result_query: &file_result_query,
            file_path_browse: self.file_path_browse,
            files_busy: self.files_busy(),
            files_error: self.files_error,
            file_indexing: Some(self.file_indexing || self.volumes.iter().any(|v| v.state == "indexing")),
            file_index_ready: Some(self.file_index_ready || self.volumes.iter().any(|v| v.state == "ready")),
            phase1: self.phase1.as_ref(),
            power_plans: &self.power_plans,
        };
        let BuiltSections { sections, flat_items } = build_sections(&sources);
        self.sections = sections.into();
        self.flat_items = flat_items;
        self.request_visible_thumbnails(cx);

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
        cx.notify();
    }

    /// Shows the list of the typed query, with what has arrived for it.
    fn show_query(&mut self, cx: &mut Context<Self>) {
        self.list_wait = None;
        self.rebuild(cx);
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
                        this.rebuild(cx);
                    });
                }
                Err(error) => eprintln!("Prism could not check recent file paths: {error}"),
            }
        }));
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
                this.show_query(cx);
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
                // Most queries match no plan: the list is as `set_query` left
                // it, and a rebuild would paint the same frame again.
                let plans = result.unwrap_or_default();
                if this.power_plans != plans {
                    this.power_plans = plans;
                    this.rebuild(cx);
                }
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
        // The wait spares a search for each key of fast typing. A key that
        // comes after a pause has no key behind it to wait for: it searches at
        // once (owner decision, 2026-11-10).
        let now = std::time::Instant::now();
        let delay = Duration::from_millis(FILE_SEARCH_DELAY_MS);
        let after_pause = self.last_file_search.is_none_or(|last| now.duration_since(last) >= delay);
        self.last_file_search = Some(now);
        self.file_task = Some(cx.spawn(async move |this, cx| {
            if !after_pause {
                Timer::after(delay).await;
            }
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

    /// The view reports the rows it shows (`shown`) and the rows within half
    /// a view of it (`ahead`), as flat-list indices. Only these rows get
    /// previews: a page holds 50 file rows, and most are never scrolled to.
    pub fn show_rows(&mut self, shown: Range<usize>, ahead: Range<usize>, cx: &mut Context<Self>) {
        if self.rows_shown == shown && self.rows_ahead == ahead {
            return;
        }
        self.rows_shown = shown;
        self.rows_ahead = ahead;
        self.request_visible_thumbnails(cx);
    }

    /// Asks for the previews of the picture rows in view, then of the rows
    /// ahead. A cached preview goes with its stamp: the backend decodes it
    /// again only when the file changed.
    fn request_visible_thumbnails(&mut self, cx: &mut Context<Self>) {
        let (shown, ahead) = (self.rows_shown.clone(), self.rows_ahead.clone());
        let end = self.flat_items.len();
        let order = shown.clone().chain(ahead.start..shown.start.min(ahead.end)).chain(shown.end.max(ahead.start)..ahead.end);
        let mut first: Vec<(String, Option<FileStamp>)> = Vec::new();
        let mut later: Vec<(String, Option<FileStamp>)> = Vec::new();
        for index in order.filter(|&index| index < end) {
            let item = &self.flat_items[index];
            let Some(path) = item.shell_path.as_deref().filter(|_| item.is_picture == Some(true)) else { continue };
            // A preview that came with the search reply needs no request.
            let from_reply = matches!(item.icon, PaletteIcon::Image { .. }) && !self.thumbnails.has(path);
            if from_reply
                || self.thumbnails.is_fresh(path)
                || self.thumbnails_in_flight.contains(path)
                || first.iter().chain(&later).any(|(p, _)| p == path)
            {
                continue;
            }
            let request = (path.to_owned(), self.thumbnails.stamp(path));
            if shown.contains(&index) { first.push(request) } else { later.push(request) }
        }
        if first.is_empty() && later.is_empty() {
            return;
        }
        self.thumbnails_in_flight.extend(first.iter().chain(&later).map(|(path, _)| path.clone()));
        let epoch = self.thumbnail_epoch;
        cx.spawn(async move |this, cx| {
            // The rows in view first. The backend answers at most
            // THUMBNAIL_BATCH_CAP paths per call.
            for batch in first.chunks(THUMBNAIL_BATCH_CAP).chain(later.chunks(THUMBNAIL_BATCH_CAP)) {
                let result = bridge::get_file_thumbnails(batch.to_vec()).await;
                let current = this.update(cx, |this, cx| this.apply_thumbnails(epoch, batch, result, cx));
                if !matches!(current, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Stores one preview reply. Returns false when the reply is from before
    /// a content update of the index (the caller stops).
    fn apply_thumbnails(
        &mut self,
        epoch: u64,
        requests: &[(String, Option<FileStamp>)],
        result: Result<Vec<Thumbnail>, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if epoch != self.thumbnail_epoch {
            return false;
        }
        for (path, _) in requests {
            self.thumbnails_in_flight.remove(path);
        }
        let Ok(replies) = result else { return true };
        let mut changed = false;
        let mut by_path = HashMap::new();
        for ((path, sent), reply) in requests.iter().zip(replies) {
            match reply {
                Thumbnail::Unchanged => self.thumbnails.confirm(path, *sent),
                Thumbnail::Read { stamp, image } => {
                    changed |= self.thumbnails.remember(path, stamp, image.clone());
                    by_path.insert(path.as_str(), image.filter(|t| !t.is_empty()));
                }
            }
        }
        // An unchanged image keeps its data URL, so its row does not change.
        for entry in &mut self.file_results {
            if let Some(thumbnail) = by_path.get(entry.path.as_str())
                && entry.thumbnail != *thumbnail
            {
                entry.thumbnail = thumbnail.clone();
                changed = true;
            }
        }
        if changed {
            self.rebuild(cx);
        }
        true
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
            if js::trim(next).is_empty() {
                // The idle list has no Phase 1 part: show it now.
                self.list_wait = None;
            } else if self.list_wait.is_none() {
                // The Phase 1 reply shows the new list. The wait starts at the
                // first key of a burst, so fast typing still updates the list.
                self.list_wait = Some(cx.spawn(async move |this, cx| {
                    Timer::after(Duration::from_millis(PHASE1_WAIT_MS)).await;
                    let _ = this.update(cx, |this, cx| this.show_query(cx));
                }));
            }
            // The selection moved to the first row.
            cx.notify();
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
        // Enter runs a result of the typed query, not of the list it replaces.
        if self.list_wait.is_some() {
            self.show_query(cx);
        }
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
                run.await
            }
            .await;
            // A fresh palette open resets the gate. Hide only the palette that this launch
            // still owns. The item is open: a failed hide is not a failed open.
            if result.is_ok() && !clipboard_item && ticket.is_current() {
                let hidden = cx.update(|cx| bridge::shell(cx).hide_palette(false, cx));
                if let Ok(Err(error)) = hidden {
                    eprintln!("Prism could not hide the palette after a launch: {}", error_string(&error));
                }
            }
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
                Err(error) => toast(
                    "Couldn\u{2019}t open item",
                    format!("{}: {}", item.title, error_string(&error)),
                    ToastKind::Error,
                    cx,
                ),
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
        self.request_visible_thumbnails(cx);
    }

    pub fn refresh_apps(&mut self, cx: &mut Context<Self>) {
        self.apps_loaded = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = bridge::refresh_apps().await;
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
