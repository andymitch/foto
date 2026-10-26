mod app;
#[cfg(target_os = "macos")]
mod macos;
mod platform;
mod thumbs;

use std::sync::Arc;

use foto_core::{Config, Foto};
use gpui_kit::component::TitleBar;
use gpui_kit::*;

actions!(foto, [Quit, OpenWindow]);

struct Shared(Arc<Foto>);

impl Global for Shared {}

fn open_main(cx: &mut App) {
    if let Some(window) = cx.windows().first() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
        return;
    }
    let foto = cx.global::<Shared>().0.clone();
    let options = WindowOptions {
        titlebar: Some(TitleBar::title_bar_options()),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1240.), px(820.)),
            cx,
        ))),
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| app::FotoApp::new(foto, window, cx))
    })
    .expect("failed to open window");
    cx.activate(true);
}

#[cfg(target_os = "macos")]
fn tray(cx: &mut App) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    let menu = Menu::new();
    let open = MenuItem::new("Open foto", true, None);
    let quit = MenuItem::new("Quit foto", true, None);
    let _ = menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]);
    let side = 18u32;
    let rgba: Vec<u8> = (0..side * side)
        .flat_map(|i| {
            let (x, y) = (i % side, i / side);
            let frame = x == 1 || x == side - 2 || y == 3 || y == side - 4;
            let inside = (3..side - 3).contains(&y) && (1..side - 1).contains(&x);
            let lens = ((x as i32 - 9).pow(2) + (y as i32 - 9).pow(2)) <= 12;
            let on = inside && (frame || lens);
            [0, 0, 0, if on { 255 } else { 0 }]
        })
        .collect();
    let icon = tray_icon::Icon::from_rgba(rgba, side, side).ok();
    let built = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("foto")
        .with_icon_as_template(true)
        .with_icon(icon.expect("tray icon"))
        .build();
    let Ok(tray) = built else { return };
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    cx.spawn(async move |cx| {
        let _tray = tray;
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                cx.update(|cx| {
                    if ev.id == open_id {
                        open_main(cx);
                    } else if ev.id == quit_id {
                        cx.quit();
                    }
                });
            }
        }
    })
    .detach();
}

fn main() {
    let platform = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let foto = match Foto::open(Config::desktop(platform), Arc::new(platform::Desktop)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("foto: {e}");
            std::process::exit(1);
        }
    };
    let background = std::env::args().any(|a| a == "--background") && cfg!(target_os = "macos");
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(Shared(foto.clone()));
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-n", OpenWindow, None),
            ]);
            app::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &OpenWindow, cx| open_main(cx));
            cx.set_menus([Menu::new("foto").items([
                MenuItem::action("Open window", OpenWindow),
                MenuItem::separator(),
                MenuItem::action("Quit foto", Quit),
            ])]);
            let quitting = foto.clone();
            cx.on_app_quit(move |_| {
                let foto = quitting.clone();
                async move { foto.shutdown() }
            })
            .detach();
            #[cfg(target_os = "macos")]
            tray(cx);
            #[cfg(not(target_os = "macos"))]
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            if !background {
                open_main(cx);
            }
        });
}
