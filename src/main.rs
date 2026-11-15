mod app;
mod mac;
mod markdown;
mod settings;
mod theme;

use app::Prompter;
use gpui::prelude::*;
use gpui::{
    point, px, size, App, Bounds, Pixels, Size, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions,
};
use gpui_platform::application;
use settings::Settings;

fn main() {
    let script_arg = std::env::args().nth(1);
    application().run(move |cx: &mut App| {
        #[cfg(target_os = "macos")]
        cx.set_activation_policy(gpui::ActivationPolicy::Accessory);

        let settings = Settings::load();
        let window_size = size(px(settings.width), px(settings.height));
        let bounds = restored_bounds(&settings, window_size, cx);

        let prompter_settings = settings.clone();
        let script_arg = script_arg.clone();
        cx.open_window(window_options(bounds), move |window, cx| {
            mac::configure(window);
            cx.new(|cx| Prompter::new(window, cx, prompter_settings, script_arg))
        })
        .expect("the prompter window could not be created");
    });
}

fn window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: None,
        }),
        // On macOS this is a non-activating panel at pop-up level, which floats
        // over other apps and over their full-screen spaces without taking focus.
        kind: WindowKind::PopUp,
        focus: true,
        show: true,
        is_movable: true,
        // We draw our own control strip and move the window explicitly via
        // `start_window_move`; otherwise AppKit treats the titlebar strip
        // (including the opacity slider) as a native drag region.
        app_owns_titlebar_drag: true,
        is_resizable: true,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_min_size: Some(size(px(320.0), px(140.0))),
        #[cfg(not(target_os = "macos"))]
        window_decorations: Some(gpui::WindowDecorations::Client),
        ..Default::default()
    }
}

/// Reopen where the prompter was, unless that place is gone (a display
/// disconnected), in which case centre it on the primary display.
fn restored_bounds(settings: &Settings, window_size: Size<Pixels>, cx: &mut App) -> Bounds<Pixels> {
    let (Some(x), Some(y)) = (settings.x, settings.y) else {
        return Bounds::centered(None, window_size, cx);
    };
    let candidate = Bounds {
        origin: point(px(x), px(y)),
        size: window_size,
    };
    let visible = cx.displays().iter().any(|display| {
        let screen = display.bounds();
        let overlap_x = candidate.origin.x < screen.origin.x + screen.size.width - px(120.0)
            && candidate.origin.x + candidate.size.width > screen.origin.x + px(120.0);
        let overlap_y = candidate.origin.y < screen.origin.y + screen.size.height - px(60.0)
            && candidate.origin.y + candidate.size.height > screen.origin.y + px(60.0);
        overlap_x && overlap_y
    });
    if visible {
        candidate
    } else {
        Bounds::centered(None, window_size, cx)
    }
}
