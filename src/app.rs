//! The prompter panel: state, the control strip, and the script.

use crate::mac;
use crate::markdown::{self, StyledSpan};
use crate::settings::{MAX_FONT, MAX_OPACITY, MIN_FONT, MIN_OPACITY, Settings};
use crate::theme;
#[cfg(not(target_os = "macos"))]
use gpui::{FontStyle, HighlightStyle, StyledText};
#[cfg(target_os = "macos")]
use crate::smooth_text::{SmoothParagraph, SmoothTextRenderer};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, ExternalPaths, FocusHandle, FontWeight, Hsla,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, Pixels, Render, ScrollHandle, ScrollStrategy, SharedString, Subscription,
    Task, UniformListScrollHandle, Window, canvas, div, point, px, relative, uniform_list,
};
#[cfg(target_os = "macos")]
use std::cell::RefCell;
use std::cell::Cell;
use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// Width of the opacity track, in points.
const TRACK: f32 = 68.0;
/// Height of the opacity control's hit area.
const TRACK_H: f32 = 22.0;
/// Height of each row in the font picker list.
const FONT_ROW_H: f32 = 23.0;
/// Default font size used by Command+0 reset.
const DEFAULT_FONT_SIZE: f32 = 34.0;

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

const SAMPLE: &str = "Paste a script with Command V, open a file with Command O, or drop a text file here.\n\nDrag the strip at the top to move this window, and pull any corner to resize it. The slider fades the backdrop behind *italic* and **bold** words.";

#[derive(Clone)]
struct FontEntry {
    family: Option<SharedString>,
    label: SharedString,
    lower: String,
}

#[derive(Clone)]
struct ScriptParagraph {
    text: SharedString,
    spans: Vec<StyledSpan>,
}

pub struct Prompter {
    settings: Settings,
    family: Option<SharedString>,
    fonts: Vec<FontEntry>,
    filtered_fonts: Vec<usize>,
    paragraphs: Vec<ScriptParagraph>,
    scroll: ScrollHandle,
    font_scroll: UniformListScrollHandle,
    font_menu_open: bool,
    hovered: bool,
    dragging_opacity: bool,
    opacity_bounds: Rc<Cell<Bounds<Pixels>>>,
    notice: Option<SharedString>,
    notice_task: Option<Task<()>>,
    font_query: String,
    font_cursor: usize,
    focus: FocusHandle,
    save_task: Option<Task<()>>,
    _bounds_observer: Option<Subscription>,
    #[cfg(target_os = "macos")]
    smooth_text: Rc<RefCell<SmoothTextRenderer>>,
}

impl Prompter {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        mut settings: Settings,
        script_arg: Option<String>,
    ) -> Self {
        let fonts = build_font_entries(cx);
        let family = resolve_family(&settings, &fonts);
        let filtered_fonts: Vec<usize> = (0..fonts.len()).collect();

        let mut raw_script: Option<String> = None;
        if let Some(path) = script_arg.or_else(|| settings.script_path.clone()) {
            if let Some(text) = load_script(&path) {
                raw_script = Some(text);
                settings.script_path = Some(path);
            }
        }
        let paragraphs = compile_paragraphs(raw_script.as_deref().unwrap_or(SAMPLE));

        let mut this = Self {
            settings,
            family,
            fonts,
            filtered_fonts,
            paragraphs,
            scroll: ScrollHandle::new(),
            font_scroll: UniformListScrollHandle::new(),
            font_menu_open: false,
            hovered: false,
            dragging_opacity: false,
            opacity_bounds: Rc::new(Cell::new(Bounds::default())),
            notice: None,
            notice_task: None,
            font_query: String::new(),
            font_cursor: 0,
            focus: cx.focus_handle(),
            save_task: None,
            _bounds_observer: None,
            #[cfg(target_os = "macos")]
            smooth_text: Rc::new(RefCell::new(SmoothTextRenderer::new(theme::text()))),
        };
        if std::env::var_os("PROMPTER_DEBUG_WINDOW").is_some() {
            eprintln!(
                "fonts: {} families installed, using {:?}",
                this.fonts.len().saturating_sub(1),
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
            if mac::is_miniaturized(window) {
                return;
            }
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
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            let json = this
                .update(cx, |this, _| this.settings.to_json())
                .ok()
                .flatten();
            if let Some(json) = json {
                cx.background_executor()
                    .spawn(async move { Settings::write_serialized(&json) })
                    .await;
            }
        }));
    }

    fn bump_font(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.font_size = (self.settings.font_size + delta).clamp(MIN_FONT, MAX_FONT);
        self.save_debounced(window, cx);
        cx.notify();
    }

    fn reset_font_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.font_size = DEFAULT_FONT_SIZE;
        self.flash(SharedString::from("34 pt"), window, cx);
        self.save_debounced(window, cx);
    }

    fn set_opacity_at(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = self.opacity_bounds.get();
        let inset = (f32::from(bounds.size.width) - TRACK) / 2.0;
        let span = f32::from(x) - f32::from(bounds.origin.x) - inset;
        let fraction = (span / TRACK).clamp(0.0, 1.0);
        self.settings.opacity = MIN_OPACITY + fraction * (MAX_OPACITY - MIN_OPACITY);
        let pct = (self.settings.opacity * 100.0).round() as i32;
        self.notice = Some(SharedString::from(format!("{pct}%")));
        self.notice_task = None;
        self.save_debounced(window, cx);
        cx.notify();
    }

    fn finish_opacity_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dragging_opacity {
            self.dragging_opacity = false;
            let pct = (self.settings.opacity * 100.0).round() as i32;
            self.flash_for(
                SharedString::from(format!("{pct}%")),
                Duration::from_millis(800),
                window,
                cx,
            );
            self.save_debounced(window, cx);
        }
    }

    fn refilter_fonts(&mut self) {
        let query = self.font_query.trim().to_lowercase();
        self.filtered_fonts.clear();
        for (idx, entry) in self.fonts.iter().enumerate() {
            if query.is_empty() || entry.lower.contains(&query) {
                self.filtered_fonts.push(idx);
            }
        }
        if self.filtered_fonts.is_empty() {
            self.font_cursor = 0;
        } else {
            self.font_cursor = self.font_cursor.min(self.filtered_fonts.len() - 1);
        }
    }

    fn open_font_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        mac::make_key(window);
        self.focus.focus(window, cx);
        self.font_menu_open = true;
        self.font_query.clear();
        self.refilter_fonts();
        let active_idx = self
            .filtered_fonts
            .iter()
            .position(|&idx| self.fonts[idx].family == self.family)
            .unwrap_or(0);
        self.font_cursor = active_idx;
        self.font_scroll
            .scroll_to_item(active_idx, ScrollStrategy::Center);
        cx.notify();
    }

    fn close_font_menu(&mut self) {
        self.font_menu_open = false;
        self.font_query.clear();
        self.refilter_fonts();
        self.font_cursor = 0;
    }

    fn choose_family(
        &mut self,
        family: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.font_family = Some(
            family
                .as_ref()
                .map(|f| f.to_string())
                .unwrap_or_else(|| "System".to_string()),
        );
        self.family = family.clone();
        self.close_font_menu();
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
        let Some(&entry_idx) = self.filtered_fonts.get(self.font_cursor) else {
            return;
        };
        let family = self.fonts[entry_idx].family.clone();
        self.choose_family(family, window, cx);
    }

    fn scroll_by(&mut self, lines: f32, cx: &mut Context<Self>) {
        let offset = self.scroll.offset();
        let cur_y = f32::from(offset.y);
        let limit = f32::from(self.scroll.max_offset().y).max(0.0);
        let step = self.settings.font_size * 1.46 * lines;
        let next = (cur_y - step).clamp(-limit, 0.0);
        self.scroll.set_offset(point(offset.x, px(next)));
        cx.notify();
    }

    fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// Show a short status in the strip, then clear it.
    fn flash(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.flash_for(text, Duration::from_millis(1500), window, cx);
    }

    fn flash_for(
        &mut self,
        text: SharedString,
        duration: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some(text);
        cx.notify();
        self.notice_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                this.notice_task = None;
                cx.notify();
            });
        }));
    }

    fn load_new_script(&mut self, text: &str) {
        self.paragraphs = compile_paragraphs(text);
        self.scroll.set_offset(point(px(0.0), px(0.0)));
    }

    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { std::fs::read_to_string(&path).ok() }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(text) = text {
                    this.load_new_script(&text);
                    this.settings.script_path = Some(path.to_string_lossy().to_string());
                    let label = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| SharedString::from(n.to_string()))
                        .unwrap_or_else(|| SharedString::from("Opened"));
                    this.flash(label, window, cx);
                    this.save_debounced(window, cx);
                } else {
                    this.flash(SharedString::from("Could not read file"), window, cx);
                }
            });
        })
        .detach();
    }

    fn paste_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(text) = item.text() {
                if !text.trim().is_empty() {
                    self.load_new_script(&text);
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
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_path(path, window, cx);
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
                "m" => window.minimize_window(),
                "v" => self.paste_script(window, cx),
                "o" => self.open_script(window, cx),
                "=" | "+" => self.bump_font(1.0, window, cx),
                "-" => self.bump_font(-1.0, window, cx),
                "0" => self.reset_font_size(window, cx),
                "backspace" if self.font_menu_open => {
                    if !self.font_query.is_empty() {
                        self.font_query.clear();
                        self.font_cursor = 0;
                        self.refilter_fonts();
                        self.font_scroll
                            .scroll_to_item(0, ScrollStrategy::Nearest);
                        cx.notify();
                    }
                }
                _ => {}
            }
            return;
        }
        if self.font_menu_open {
            match key {
                "escape" => {
                    self.close_font_menu();
                    cx.notify();
                }
                "backspace" => {
                    let changed = if keystroke.modifiers.alt {
                        pop_last_word(&mut self.font_query)
                    } else {
                        self.font_query.pop().is_some()
                    };
                    if changed {
                        self.font_cursor = 0;
                        self.refilter_fonts();
                        self.font_scroll
                            .scroll_to_item(0, ScrollStrategy::Nearest);
                        cx.notify();
                    }
                }
                "enter" => self.pick_filtered(window, cx),
                "down" => {
                    if !self.filtered_fonts.is_empty() {
                        self.font_cursor =
                            (self.font_cursor + 1).min(self.filtered_fonts.len() - 1);
                        self.font_scroll
                            .scroll_to_item(self.font_cursor, ScrollStrategy::Nearest);
                        cx.notify();
                    }
                }
                "up" => {
                    self.font_cursor = self.font_cursor.saturating_sub(1);
                    self.font_scroll
                        .scroll_to_item(self.font_cursor, ScrollStrategy::Nearest);
                    cx.notify();
                }
                _ => {
                    if let Some(text) = keystroke.key_char.as_ref() {
                        if !text.is_empty()
                            && !text.chars().any(char::is_control)
                            && !keystroke.modifiers.control
                            && !keystroke.modifiers.alt
                        {
                            self.font_query.push_str(text);
                            self.font_cursor = 0;
                            self.refilter_fonts();
                            self.font_scroll
                                .scroll_to_item(0, ScrollStrategy::Nearest);
                            cx.notify();
                        }
                    }
                }
            }
            return;
        }
        match key {
            "up" => self.scroll_by(-1.0, cx),
            "down" => self.scroll_by(1.0, cx),
            "pageup" => self.scroll_by(-5.0, cx),
            "pagedown" => self.scroll_by(5.0, cx),
            "home" => self.scroll_to_top(cx),
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
            .pl(px(6.0))
            .pr(px(7.0))
            .gap(px(2.0))
            .border_b_1()
            .border_color(theme::faint(0.07))
            .child(
                div()
                    .id("grip")
                    .h_full()
                    .w(px(16.0))
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
                    .min_w(px(8.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .cursor(CursorStyle::OpenHand)
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, window, _| {
                        window.start_window_move()
                    })
                    .children(self.notice.as_ref().map(|text| {
                        div()
                            .max_w_full()
                            .px(px(4.0))
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(10.5))
                            .text_color(theme::faint(0.72))
                            .font_weight(FontWeight::MEDIUM)
                            .child(text.clone())
                    })),
            )
            .child(
                div()
                    .id("opacity")
                    .relative()
                    .w(px(TRACK + 12.0))
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
                                    .bg(if self.dragging_opacity {
                                        theme::faint(0.85)
                                    } else {
                                        theme::faint(0.62)
                                    }),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-3.5))
                                    .left(px(knob_x - 5.0))
                                    .size(px(10.0))
                                    .rounded_full()
                                    .bg(if self.dragging_opacity {
                                        theme::accent(1.0)
                                    } else {
                                        theme::text()
                                    })
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
                    .px(px(4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    .text_size(px(12.0))
                    .text_color(if self.font_menu_open {
                        theme::accent(1.0)
                    } else {
                        theme::faint(0.80)
                    })
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|this| this.bg(theme::control_hover(1.0)).text_color(theme::faint(1.0)))
                    .opacity(if self.font_menu_open { 1.0 } else { chrome })
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                        if this.font_menu_open {
                            this.close_font_menu();
                            cx.notify();
                        } else {
                            this.open_font_menu(window, cx);
                        }
                    }))
                    .child("Aa"),
            )
            .child(divider(chrome))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .child(
                        div()
                            .id("minimize")
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .cursor(CursorStyle::PointingHand)
                            .hover(|this| this.bg(theme::control_hover(1.0)))
                            .opacity(chrome)
                            .on_mouse_down(
                                MouseButton::Left,
                                |_: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                },
                            )
                            .on_click(|_: &gpui::ClickEvent, window, _cx| {
                                window.minimize_window();
                            })
                            .child(minimize_glyph()),
                    )
                    .child(
                        div()
                            .id("close")
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .cursor(CursorStyle::PointingHand)
                            .text_size(px(13.0))
                            .text_color(theme::faint(0.80))
                            .hover(|this| {
                                this.bg(theme::control_hover(1.0))
                                    .text_color(theme::faint(1.0))
                            })
                            .opacity(chrome)
                            .on_mouse_down(
                                MouseButton::Left,
                                |_: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                },
                            )
                            .on_click(|_: &gpui::ClickEvent, _window, cx| cx.quit())
                            .child("×"),
                    ),
            )
    }

    fn font_menu(&self, available: f32, cx: &mut Context<Self>) -> AnyElement {
        let match_count = self.filtered_fonts.len();
        let menu_max_h = available.max(96.0);
        // Search box (31) + weight chips (28) + divider (13) + list bottom pad (6) = 78pt.
        let max_list_h = (menu_max_h - 78.0).max(FONT_ROW_H);
        let content_h = (match_count.max(1) as f32) * FONT_ROW_H + 6.0;
        let list_height = content_h.min(max_list_h);

        let empty = self.font_query.is_empty();
        let weight_name = WEIGHTS
            .iter()
            .find(|(value, _)| (self.settings.font_weight - value).abs() < 1.0)
            .map(|(_, name)| *name)
            .unwrap_or("Variable");

        let list_element: AnyElement = if match_count == 0 {
            div()
                .id("font-empty")
                .h(px(FONT_ROW_H))
                .px(px(9.0))
                .flex()
                .items_center()
                .text_size(px(11.5))
                .text_color(theme::faint(0.34))
                .child("Nothing matches")
                .into_any_element()
        } else {
            uniform_list(
                "font-list",
                match_count,
                cx.processor(|this, range: Range<usize>, _window, cx| {
                    let active = &this.family;
                    range
                        .map(|filtered_ix| {
                            let entry_ix = this.filtered_fonts[filtered_ix];
                            let entry = &this.fonts[entry_ix];
                            let selected = &entry.family == active;
                            let highlighted = filtered_ix == this.font_cursor;
                            let picked = entry.family.clone();
                            font_row_element(
                                filtered_ix,
                                entry.label.clone(),
                                selected,
                                highlighted,
                                cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                                    this.choose_family(picked.clone(), window, cx)
                                }),
                            )
                        })
                        .collect()
                }),
            )
            .h(px(list_height))
            .w_full()
            .pb(px(6.0))
            .track_scroll(&self.font_scroll)
            .into_any_element()
        };

        div()
            .id("font-menu")
            .absolute()
            .top(px(theme::STRIP - 2.0))
            .right(px(8.0))
            .w(px(252.0))
            .max_h(px(menu_max_h))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .border_1()
            .border_color(theme::faint(0.14))
            .bg(gpui::hsla(220.0 / 360.0, 0.16, 0.085, 0.985))
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    mac::make_key(window);
                    this.focus.focus(window, cx);
                }),
            )
            .child(
                div()
                    .id("font-search")
                    .mx(px(7.0))
                    .mt(px(7.0))
                    .h(px(24.0))
                    .px(px(7.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(theme::faint(0.07))
                    .overflow_hidden()
                    .child(if empty {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .w(px(1.5))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(theme::accent(0.85)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(theme::faint(0.36))
                                    .child("Search fonts"),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(
                                div()
                                    .max_w_full()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(12.0))
                                    .text_color(theme::faint(0.92))
                                    .child(SharedString::from(self.font_query.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(1.5))
                                    .h(px(12.0))
                                    .ml(px(1.0))
                                    .rounded_full()
                                    .bg(theme::accent(0.90)),
                            )
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .mx(px(7.0))
                    .mt(px(7.0))
                    .children(weight_chips(
                        self.settings.font_weight,
                        self.family.as_ref(),
                        cx,
                    ))
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
            .child(list_element)
            .into_any_element()
    }

    fn script_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size = self.settings.font_size;
        let base_weight = self.settings.font_weight;
        let bold_weight = FontWeight((base_weight + 300.0).clamp(600.0, 900.0));
        let family = self.family.clone();
        let gap = (size * 0.62).max(10.0);
        let bottom_pad = (size * 1.4).max(24.0);

        #[cfg(target_os = "macos")]
        let children: Vec<AnyElement> = {
            let bold_w = bold_weight.0;
            let smooth_text = self.smooth_text.clone();
            self.paragraphs
                .iter()
                .enumerate()
                .map(|(ix, p)| {
                    SmoothParagraph::new(
                        SharedString::from(format!("para:{ix}")),
                        p.text.clone(),
                        p.spans.clone(),
                        family.clone(),
                        size,
                        base_weight,
                        bold_w,
                        1.46,
                        theme::text(),
                        smooth_text.clone(),
                    )
                    .into_any_element()
                })
                .collect()
        };

        #[cfg(not(target_os = "macos"))]
        let children: Vec<AnyElement> = self
            .paragraphs
            .iter()
            .map(|p| {
                if p.spans.is_empty() {
                    p.text.clone().into_any_element()
                } else {
                    StyledText::new(p.text.clone())
                        .with_highlights(p.spans.iter().map(|span| {
                            (
                                span.range.clone(),
                                HighlightStyle {
                                    font_weight: span.style.bold.then_some(bold_weight),
                                    font_style: span.style.italic.then_some(FontStyle::Italic),
                                    ..Default::default()
                                },
                            )
                        }))
                        .into_any_element()
                }
            })
            .collect();

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
                    .pb(px(bottom_pad))
                    .when_some(family, |this, family| this.font_family(family))
                    .text_size(px(size))
                    .line_height(relative(1.46))
                    .font_weight(FontWeight(base_weight))
                    .text_color(theme::text())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            mac::make_key(window);
                            this.focus.focus(window, cx);
                        }),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(gap))
                            .children(children),
                    ),
            )
    }
}

impl Render for Prompter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = if self.hovered || self.dragging_opacity || self.font_menu_open {
            1.0
        } else {
            0.52
        };
        let alpha = self.settings.opacity;
        let menu_height = f32::from(window.bounds().size.height) - theme::STRIP - 12.0;

        let menu = if self.font_menu_open {
            Some(self.font_menu(menu_height, cx))
        } else {
            None
        };
        let body = self.script_body(cx);
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
            .on_drop::<ExternalPaths>(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.open_path(path.clone(), window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.dragging_opacity {
                    if event.dragging() {
                        this.set_opacity_at(event.position.x, window, cx);
                    } else {
                        this.finish_opacity_drag(window, cx);
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    this.finish_opacity_drag(window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, window, cx| {
                    this.finish_opacity_drag(window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                    if this.font_menu_open {
                        this.close_font_menu();
                        cx.notify();
                    }
                }),
            )
            .child(header)
            .child(body)
            .children(menu)
    }
}

fn pop_last_word(s: &mut String) -> bool {
    let before = s.len();
    while s.ends_with(char::is_whitespace) {
        s.pop();
    }
    while s.chars().last().is_some_and(|c| !c.is_whitespace()) {
        s.pop();
    }
    s.len() != before
}

fn minimize_glyph() -> AnyElement {
    div()
        .pt(px(5.0))
        .child(
            div()
                .w(px(8.0))
                .h(px(1.5))
                .rounded(px(0.5))
                .bg(theme::faint(0.80)),
        )
        .into_any_element()
}

fn glyph_button(
    id: &'static str,
    label: &'static str,
    chrome: f32,
    size: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .text_size(px(size))
        .text_color(theme::faint(0.80))
        .hover(|this| this.bg(theme::control_hover(1.0)).text_color(theme::faint(1.0)))
        .opacity(chrome)
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _window, cx| {
            cx.stop_propagation();
        })
        .child(label)
}

fn divider(chrome: f32) -> impl IntoElement {
    div()
        .w(px(1.0))
        .h(px(13.0))
        .mx(px(2.5))
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
                    this.bump_font(-1.0, window, cx)
                })),
        )
        .child(
            div()
                .w(px(22.0))
                .flex()
                .justify_center()
                .text_size(px(11.0))
                .text_color(theme::faint(0.62))
                .child(format!("{}", size.round() as i32)),
        )
        .child(
            glyph_button("font-larger", "+", chrome, 14.0)
                .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.bump_font(1.0, window, cx)
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

fn font_row_element(
    index: usize,
    label: SharedString,
    selected: bool,
    highlighted: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let marker: Hsla = if selected {
        theme::accent(1.0)
    } else {
        theme::faint(0.0)
    };
    let row_bg: Hsla = if highlighted {
        theme::faint(0.10)
    } else {
        theme::faint(0.0)
    };
    div()
        .id(("font-row", index))
        .h(px(FONT_ROW_H))
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
                .flex_1()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .text_color(if selected {
                    theme::faint(1.0)
                } else {
                    theme::faint(0.72)
                })
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

fn weight_chips(
    current: f32,
    family: Option<&SharedString>,
    cx: &mut Context<Prompter>,
) -> Vec<AnyElement> {
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
                .when_some(family.cloned(), |this, family| this.font_family(family))
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

fn build_font_entries(cx: &mut Context<Prompter>) -> Vec<FontEntry> {
    let mut names: Vec<String> = cx.text_system().all_font_names();
    names.sort();
    names.dedup();

    let mut seen: HashSet<String> = HashSet::with_capacity(names.len());
    let mut entries: Vec<FontEntry> = Vec::with_capacity(names.len() + 1);
    entries.push(FontEntry {
        family: None,
        label: SharedString::from("System"),
        lower: "system".to_string(),
    });

    for wanted in FONT_SHORTLIST {
        if let Some(found) = names.iter().find(|n| n.eq_ignore_ascii_case(wanted)) {
            if seen.insert(found.clone()) {
                entries.push(FontEntry {
                    family: Some(SharedString::from(found.clone())),
                    label: SharedString::from(found.clone()),
                    lower: found.to_lowercase(),
                });
            }
        }
    }
    for name in names {
        if seen.insert(name.clone()) {
            let lower = name.to_lowercase();
            let label = SharedString::from(name);
            entries.push(FontEntry {
                family: Some(label.clone()),
                label,
                lower,
            });
        }
    }
    entries
}

fn resolve_family(settings: &Settings, fonts: &[FontEntry]) -> Option<SharedString> {
    let find = |wanted: &str| {
        fonts
            .iter()
            .skip(1)
            .find(|f| f.label.as_ref().eq_ignore_ascii_case(wanted))
            .and_then(|f| f.family.clone())
    };
    if let Some(wanted) = settings.font_family.as_ref() {
        if wanted.eq_ignore_ascii_case("System") {
            return None;
        }
        return find(wanted);
    }
    find(DEFAULT_FACE)
}

fn compile_paragraphs(text: &str) -> Vec<ScriptParagraph> {
    markdown::parse_paragraphs(text)
        .into_iter()
        .map(|p| ScriptParagraph {
            text: SharedString::from(p.text),
            spans: p.spans,
        })
        .collect()
}

fn load_script(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}
