//! The prompter panel: state, the control strip, and the script.

use crate::settings::{MAX_FONT, MAX_OPACITY, MIN_FONT, MIN_OPACITY, Settings};
use crate::theme;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, FocusHandle, FontWeight, Hsla, IntoElement,
    KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, Render,
    ScrollHandle, SharedString, Subscription, Window, canvas, div, point, px, relative,
};
use gpui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Width of the opacity track, in points.
const TRACK: f32 = 76.0;
/// Height of the opacity control's hit area.
const TRACK_H: f32 = 18.0;

/// Offered at the top of the family list when they are installed. The first
/// that resolves also becomes the default face on a fresh install.
const FONT_SHORTLIST: &[&str] = &[
    "Geist Standard",
    "Google Sans Text",
    "Literata",
    "SF Pro Text",
    "SF Pro Display",
    "Helvetica Neue",
    "Inter",
    "Lato",
    "Source Serif 4",
    "Bitter",
    "Alegreya",
    "Berkeley Mono",
    "Geist Mono",
    "JetBrains Mono",
    "Avenir Next",
    "IBM Plex Sans",
    "Atkinson Hyperlegible",
    "Segoe UI",
    "Cantarell",
    "DejaVu Sans",
];

/// Face used before the user has chosen one.
const DEFAULT_FACE: &str = "Geist Standard";

const SAMPLE: &str = "Paste a script with Command V, or open a file with Command O.\n\nDrag the strip at the top to move this window. Pull any corner to resize it. The slider in the strip fades the panel behind the words.\n\nPress Space to roll the script.";

pub struct Prompter {
    settings: Settings,
    family: Option<SharedString>,
    families: Vec<SharedString>,
    paragraphs: Vec<SharedString>,
    scroll: ScrollHandle,
    font_scroll: ScrollHandle,
    font_menu_open: bool,
    hovered: bool,
    dragging_opacity: bool,
    opacity_bounds: Rc<Cell<Bounds<Pixels>>>,
    playing: bool,
    notice: Option<(SharedString, std::time::Instant)>,
    font_query: String,
    font_cursor: usize,
    focus: FocusHandle,
    save_generation: u64,
    _bounds_observer: Option<Subscription>,
}

impl Prompter {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        mut settings: Settings,
        script_arg: Option<String>,
    ) -> Self {
        let families = installed_families(cx);
        let family = resolve_family(&settings, &families);

        let mut paragraphs = Vec::new();
        if let Some(path) = script_arg.or_else(|| settings.script_path.clone()) {
            if let Some(text) = load_script(&path) {
                paragraphs = split_paragraphs(&text);
                settings.script_path = Some(path);
            }
        }
        if paragraphs.is_empty() {
            paragraphs = split_paragraphs(SAMPLE);
        }

        let mut this = Self {
            settings,
            family,
            families,
            paragraphs,
            scroll: ScrollHandle::new(),
            font_scroll: ScrollHandle::new(),
            font_menu_open: false,
            hovered: false,
            dragging_opacity: false,
            opacity_bounds: Rc::new(Cell::new(Bounds::default())),
            playing: false,
            notice: None,
            font_query: String::new(),
            font_cursor: 0,
            focus: cx.focus_handle(),
            save_generation: 0,
            _bounds_observer: None,
        };
        if std::env::var_os("PROMPTER_DEBUG_WINDOW").is_some() {
            eprintln!(
                "fonts: {} families installed, using {:?}",
                this.families.len(),
                this.family
            );
        }
        this.focus.focus(window, cx);
        let bounds = window.bounds();
        this.settings.width = f32::from(bounds.size.width);
        this.settings.height = f32::from(bounds.size.height);
        this.settings.x = Some(f32::from(bounds.origin.x));
        this.settings.y = Some(f32::from(bounds.origin.y));
        this._bounds_observer = Some(cx.observe_window_bounds(window, |this, window, cx| {
            let bounds = window.bounds();
            this.settings.width = f32::from(bounds.size.width);
            this.settings.height = f32::from(bounds.size.height);
            this.settings.x = Some(f32::from(bounds.origin.x));
            this.settings.y = Some(f32::from(bounds.origin.y));
            this.save_debounced(window, cx);
        }));
        this
    }

    fn save_debounced(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_generation += 1;
        let generation = self.save_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            let _ = this.update(cx, |this, _| {
                if this.save_generation == generation {
                    this.settings.save();
                }
            });
        })
        .detach();
    }

    fn bump_font(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.font_size = (self.settings.font_size + delta).clamp(MIN_FONT, MAX_FONT);
        self.save_debounced(window, cx);
        cx.notify();
    }

    fn set_opacity_at(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.opacity_bounds.get();
        let inset = (f32::from(bounds.size.width) - TRACK) / 2.0;
        let span = f32::from(x) - f32::from(bounds.origin.x) - inset;
        let fraction = (span / TRACK).clamp(0.0, 1.0);
        self.settings.opacity = MIN_OPACITY + fraction * (MAX_OPACITY - MIN_OPACITY);
        self.save_debounced(window, cx);
        cx.notify();
    }

    fn choose_family(&mut self, family: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.font_family = family.as_ref().map(|f| f.to_string());
        self.family = family.clone();
        self.font_menu_open = false;
        self.font_query.clear();
        self.font_cursor = 0;
        let name = family.unwrap_or_else(|| SharedString::from("System"));
        self.flash(name, window, cx);
        self.save_debounced(window, cx);
    }

    fn choose_weight(&mut self, weight: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.font_weight = weight;
        self.save_debounced(window, cx);
        cx.notify();
    }

    fn pick_filtered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.font_query.trim().to_lowercase();
        let matches: Vec<SharedString> = self
            .families
            .iter()
            .filter(|family| query.is_empty() || family.to_lowercase().contains(&query))
            .cloned()
            .collect();
        if matches.is_empty() {
            return;
        }
        let index = self.font_cursor.min(matches.len() - 1);
        let family = matches[index].clone();
        self.choose_family(Some(family), window, cx);
    }

    fn toggle_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.playing = !self.playing;
        cx.notify();
        if !self.playing {
            return;
        }
        let speed = self.settings.scroll_speed;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let keep_going = this.update_in(cx, |this, _window, cx| {
                    if !this.playing {
                        return false;
                    }
                    let offset = this.scroll.offset();
                    let max = this.scroll.max_offset();
                    if offset.y >= max.y {
                        this.playing = false;
                        cx.notify();
                        return false;
                    }
                    let step = speed / 60.0;
                    this.scroll.set_offset(point(px(0.0), offset.y + px(step)));
                    cx.notify();
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn scroll_by(&mut self, lines: f32, cx: &mut Context<Self>) {
        let offset = f32::from(self.scroll.offset().y);
        let limit = f32::from(self.scroll.max_offset().y);
        let step = self.settings.font_size * 1.45 * lines;
        let next = (offset + step).clamp(0.0, limit.max(0.0));
        self.scroll.set_offset(point(px(0.0), px(next)));
        cx.notify();
    }

    /// Show a short status in the strip, then fall back to the family name.
    fn flash(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = Some((text, std::time::Instant::now()));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                let stale = this
                    .notice
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(1400));
                if stale {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn paste_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(text) = item.text() {
                if !text.trim().is_empty() {
                    self.paragraphs = split_paragraphs(&text);
                    self.scroll.set_offset(point(px(0.0), px(0.0)));
                    self.settings.script_path = None;
                    self.flash(SharedString::from("Pasted"), window, cx);
                    self.save_debounced(window, cx);
                }
            }
        }
    }

    fn open_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(SharedString::from("Open a script")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let text = std::fs::read_to_string(&path).ok();
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(text) = text {
                    this.paragraphs = split_paragraphs(&text);
                    this.scroll.set_offset(point(px(0.0), px(0.0)));
                    this.settings.script_path = Some(path.to_string_lossy().to_string());
                    this.save_debounced(window, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let platform = keystroke.modifiers.platform;
        let key = keystroke.key.as_str();
        if platform {
            match key {
                "q" => cx.quit(),
                "v" => self.paste_script(window, cx),
                "o" => self.open_script(window, cx),
                "=" | "+" => self.bump_font(2.0, window, cx),
                "-" => self.bump_font(-2.0, window, cx),
                _ => {}
            }
            return;
        }
        if self.font_menu_open {
            match key {
                "escape" => {
                    self.font_menu_open = false;
                    self.font_query.clear();
                    self.font_cursor = 0;
                    cx.notify();
                }
                "backspace" => {
                    if self.font_query.pop().is_some() {
                        self.font_cursor = 0;
                        cx.notify();
                    }
                }
                "enter" => self.pick_filtered(window, cx),
                "down" => {
                    self.font_cursor = self.font_cursor.saturating_add(1);
                    cx.notify();
                }
                "up" => {
                    self.font_cursor = self.font_cursor.saturating_sub(1);
                    cx.notify();
                }
                _ => {
                    if let Some(text) = keystroke.key_char.as_ref() {
                        if !text.is_empty()
                            && !keystroke.modifiers.control
                            && !keystroke.modifiers.alt
                        {
                            self.font_query.push_str(text);
                            self.font_cursor = 0;
                            cx.notify();
                        }
                    }
                }
            }
            return;
        }
        match key {
            "space" => self.toggle_play(window, cx),
            "up" => self.scroll_by(-1.0, cx),
            "down" => self.scroll_by(1.0, cx),
            "escape" => {
                if self.font_menu_open {
                    self.font_menu_open = false;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn header(&self, chrome: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let opacity = self.settings.opacity;
        let bounds_cell = self.opacity_bounds.clone();
        let fill = TRACK * (opacity - MIN_OPACITY) / (MAX_OPACITY - MIN_OPACITY);
        let knob_x = fill.max(0.0);

        div()
            .id("header")
            .h(px(theme::STRIP))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .pl(px(7.0))
            .pr(px(8.0))
            .gap(px(3.0))
            .border_b_1()
            .border_color(theme::faint(0.07))
            .child(
                div()
                    .id("grip")
                    .h_full()
                    .w(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::OpenHand)
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, window, _| {
                        window.start_window_move()
                    })
                    .opacity(chrome)
                    .child(grip_dots()),
            )
            .child(
                div()
                    .id("drag-space")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::OpenHand)
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, window, _| {
                        window.start_window_move()
                    })
                    .children(self.notice.as_ref().map(|(text, _)| {
                        div()
                            .text_size(px(10.5))
                            .text_color(theme::faint(0.66))
                            .font_weight(FontWeight::MEDIUM)
                            .child(text.clone())
                    })),
            )
            .child(
                div()
                    .id("opacity")
                    .relative()
                    .w(px(TRACK + 8.0))
                    .h(px(TRACK_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.dragging_opacity = true;
                            this.set_opacity_at(event.position.x, window, cx);
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _window, _cx| bounds_cell.set(bounds),
                            |_bounds, _: (), _window, _cx| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .w(px(TRACK))
                            .h(px(3.0))
                            .rounded_full()
                            .bg(theme::faint(0.16))
                            .relative()
                            .child(
                                div()
                                    .h_full()
                                    .w(px(fill))
                                    .rounded_full()
                                    .bg(theme::faint(0.62)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-3.5))
                                    .left(px(knob_x - 5.0))
                                    .size(px(10.0))
                                    .rounded_full()
                                    .bg(theme::text())
                                    .opacity(chrome),
                            ),
                    ),
            )
            .child(divider(chrome))
            .child(font_size_group(self.settings.font_size, chrome, cx))
            .child(divider(chrome))
            .child(
                div()
                    .id("family")
                    .h(px(22.0))
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    .text_size(px(12.5))
                    .text_color(theme::faint(0.80))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|this| this.bg(theme::control_hover(1.0)).text_color(theme::faint(1.0)))
                    .opacity(chrome)
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                        this.font_menu_open = !this.font_menu_open;
                        cx.notify();
                    }))
                    .child("Aa"),
            )
            .child(divider(chrome))
            .child(
                div()
                    .id("close")
                    .size(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    .text_size(px(13.0))
                    .text_color(theme::faint(0.80))
                    .hover(|this| this.bg(theme::control_hover(1.0)).text_color(theme::faint(1.0)))
                    .opacity(chrome)
                    .on_click(|_: &gpui::ClickEvent, _window, cx| cx.quit())
                    .child("×"),
            )
    }

    fn font_menu(&mut self, available: f32, cx: &mut Context<Self>) -> AnyElement {
        let query = self.font_query.trim().to_lowercase();
        let active = self.family.clone();

        let matches: Vec<SharedString> = self
            .families
            .iter()
            .filter(|family| query.is_empty() || family.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let cursor = self.font_cursor.min(matches.len().saturating_sub(1));

        let mut rows: Vec<AnyElement> = Vec::with_capacity(matches.len() + 1);
        if query.is_empty() || "system".contains(&query) {
            rows.push(system_font_row(active.is_none(), false, cx));
        }
        for (index, family) in matches.iter().enumerate() {
            let selected = active.as_ref() == Some(family);
            let highlighted = !query.is_empty() && index == cursor;
            let picked = family.clone();
            rows.push(
                font_row_element(
                    family.to_string(),
                    selected,
                    highlighted,
                    cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                        this.choose_family(Some(picked.clone()), window, cx)
                    }),
                )
                .into_any_element(),
            );
        }
        if matches.is_empty() {
            rows.push(
                div()
                    .h(px(24.0))
                    .px(px(9.0))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(theme::faint(0.34))
                    .child("Nothing matches")
                    .into_any_element(),
            );
        }

        let list_height =
            (rows.len() as f32 * 23.0 + 6.0).min((available - 84.0).max(46.0));
        let empty = self.font_query.is_empty();
        let search_text = if empty {
            SharedString::from("Search fonts")
        } else {
            SharedString::from(format!("{}|", self.font_query))
        };
        let weight_name = WEIGHTS
            .iter()
            .find(|(value, _)| (self.settings.font_weight - value).abs() < 1.0)
            .map(|(_, name)| *name)
            .unwrap_or("Variable");

        div()
            .id("font-menu")
            .absolute()
            .top(px(theme::STRIP - 2.0))
            .right(px(8.0))
            .w(px(252.0))
            .max_h(px(available.max(96.0)))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .border_1()
            .border_color(theme::faint(0.14))
            .bg(gpui::hsla(220.0 / 360.0, 0.16, 0.085, 0.985))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .id("font-search")
                    .mx(px(7.0))
                    .mt(px(7.0))
                    .h(px(24.0))
                    .px(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(6.0))
                    .bg(theme::faint(0.07))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(theme::faint(if empty { 0.36 } else { 0.92 }))
                            .child(search_text),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .mx(px(7.0))
                    .mt(px(7.0))
                    .children(weight_chips(self.settings.font_weight, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme::faint(0.34))
                            .child(weight_name),
                    ),
            )
            .child(
                div()
                    .h(px(1.0))
                    .mx(px(7.0))
                    .mt(px(7.0))
                    .mb(px(5.0))
                    .bg(theme::faint(0.09)),
            )
            .child(
                div()
                    .id("font-list")
                    .h(px(list_height))
                    .overflow_y_scroll()
                    .track_scroll(&self.font_scroll)
                    .pb(px(6.0))
                    .children(rows),
            )
            .into_any_element()
    }

    fn script_body(&self) -> impl IntoElement {
        let size = self.settings.font_size;
        let family = self.family.clone();
        let paragraphs = self.paragraphs.clone();
        let gap = (size * 0.62).max(10.0);

        div()
            .relative()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .child(
                div()
                    .id("script")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(26.0))
                    .pt(px(size * 0.52))
                    .pb(px(size * 1.2))
                    .when_some(family, |this, family| this.font_family(family))
                    .text_size(px(size))
                    .line_height(relative(1.46))
                    .font_weight(FontWeight(self.settings.font_weight))
                    .text_color(theme::text())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(gap))
                            .children(paragraphs),
                    ),
            )
    }
}

impl Render for Prompter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = if self.hovered { 1.0 } else { 0.52 };
        let alpha = self.settings.opacity;
        let menu_open = self.font_menu_open;
        let menu_height = f32::from(window.bounds().size.height) - theme::STRIP - 12.0;
        let menu = if menu_open { Some(self.font_menu(menu_height, cx)) } else { None };
        let body = self.script_body();
        let header = self.header(chrome, cx);

        div()
            .id("prompter")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS))
            .overflow_hidden()
            .bg(theme::backdrop(alpha))
            .border_1()
            .border_color(theme::border())
            .text_color(theme::text())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if this.hovered != *hovered {
                    this.hovered = *hovered;
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.dragging_opacity {
                    this.set_opacity_at(event.position.x, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    if this.dragging_opacity {
                        this.dragging_opacity = false;
                        this.save_debounced(window, cx);
                    }
                }),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                if this.font_menu_open {
                    this.font_menu_open = false;
                    cx.notify();
                }
            }))
            .child(header)
            .child(body)
            .children(menu)
    }
}

fn glyph_button(
    id: &'static str,
    label: &'static str,
    chrome: f32,
    size: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .text_size(px(size))
        .text_color(theme::faint(0.80))
        .hover(|this| this.bg(theme::control_hover(1.0)).text_color(theme::faint(1.0)))
        .opacity(chrome)
        .child(label)
}

fn divider(chrome: f32) -> impl IntoElement {
    div()
        .w(px(1.0))
        .h(px(13.0))
        .mx(px(5.0))
        .bg(theme::faint(0.14))
        .opacity(chrome)
}

fn font_size_group(size: f32, chrome: f32, cx: &mut Context<Prompter>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(1.0))
        .child(
            glyph_button("font-smaller", "−", chrome, 14.0)
                .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.bump_font(-2.0, window, cx)
                })),
        )
        .child(
            div()
                .w(px(24.0))
                .flex()
                .justify_center()
                .text_size(px(11.0))
                .text_color(theme::faint(0.62))
                .child(format!("{}", size.round() as i32)),
        )
        .child(
            glyph_button("font-larger", "+", chrome, 14.0)
                .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.bump_font(2.0, window, cx)
                })),
        )
}

fn grip_dots() -> impl IntoElement {
    let mut rows = Vec::new();
    for _ in 0..3 {
        rows.push(
            div()
                .flex()
                .gap(px(2.0))
                .child(div().size(px(1.5)).rounded_full().bg(theme::faint(0.55)))
                .child(div().size(px(1.5)).rounded_full().bg(theme::faint(0.55))),
        );
    }
    div().flex().flex_col().gap(px(2.0)).children(rows)
}

fn system_font_row(selected: bool, highlighted: bool, cx: &mut Context<Prompter>) -> AnyElement {
    font_row_element(
        "System".to_string(),
        selected,
        highlighted,
        cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
            this.choose_family(None, window, cx)
        }),
    )
    .into_any_element()
}

fn font_row_element(
    label: String,
    selected: bool,
    highlighted: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let marker: Hsla = if selected { theme::accent(1.0) } else { theme::faint(0.0) };
    let row_bg: Hsla = if highlighted { theme::faint(0.10) } else { theme::faint(0.0) };
    div()
        .id(SharedString::from(format!("font:{}", label)))
        .h(px(23.0))
        .w_full()
        .px(px(9.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .cursor(CursorStyle::PointingHand)
        .bg(row_bg)
        .hover(|this| this.bg(theme::control_hover(0.8)))
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
            cx.stop_propagation();
        })
        .on_click(on_click)
        .child(div().size(px(3.0)).rounded_full().bg(marker))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(if selected { theme::faint(1.0) } else { theme::faint(0.72) })
                .child(label),
        )
}

/// The five weights worth offering for a prompter, with their CSS values.
const WEIGHTS: [(f32, &str); 5] = [
    (300.0, "Light"),
    (400.0, "Regular"),
    (500.0, "Medium"),
    (600.0, "Semibold"),
    (700.0, "Bold"),
];

fn weight_chips(current: f32, cx: &mut Context<Prompter>) -> Vec<AnyElement> {
    WEIGHTS
        .iter()
        .map(|(weight, _)| {
            let active = (current - weight).abs() < 1.0;
            div()
                .id(SharedString::from(format!("weight:{weight}")))
                .h(px(21.0))
                .w(px(27.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .cursor(CursorStyle::PointingHand)
                .bg(theme::faint(if active { 0.15 } else { 0.02 }))
                .hover(|this| this.bg(theme::faint(0.12)))
                .text_size(px(11.0))
                .text_color(theme::faint(if active { 1.0 } else { 0.5 }))
                .font_weight(FontWeight(*weight))
                .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                    this.choose_weight(*weight, window, cx)
                }))
                .child("Aa")
                .into_any_element()
        })
        .collect()
}

fn installed_families(cx: &mut Context<Prompter>) -> Vec<SharedString> {
    let mut names: Vec<String> = cx.text_system().all_font_names();
    names.sort();
    names.dedup();
    let mut ordered: Vec<SharedString> = Vec::new();
    for wanted in FONT_SHORTLIST {
        if let Some(found) = names.iter().find(|n| n.eq_ignore_ascii_case(wanted)) {
            ordered.push(SharedString::from(found.clone()));
        }
    }
    for name in names {
        if !ordered.iter().any(|n| n.as_ref() == name.as_str()) {
            ordered.push(SharedString::from(name));
        }
    }
    ordered
}

fn resolve_family(settings: &Settings, families: &[SharedString]) -> Option<SharedString> {
    let find = |wanted: &str| {
        families
            .iter()
            .find(|f| f.as_ref().eq_ignore_ascii_case(wanted))
            .cloned()
    };
    if let Some(wanted) = settings.font_family.as_ref() {
        return find(wanted);
    }
    find(DEFAULT_FACE)
}

fn split_paragraphs(text: &str) -> Vec<SharedString> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let trimmed = block.trim();
        if !trimmed.is_empty() {
            let joined = trimmed.lines().map(str::trim).collect::<Vec<_>>().join(" ");
            out.push(SharedString::from(joined));
        }
    }
    out
}

fn load_script(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}
