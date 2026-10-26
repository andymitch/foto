use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::DateTime;
use foto_core::{AssetDetail, AssetSummary, Event, EventSink, Filter, Foto, Kind, Policy, Status};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::thumbs::{Thumbs, decode};

const SIDEBAR: f32 = 232.;
const VIEWER: &str = "FotoViewer";

actions!(viewer, [Close, Prev, Next, ToggleInfo, Favorite, Delete]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Close, Some(VIEWER)),
        KeyBinding::new("left", Prev, Some(VIEWER)),
        KeyBinding::new("right", Next, Some(VIEWER)),
        KeyBinding::new("i", ToggleInfo, Some(VIEWER)),
        KeyBinding::new("f", Favorite, Some(VIEWER)),
        KeyBinding::new("backspace", Delete, Some(VIEWER)),
        KeyBinding::new("delete", Delete, Some(VIEWER)),
    ]);
}
const GAP: f32 = 4.;
const TARGET_TILE: f32 = 176.;
const PAD: f32 = 16.;

#[derive(Clone, Copy, PartialEq)]
pub enum Section {
    Photos,
    Favorites,
    Deleted,
    Folders,
    Devices,
    Settings,
}

impl Section {
    fn filter(self) -> Option<Filter> {
        match self {
            Section::Photos => Some(Filter::All),
            Section::Favorites => Some(Filter::Favorites),
            Section::Deleted => Some(Filter::Deleted),
            _ => None,
        }
    }
}

enum Row {
    Header(SharedString),
    Tiles(usize, usize),
}

struct Viewer {
    index: usize,
    hash: String,
    large: Option<Arc<RenderImage>>,
    full: Option<Arc<RenderImage>>,
    detail: Option<AssetDetail>,
    zoom: f32,
    offset: Point<Pixels>,
    drag: Option<Point<Pixels>>,
    info: bool,
}

struct Share {
    ticket: SharedString,
    writable: bool,
    qr: Arc<RenderImage>,
}

#[derive(Default)]
pub struct Inbox(Mutex<Vec<Event>>);

impl EventSink for Inbox {
    fn on_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

pub struct FotoApp {
    foto: Arc<Foto>,
    lib: Option<String>,
    section: Section,
    items: Arc<Vec<AssetSummary>>,
    rows: Vec<Row>,
    cols: usize,
    tile: f32,
    list: ListState,
    thumbs: Thumbs,
    viewer: Option<Viewer>,
    status: Status,
    share: Option<Share>,
    focus: FocusHandle,
    name_input: Entity<InputState>,
    ticket_input: Entity<InputState>,
    device_input: Entity<InputState>,
    _poll: Task<()>,
}

pub fn day(ts: i64, fmt: &str) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|d| d.format(fmt).to_string())
        .unwrap_or_default()
}

fn duration(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b / 1024),
    }
}

fn qr(text: &str) -> Option<Arc<RenderImage>> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let (w, scale, quiet) = (code.width() as u32, 6u32, 4u32);
    let side = (w + quiet * 2) * scale;
    let colors = code.to_colors();
    let img = image::RgbaImage::from_fn(side, side, |x, y| {
        let (mx, my) = (x / scale, y / scale);
        let dark = mx >= quiet
            && my >= quiet
            && mx < w + quiet
            && my < w + quiet
            && colors[((my - quiet) * w + (mx - quiet)) as usize] == qrcode::Color::Dark;
        if dark {
            image::Rgba([0, 0, 0, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    });
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(img)])))
}

impl FotoApp {
    pub fn new(foto: Arc<Foto>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inbox = Arc::new(Inbox::default());
        foto.subscribe(inbox.clone());
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let events: Vec<Event> = std::mem::take(&mut *inbox.0.lock().unwrap());
                if events.is_empty() {
                    continue;
                }
                let ok = this.update(cx, |this, cx| this.on_events(events, cx));
                if ok.is_err() {
                    break;
                }
            }
        });
        let lib = foto.libraries().first().map(|l| l.id.clone());
        let name = foto.device_name();
        let mut app = FotoApp {
            status: foto.status(),
            foto,
            lib,
            section: Section::Photos,
            items: Arc::new(Vec::new()),
            rows: Vec::new(),
            cols: 0,
            tile: TARGET_TILE,
            list: ListState::new(0, ListAlignment::Top, px(800.)),
            thumbs: Thumbs::new(400),
            viewer: None,
            share: None,
            focus: cx.focus_handle(),
            name_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Library name, e.g. Family")),
            ticket_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Paste a ticket from another device")
            }),
            device_input: cx.new(|cx| InputState::new(window, cx).default_value(name)),
            _poll: poll,
        };
        app.reload(cx);
        app
    }

    fn on_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        let mut reload = false;
        for e in events {
            match e {
                Event::LibraryChanged { library } => {
                    reload |= Some(&library) == self.lib.as_ref();
                    if self.lib.is_none() {
                        self.lib = Some(library);
                        reload = true;
                    }
                }
                Event::StatusChanged { status } => self.status = status,
            }
        }
        if reload {
            self.thumbs.retry_failed();
            self.reload(cx);
            if let Some(v) = &mut self.viewer
                && (v.large.is_none() || v.full.is_none())
            {
                let i = v.index;
                self.load_viewer(i, cx);
            }
        }
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let (Some(lib), Some(filter)) = (self.lib.clone(), self.section.filter()) else {
            return;
        };
        let foto = self.foto.clone();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_spawn(async move { foto.timeline(lib, filter) })
                .await;
            this.update(cx, |this, cx| {
                this.items = Arc::new(items);
                this.rebuild();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut i = 0;
        let cols = self.cols.max(1);
        while i < self.items.len() {
            let month = day(self.items[i].taken_at, "%B %Y");
            rows.push(Row::Header(month.clone().into()));
            let start = i;
            while i < self.items.len() && day(self.items[i].taken_at, "%B %Y") == month {
                i += 1;
            }
            let mut s = start;
            while s < i {
                let e = (s + cols).min(i);
                rows.push(Row::Tiles(s, e));
                s = e;
            }
        }
        let top = self.list.logical_scroll_top();
        self.rows = rows;
        self.list.reset(self.rows.len());
        self.list.scroll_to(top);
    }

    fn select(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.viewer = None;
        self.items = Arc::new(Vec::new());
        self.rebuild();
        self.reload(cx);
        cx.notify();
    }

    fn load_thumb(&mut self, hash: String, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.clone() else { return };
        let foto = self.foto.clone();
        cx.spawn(async move |this, cx| {
            let key = hash.clone();
            let img = cx
                .background_spawn(async move {
                    foto.thumbnail(lib, hash, false).and_then(|b| decode(&b))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(img) = img {
                    this.thumbs.insert(key, Arc::new(img), cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_viewer(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        self.viewer = Some(Viewer {
            index,
            hash: item.hash.clone(),
            large: None,
            full: None,
            detail: None,
            zoom: 1.,
            offset: Point::default(),
            drag: None,
            info: self.viewer.as_ref().is_some_and(|v| v.info),
        });
        self.load_viewer(index, cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn load_viewer(&mut self, index: usize, cx: &mut Context<Self>) {
        let (Some(lib), Some(item)) = (self.lib.clone(), self.items.get(index).cloned()) else {
            return;
        };
        let foto = self.foto.clone();
        cx.spawn(async move |this, cx| {
            let hash = item.hash.clone();
            let (detail, large, full) = cx
                .background_spawn(async move {
                    let detail = foto.asset(lib.clone(), hash.clone());
                    let large = foto
                        .thumbnail(lib.clone(), hash.clone(), true)
                        .and_then(|b| decode(&b));
                    let full = match item.kind {
                        Kind::Photo => foto
                            .original(lib, hash)
                            .and_then(|p| crate::platform::full_image(&PathBuf::from(p)))
                            .and_then(|b| decode(&b)),
                        Kind::Video => None,
                    };
                    (detail, large, full)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(v) = &mut this.viewer
                    && v.hash == item.hash
                {
                    v.detail = detail;
                    v.large = large.map(Arc::new).or(v.large.take());
                    v.full = full.map(Arc::new).or(v.full.take());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn close_viewer(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.viewer.take() {
            for img in [v.large, v.full].into_iter().flatten() {
                cx.drop_image(img, None);
            }
        }
        cx.notify();
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(v) = &self.viewer else { return };
        let next = v.index as isize + delta;
        if next >= 0 && (next as usize) < self.items.len() {
            let old = self.viewer.take();
            for img in old.into_iter().flat_map(|v| [v.large, v.full]).flatten() {
                cx.drop_image(img, None);
            }
            self.open_viewer(next as usize, window, cx);
        }
    }

    fn current(&self) -> Option<(String, String)> {
        Some((self.lib.clone()?, self.viewer.as_ref()?.hash.clone()))
    }

    fn toggle_favorite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((lib, hash)) = self.current() else {
            return;
        };
        let fav = self
            .viewer
            .as_ref()
            .and_then(|v| v.detail.as_ref())
            .is_some_and(|d| d.favorite);
        if let Err(e) = self.foto.set_favorite(lib, hash, !fav) {
            window.push_notification(e.to_string(), cx);
        }
        if let Some(d) = self.viewer.as_mut().and_then(|v| v.detail.as_mut()) {
            d.favorite = !fav;
        }
        cx.notify();
    }

    fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((lib, hash)) = self.current() else {
            return;
        };
        let deleted = self.section == Section::Deleted;
        let r = if deleted {
            self.foto.restore(lib, vec![hash])
        } else {
            self.foto.delete(lib, vec![hash])
        };
        match r {
            Ok(()) => {
                let index = self.viewer.as_ref().map(|v| v.index).unwrap_or(0);
                let mut items = (*self.items).clone();
                if index < items.len() {
                    items.remove(index);
                }
                self.items = Arc::new(items);
                self.rebuild();
                self.close_viewer(cx);
                if index < self.items.len() {
                    self.open_viewer(index, window, cx);
                }
                let msg = if deleted {
                    "Restored"
                } else {
                    "Moved to Recently deleted"
                };
                window.push_notification(msg, cx);
            }
            Err(e) => window.push_notification(e.to_string(), cx),
        }
    }

    fn info_toggle(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = &mut self.viewer {
            v.info = !v.info;
        }
        cx.notify();
    }

    fn create_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            window.push_notification("Give the library a name first", cx);
            return;
        }
        match self.foto.create_library(name) {
            Ok(info) => {
                self.lib = Some(info.id);
                self.select(Section::Folders, cx);
            }
            Err(e) => window.push_notification(e.to_string(), cx),
        }
    }

    fn join(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ticket = self.ticket_input.read(cx).value().trim().to_string();
        if ticket.is_empty() {
            return;
        }
        match self.foto.join(ticket) {
            Ok(info) => {
                self.lib = Some(info.id);
                self.ticket_input
                    .update(cx, |s, cx| s.set_value("", window, cx));
                window.push_notification("Joined. Photos appear as the other device syncs.", cx);
                self.select(Section::Photos, cx);
            }
            Err(e) => window.push_notification(e.to_string(), cx),
        }
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.clone() else { return };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Watch folder".into()),
        });
        let foto = self.foto.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            for p in paths {
                let _ = foto.add_root(lib.clone(), p.to_string_lossy().into_owned());
            }
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    fn share(&mut self, writable: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lib) = self.lib.clone() else { return };
        match self.foto.share(lib, writable) {
            Ok(ticket) => {
                self.share = qr(&ticket).map(|qr| Share {
                    ticket: ticket.into(),
                    writable,
                    qr,
                });
                cx.notify();
            }
            Err(e) => window.push_notification(e.to_string(), cx),
        }
    }

    fn set_policy(
        &mut self,
        f: impl FnOnce(&mut Policy),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut p = self.foto.policy();
        f(&mut p);
        if let Err(e) = self.foto.set_policy(p) {
            window.push_notification(e.to_string(), cx);
        }
        cx.notify();
    }

    fn status_text(&self) -> (String, bool) {
        let s = &self.status;
        let busy = s.indexing > 0 || s.thumbs_pending > 0 || s.originals_pending > 0;
        let text = if let Some(p) = &s.paused {
            p.clone()
        } else if s.indexing > 0 {
            format!("Importing {} files", s.indexing)
        } else if s.thumbs_pending > 0 {
            format!("Fetching {} previews", s.thumbs_pending)
        } else if s.originals_pending > 0 {
            format!("Downloading {} originals", s.originals_pending)
        } else {
            "Up to date".into()
        };
        let peers = match s.peers_online {
            0 => " · no devices online".to_string(),
            1 => " · 1 device online".to_string(),
            n => format!(" · {n} devices online"),
        };
        (text + &peers, busy)
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let libraries = self.foto.libraries();
        let item = |id: &'static str,
                    icon: IconName,
                    label: &'static str,
                    section: Section,
                    cx: &mut Context<Self>| {
            nav(id, icon, label, self.section == section, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.select(section, cx)))
        };
        let (status, busy) = self.status_text();
        v_flex()
            .w(px(SIDEBAR))
            .h_full()
            .flex_shrink_0()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .p_3()
            .pt(px(40.))
            .gap_1()
            .when(libraries.len() > 1, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .px_2()
                        .child("Libraries"),
                )
                .children(libraries.iter().map(|l| {
                    let id = l.id.clone();
                    nav(
                        SharedString::from(format!("lib-{}", l.id)),
                        IconName::GalleryVerticalEnd,
                        l.name.clone(),
                        self.lib.as_ref() == Some(&l.id),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.lib = Some(id.clone());
                        this.select(Section::Photos, cx);
                    }))
                }))
                .child(div().h_2())
            })
            .when_some(
                libraries.iter().find(|l| Some(&l.id) == self.lib.as_ref()),
                |this, l| {
                    this.child(
                        h_flex()
                            .px_2()
                            .pb_2()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(l.name.clone()),
                            )
                            .when(!l.writable, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("read-only"),
                                )
                            }),
                    )
                },
            )
            .child(item(
                "nav-photos",
                IconName::GalleryVerticalEnd,
                "Photos",
                Section::Photos,
                cx,
            ))
            .child(item(
                "nav-favorites",
                IconName::Heart,
                "Favourites",
                Section::Favorites,
                cx,
            ))
            .child(item(
                "nav-deleted",
                IconName::Delete,
                "Recently deleted",
                Section::Deleted,
                cx,
            ))
            .child(div().h_3())
            .child(item(
                "nav-folders",
                IconName::Folder,
                "Folders",
                Section::Folders,
                cx,
            ))
            .child(item(
                "nav-devices",
                IconName::Network,
                "Devices",
                Section::Devices,
                cx,
            ))
            .child(item(
                "nav-settings",
                IconName::Settings,
                "Settings",
                Section::Settings,
                cx,
            ))
            .child(div().flex_1())
            .child(
                h_flex()
                    .px_2()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .when(busy, |this| {
                        this.child(Icon::new(IconName::LoaderCircle).xsmall())
                    })
                    .child(status),
            )
    }

    fn render_onboarding(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = |cx: &App| {
            v_flex()
                .w(px(420.))
                .gap_3()
                .p_5()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_6()
            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Welcome to foto"))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child("Your photos stay on your devices and sync directly between them."),
            )
            .child(
                card(cx)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Start a new library"))
                    .child(Input::new(&self.name_input))
                    .child(
                        Button::new("create")
                            .primary()
                            .label("Create library")
                            .on_click(cx.listener(|this, _, window, cx| this.create_library(window, cx))),
                    ),
            )
            .child(
                card(cx)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Join a library on another device"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("On the other device open Devices → Invite a device, then paste the ticket here."),
                    )
                    .child(Input::new(&self.ticket_input))
                    .child(
                        Button::new("join")
                            .label("Join")
                            .on_click(cx.listener(|this, _, window, cx| this.join(window, cx))),
                    ),
            )
    }

    fn render_row(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.rows.get(ix) {
            Some(Row::Header(label)) => div()
                .pt_5()
                .pb_2()
                .px(px(PAD))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label.clone())
                .into_any_element(),
            Some(Row::Tiles(start, end)) => {
                let (start, end) = (*start, *end);
                let tile = px(self.tile);
                let mut tiles = Vec::new();
                for i in start..end {
                    let item = self.items[i].clone();
                    let image = self.thumbs.get(&item.hash);
                    if image.is_none() && self.thumbs.wants(&item.hash) {
                        self.load_thumb(item.hash.clone(), cx);
                    }
                    tiles.push(
                        div()
                            .id(SharedString::from(item.hash.clone()))
                            .relative()
                            .size(tile)
                            .overflow_hidden()
                            .bg(cx.theme().muted)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_viewer(i, window, cx)
                            }))
                            .when_some(image, |this, image| {
                                this.child(img(image).size_full().object_fit(ObjectFit::Cover))
                            })
                            .when(item.kind == Kind::Video, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .bottom_1()
                                        .right_1()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(black().opacity(0.55))
                                        .text_color(white())
                                        .text_xs()
                                        .child(duration(item.duration_ms)),
                                )
                            })
                            .when(item.live, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_1()
                                        .left_1()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(black().opacity(0.45))
                                        .text_color(white())
                                        .text_xs()
                                        .child("LIVE"),
                                )
                            })
                            .when(item.favorite, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .bottom_1()
                                        .left_1()
                                        .text_color(white())
                                        .child(Icon::new(IconName::Heart).xsmall()),
                                )
                            })
                            .when(!item.local, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_1()
                                        .right_1()
                                        .size_2()
                                        .rounded_full()
                                        .bg(white().opacity(0.8)),
                                )
                            }),
                    );
                }
                h_flex()
                    .px(px(PAD))
                    .pb(px(GAP))
                    .gap(px(GAP))
                    .children(tiles)
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width) - SIDEBAR - PAD * 2.;
        let cols = (((width + GAP) / (TARGET_TILE + GAP)).floor() as usize).max(1);
        self.tile = ((width - GAP * (cols as f32 - 1.)) / cols as f32).floor();
        if cols != self.cols {
            self.cols = cols;
            self.rebuild();
        }
        let title = match self.section {
            Section::Favorites => "Favourites",
            Section::Deleted => "Recently deleted",
            _ => "Photos",
        };
        let count = self.items.len();
        let empty = match self.section {
            Section::Favorites => "No favourites yet. Open a photo and press F to add one.",
            Section::Deleted => "Nothing deleted in the last 30 days.",
            _ => "No photos yet. Add a folder, or invite a device that has some.",
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px(px(PAD))
                    .pt(px(36.))
                    .pb_2()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{count} items")),
                    )
                    .child(div().flex_1())
                    .when(self.section == Section::Photos, |this| {
                        this.child(
                            Button::new("add-folder")
                                .small()
                                .icon(IconName::Plus)
                                .label("Add folder")
                                .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                        )
                    }),
            )
            .child(if count == 0 {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(empty)
                    .into_any_element()
            } else {
                list(
                    self.list.clone(),
                    cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
                )
                .flex_1()
                .into_any_element()
            })
    }

    fn render_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.viewer.as_ref().expect("viewer");
        let item = self.items.get(v.index).cloned();
        let info_w = if v.info { 300. } else { 0. };
        let vp = window.viewport_size();
        let (cw, ch) = (f32::from(vp.width) - info_w, f32::from(vp.height) - 56.);
        let (iw, ih) = item
            .as_ref()
            .map(|i| (i.w.max(1) as f32, i.h.max(1) as f32))
            .unwrap_or((4., 3.));
        let fit = (cw / iw).min(ch / ih);
        let (dw, dh) = (iw * fit * v.zoom, ih * fit * v.zoom);
        let left = (cw - dw) / 2. + f32::from(v.offset.x);
        let top = (ch - dh) / 2. + f32::from(v.offset.y);
        let image = v.full.clone().or(v.large.clone());
        let detail = v.detail.clone();
        let deleted = self.section == Section::Deleted;
        let favorite = detail.as_ref().is_some_and(|d| d.favorite);
        let path = detail.as_ref().and_then(|d| d.path.clone());
        let is_video = item.as_ref().is_some_and(|i| i.kind == Kind::Video);
        let title = detail
            .as_ref()
            .map(|d| format!("{}  ·  {}", d.name, day(d.taken_at, "%-d %B %Y, %H:%M")))
            .unwrap_or_default();
        let bar = h_flex()
            .h(px(56.))
            .pl(px(80.))
            .pr_3()
            .gap_2()
            .bg(black().opacity(0.85))
            .text_color(white())
            .child(
                dark_button("viewer-back", IconName::ChevronLeft, Some("Back"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_viewer(cx))),
            )
            .child(div().flex_1().text_sm().text_ellipsis().child(title))
            .when(is_video && path.is_some(), |this| {
                let p = path.clone().unwrap_or_default();
                this.child(
                    dark_button("viewer-play", IconName::Play, Some("Play"))
                        .on_click(move |_, _, cx| cx.open_with_system(std::path::Path::new(&p))),
                )
            })
            .child(
                dark_button(
                    "viewer-fav",
                    if favorite {
                        IconName::Heart
                    } else {
                        IconName::HeartOff
                    },
                    None,
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_favorite(window, cx))),
            )
            .child(
                dark_button("viewer-info", IconName::Info, None)
                    .on_click(cx.listener(|this, _, _, cx| this.info_toggle(cx))),
            )
            .when_some(path.clone(), |this, p| {
                this.child(
                    dark_button("viewer-reveal", IconName::FolderOpen, None)
                        .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&p))),
                )
            })
            .child(
                dark_button(
                    "viewer-delete",
                    if deleted {
                        IconName::Undo2
                    } else {
                        IconName::Delete
                    },
                    Some(if deleted { "Restore" } else { "Delete" }),
                )
                .on_click(cx.listener(|this, _, window, cx| this.delete_current(window, cx))),
            );
        let stage = div()
            .id("viewer-stage")
            .relative()
            .w(px(cw))
            .h(px(ch))
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| {
                let dy = f32::from(ev.delta.pixel_delta(px(16.)).y);
                if let Some(v) = &mut this.viewer {
                    v.zoom = (v.zoom * (1. + dy * 0.005)).clamp(1., 8.);
                    if v.zoom == 1. {
                        v.offset = Point::default();
                    }
                }
                let _ = window;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    if let Some(v) = &mut this.viewer {
                        if ev.click_count == 2 {
                            v.zoom = if v.zoom > 1. { 1. } else { 2.5 };
                            v.offset = Point::default();
                        }
                        v.drag = Some(ev.position);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if let Some(v) = &mut this.viewer {
                        v.drag = None;
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(v) = &mut this.viewer
                    && let Some(start) = v.drag
                    && ev.pressed_button == Some(MouseButton::Left)
                    && v.zoom > 1.
                {
                    v.offset += ev.position - start;
                    v.drag = Some(ev.position);
                    cx.notify();
                }
            }))
            .when_some(image, |this, image| {
                this.child(
                    img(image)
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(dw))
                        .h(px(dh)),
                )
            })
            .when(is_video, |this| {
                this.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .p_3()
                                .rounded_full()
                                .bg(black().opacity(0.5))
                                .child(Icon::new(IconName::Play).large().text_color(white())),
                        ),
                )
            })
            .child(
                div().absolute().left_2().top(px(ch / 2. - 20.)).child(
                    dark_button("viewer-prev", IconName::ChevronLeft, None)
                        .on_click(cx.listener(|this, _, window, cx| this.step(-1, window, cx))),
                ),
            )
            .child(
                div().absolute().right_2().top(px(ch / 2. - 20.)).child(
                    dark_button("viewer-next", IconName::ChevronRight, None)
                        .on_click(cx.listener(|this, _, window, cx| this.step(1, window, cx))),
                ),
            );
        let info = detail.map(|d| {
            let row = |k: &'static str, val: String| {
                v_flex()
                    .gap_0p5()
                    .child(div().text_xs().text_color(gpui_kit::rgb(0x9a9a9a)).child(k))
                    .child(div().text_sm().child(val))
            };
            v_flex()
                .w(px(300.))
                .h(px(ch))
                .p_4()
                .gap_3()
                .bg(rgb(0x1c1c1e))
                .text_color(white())
                .child(row("Name", d.name.clone()))
                .child(row("Taken", day(d.taken_at, "%-d %B %Y, %H:%M")))
                .child(row("Dimensions", format!("{} × {}", d.w, d.h)))
                .child(row("Size", size(d.size)))
                .when_some(d.camera.clone(), |this, c| this.child(row("Camera", c)))
                .when(d.duration_ms > 0, |this| {
                    this.child(row("Duration", duration(d.duration_ms)))
                })
                .child(row("Added from", d.origin.clone()))
                .child(row(
                    "Originals on",
                    if d.holders.is_empty() {
                        "No device yet".into()
                    } else {
                        d.holders.join(", ")
                    },
                ))
                .child(row(
                    "On this device",
                    d.path.clone().unwrap_or_else(|| "Not downloaded".into()),
                ))
        });
        v_flex().absolute().inset_0().bg(black()).child(bar).child(
            h_flex()
                .items_start()
                .child(stage)
                .when(self.viewer.as_ref().is_some_and(|v| v.info), |this| {
                    this.children(info)
                }),
        )
    }

    fn render_folders(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let lib = self.lib.clone().unwrap_or_default();
        let roots: Vec<_> = self
            .foto
            .roots()
            .into_iter()
            .filter(|r| r.library == lib)
            .collect();
        let library_dir = self.foto.library_dir(lib.clone());
        panel("Folders", cx)
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("foto watches these folders and imports new photos and videos automatically. Files stay where they are."),
            )
            .children(roots.into_iter().map(|r| {
                let path = r.path.clone();
                let lib = lib.clone();
                let is_library = r.path == library_dir;
                h_flex()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(Icon::new(IconName::Folder))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_sm().child(r.path.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if !r.online {
                                        "Offline (drive not connected). Nothing is deleted while it is away.".to_string()
                                    } else if is_library {
                                        format!("{} files · photos from other devices are saved here", r.files)
                                    } else {
                                        format!("{} files", r.files)
                                    }),
                            ),
                    )
                    .child({
                        let p = path.clone();
                        Button::new(SharedString::from(format!("reveal-{path}")))
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&p)))
                    })
                    .when(!is_library, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("remove-{path}")))
                                .ghost()
                                .small()
                                .label("Stop watching")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Err(e) = this.foto.remove_root(lib.clone(), path.clone()) {
                                        window.push_notification(e.to_string(), cx);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
            }))
            .child(
                h_flex().child(
                    Button::new("folders-add")
                        .primary()
                        .icon(IconName::Plus)
                        .label("Add folder…")
                        .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                ),
            )
    }

    fn render_devices(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let lib = self.lib.clone().unwrap_or_default();
        let devices = self.foto.devices(lib.clone());
        let writable = self
            .foto
            .libraries()
            .iter()
            .any(|l| l.id == lib && l.writable);
        panel("Devices", cx)
            .child(section_title("Invite a device"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Scan the code with the foto app on your phone, or copy the ticket to another computer. Anyone with a full-access ticket can add and delete photos."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("invite-write")
                            .primary()
                            .label("Invite one of my devices")
                            .disabled(!writable)
                            .on_click(cx.listener(|this, _, window, cx| this.share(true, window, cx))),
                    )
                    .child(
                        Button::new("invite-read")
                            .label("Share read-only")
                            .on_click(cx.listener(|this, _, window, cx| this.share(false, window, cx))),
                    ),
            )
            .when_some(self.share.as_ref(), |this, share| {
                let ticket = share.ticket.clone();
                this.child(
                    h_flex()
                        .gap_4()
                        .items_start()
                        .child(img(share.qr.clone()).size(px(220.)).rounded_md())
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_2()
                                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(if share.writable {
                                    "Full access ticket"
                                } else {
                                    "Read-only ticket"
                                }))
                                .child(
                                    div()
                                        .w_full()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_xs()
                                        .font_family("Menlo")
                                        .p_2()
                                        .rounded_md()
                                        .bg(cx.theme().muted)
                                        .child(share.ticket.clone()),
                                )
                                .child(
                                    h_flex().child(
                                        Button::new("copy-ticket")
                                            .small()
                                            .icon(IconName::Copy)
                                            .label("Copy ticket")
                                            .on_click(move |_, window, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(ticket.to_string()));
                                                window.push_notification("Ticket copied", cx);
                                            }),
                                    ),
                                ),
                        ),
                )
            })
            .child(section_title("Devices in this library"))
            .children(devices.into_iter().map(|d| {
                let originals = if d.originals_all { format!("{} of {} originals", d.held, d.total) } else { "Downloads originals on demand".into() };
                h_flex()
                    .gap_3()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(div().size_2().rounded_full().bg(if d.online { cx.theme().success } else { cx.theme().muted_foreground }))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_sm().child(if d.this_device { format!("{} (this device)", d.name) } else { d.name.clone() }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} · {}{}{}",
                                        d.platform,
                                        originals,
                                        if d.archive { " · archive" } else { "" },
                                        if d.online || d.this_device { String::new() } else { format!(" · last seen {}", day(d.last_seen, "%-d %b %H:%M")) }
                                    )),
                            ),
                    )
            }))
            .child(section_title("Join another library"))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.ticket_input)))
                    .child(Button::new("join-other").label("Join").on_click(cx.listener(|this, _, window, cx| this.join(window, cx)))),
            )
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let policy = self.foto.policy();
        let lib = self.lib.clone().unwrap_or_default();
        let library_dir = self.foto.library_dir(lib);
        let autostart = crate::platform::autostart_enabled();
        panel("Settings", cx)
            .child(section_title("This device"))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(260.)).child(Input::new(&self.device_input)))
                    .child(Button::new("save-name").label("Rename").on_click(cx.listener(|this, _, window, cx| {
                        let name = this.device_input.read(cx).value().trim().to_string();
                        if !name.is_empty()
                            && let Err(e) = this.foto.set_device_name(name)
                        {
                            window.push_notification(e.to_string(), cx);
                        }
                        cx.notify();
                    }))),
            )
            .child(
                Switch::new("originals-all")
                    .checked(policy.originals_all)
                    .label("Keep every original on this computer")
                    .on_click(cx.listener(|this, v: &bool, window, cx| {
                        let v = *v;
                        this.set_policy(|p| p.originals_all = v, window, cx)
                    })),
            )
            .child(
                Switch::new("archive")
                    .checked(policy.archive)
                    .label("Archive: keep files that are deleted on other devices")
                    .on_click(cx.listener(|this, v: &bool, window, cx| {
                        let v = *v;
                        this.set_policy(|p| p.archive = v, window, cx)
                    })),
            )
            .child(
                Switch::new("autostart")
                    .checked(autostart)
                    .label("Open foto at login so it keeps syncing")
                    .on_click(cx.listener(|_, v: &bool, window, cx| {
                        if let Err(e) = crate::platform::set_autostart(*v) {
                            window.push_notification(e.to_string(), cx);
                        }
                        cx.notify();
                    })),
            )
            .child(section_title("Library folder"))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_sm().child(library_dir.clone()))
                    .child(
                        Button::new("reveal-library")
                            .small()
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&library_dir))),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Photos from your other devices are saved here, sorted by year and month. Deleting a file here deletes it on every device (it goes to Recently deleted for 30 days)."),
            )
            .child(section_title("Device ID"))
            .child(div().text_xs().font_family("Menlo").child(self.foto.device_id()))
    }
}

fn nav(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    h_flex()
        .id(id.into())
        .w_full()
        .gap_2()
        .px_2()
        .py_1p5()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .text_color(cx.theme().sidebar_foreground)
        .when(selected, |this| {
            this.bg(cx.theme().sidebar_accent)
                .text_color(cx.theme().sidebar_accent_foreground)
        })
        .hover(|this| this.bg(cx.theme().sidebar_accent))
        .child(Icon::new(icon).small())
        .child(label.into())
}

fn dark_button(id: &'static str, icon: IconName, label: Option<&'static str>) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap_1()
        .px_2()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(white())
        .hover(|this| this.bg(white().opacity(0.12)))
        .child(Icon::new(icon).text_color(white()))
        .when_some(label, |this, l| this.child(l))
}

fn panel(title: &'static str, cx: &App) -> Div {
    v_flex()
        .w_full()
        .max_w(px(860.))
        .px(px(24.))
        .pt(px(36.))
        .pb_6()
        .gap_3()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .child(title),
        )
}

fn section_title(t: &'static str) -> Div {
    div().pt_3().font_weight(FontWeight::SEMIBOLD).child(t)
}

impl Render for FotoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.lib.is_none() {
            self.render_onboarding(cx).into_any_element()
        } else {
            let main = match self.section {
                Section::Photos | Section::Favorites | Section::Deleted => {
                    self.render_grid(window, cx).into_any_element()
                }
                Section::Folders => div()
                    .id("folders")
                    .size_full()
                    .overflow_y_scroll()
                    .child(self.render_folders(cx))
                    .into_any_element(),
                Section::Devices => div()
                    .id("devices")
                    .size_full()
                    .overflow_y_scroll()
                    .child(self.render_devices(cx))
                    .into_any_element(),
                Section::Settings => div()
                    .id("settings")
                    .size_full()
                    .overflow_y_scroll()
                    .child(self.render_settings(cx))
                    .into_any_element(),
            };
            h_flex()
                .size_full()
                .child(self.render_sidebar(cx))
                .child(div().flex_1().min_w_0().h_full().child(main))
                .into_any_element()
        };
        div()
            .id("foto")
            .track_focus(&self.focus)
            .when(self.viewer.is_some(), |this| {
                this.key_context(VIEWER)
                    .on_action(cx.listener(|this, _: &Close, _, cx| this.close_viewer(cx)))
                    .on_action(cx.listener(|this, _: &Prev, window, cx| this.step(-1, window, cx)))
                    .on_action(cx.listener(|this, _: &Next, window, cx| this.step(1, window, cx)))
                    .on_action(cx.listener(|this, _: &ToggleInfo, _, cx| this.info_toggle(cx)))
                    .on_action(cx.listener(|this, _: &Favorite, window, cx| {
                        this.toggle_favorite(window, cx)
                    }))
                    .on_action(
                        cx.listener(|this, _: &Delete, window, cx| this.delete_current(window, cx)),
                    )
            })
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(body)
            .when(self.viewer.is_some(), |this| {
                this.child(self.render_viewer(window, cx))
            })
    }
}
