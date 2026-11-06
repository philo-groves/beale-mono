use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rune::{Cursor, Drag, El, Icon, Live, Phase, ScrollPos, Spring, Tx, WheelEvent, col, el, icon};

use crate::{State, ThemeStyle};

const TRACK_WIDTH: f32 = 12.0;
const ARROW_HEIGHT: f32 = 20.0;
const ARROW_STEP: f32 = 48.0;
const THUMB_WIDTH: f32 = 6.0;
const MIN_THUMB_HEIGHT: f32 = 28.0;

static TRIANGLE_UP: Icon = Icon {
    name: "beale-scroll-up",
    view: 24.0,
    body: r#"<path d="M12 6 20 17H4Z" fill="currentColor"/>"#,
};

static TRIANGLE_DOWN: Icon = Icon {
    name: "beale-scroll-down",
    view: 24.0,
    body: r#"<path d="M4 7h16l-8 11Z" fill="currentColor"/>"#,
};

#[derive(Clone)]
pub(crate) struct SettingsScroll {
    scroll_key: &'static str,
    position: Rc<Cell<ScrollPos>>,
    track_height: Rc<Cell<f32>>,
    thumb_rect: Live<[f32; 4]>,
    opacity: Live<f32>,
    wheel_target: Rc<Cell<Option<(f32, Instant)>>>,
    drag_start_offset: Rc<Cell<f32>>,
}

impl Default for SettingsScroll {
    fn default() -> Self {
        Self::for_key("settings-scroll")
    }
}

impl SettingsScroll {
    pub(crate) fn for_key(scroll_key: &'static str) -> Self {
        Self {
            scroll_key,
            position: Rc::new(Cell::new(ScrollPos::default())),
            track_height: Rc::new(Cell::new(0.0)),
            thumb_rect: Live::new([3.0, 0.0, THUMB_WIDTH, MIN_THUMB_HEIGHT]),
            opacity: Live::new(0.0),
            wheel_target: Rc::new(Cell::new(None)),
            drag_start_offset: Rc::new(Cell::new(0.0)),
        }
    }
}

impl SettingsScroll {
    fn update_thumb(&self) {
        let position = self.position.get();
        let track_height = self.track_height.get();
        let max = position.max[1];
        if max <= 1.0 || track_height <= 0.0 {
            self.opacity.set(0.0);
            return;
        }
        let viewport_height = track_height + 2.0 * ARROW_HEIGHT;
        let thumb_height = (track_height * viewport_height / (viewport_height + max))
            .clamp(MIN_THUMB_HEIGHT.min(track_height), track_height);
        let travel = track_height - thumb_height;
        let thumb_y = (position.offset[1] / max).clamp(0.0, 1.0) * travel;
        self.thumb_rect.set([
            (TRACK_WIDTH - THUMB_WIDTH) / 2.0,
            thumb_y,
            THUMB_WIDTH,
            thumb_height,
        ]);
        self.opacity.set(1.0);
    }

    pub(crate) fn watch_scroll(&self, position: ScrollPos) {
        self.position.set(position);
        self.update_thumb();
    }

    pub(crate) fn wheel(&self, event: WheelEvent, tx: &mut Tx) -> bool {
        if event.precise || event.dy == 0.0 || event.dx.abs() > event.dy.abs() {
            self.wheel_target.set(None);
            return false;
        }
        let position = self.position.get();
        if position.max[1] <= 1.0 {
            return false;
        }
        let now = Instant::now();
        let base = self.wheel_target.get().and_then(|(target, at)| {
            (now.duration_since(at) < Duration::from_millis(280)).then_some(target)
        });
        let target = (base.unwrap_or(position.offset[1]) - event.dy).clamp(0.0, position.max[1]);
        self.wheel_target.set(Some((target, now)));
        tx.scroll_to_with(self.scroll_key, 0.0, target, Spring::new(0.18, 0.0));
        tx.live_only();
        true
    }

    pub(crate) fn bar(&self, theme: ThemeStyle) -> El {
        let layout_scroll = self.clone();
        let drag_scroll = self.clone();
        let up_scroll = self.clone();
        let down_scroll = self.clone();
        col()
            .key("settings-scrollbar")
            .role("scrollbar")
            .label("Content scroll position")
            .pin(None, Some(0.0), Some(0.0), Some(0.0))
            .w(TRACK_WIDTH)
            .fill(theme.color("#0d0d0d"))
            .cursor(Cursor::Default)
            .live_opacity(&self.opacity)
            .child(
                el().key("settings-scroll-up")
                    .w_full()
                    .h(ARROW_HEIGHT)
                    .center()
                    .role("button")
                    .label("Scroll up")
                    .on_press_tx(move |_: &mut State, tx| {
                        let position = up_scroll.position.get();
                        tx.scroll_to_y(up_scroll.scroll_key, position.offset[1] - ARROW_STEP);
                        tx.live_only();
                    })
                    .child(icon(&TRIANGLE_UP, 10.0).color(theme.color("#555555"))),
            )
            .child(
                el().key("settings-scrollbar-track")
                    .w_full()
                    .grow(1.0)
                    .on_layout_tx(move |_: &mut State, [_, _, _, height], tx| {
                        layout_scroll.track_height.set(height);
                        layout_scroll.update_thumb();
                        tx.live_only();
                    })
                    .on_drag(move |_: &mut State, drag: &mut Drag, tx| {
                        let position = drag_scroll.position.get();
                        let [_, thumb_y, _, thumb_height] = drag_scroll.thumb_rect.get();
                        let travel = (drag_scroll.track_height.get() - thumb_height).max(0.0);
                        if position.max[1] <= 1.0 || travel <= 0.0 {
                            return;
                        }
                        if drag.phase == Phase::Down {
                            drag_scroll.drag_start_offset.set(position.offset[1]);
                            drag.data = if (thumb_y..=thumb_y + thumb_height).contains(&drag.y) {
                                drag.y - thumb_y
                            } else {
                                thumb_height / 2.0
                            };
                            drag_scroll.wheel_target.set(None);
                        }
                        if drag.phase == Phase::Up && drag.cancelled() {
                            tx.scroll_to_y(
                                drag_scroll.scroll_key,
                                drag_scroll.drag_start_offset.get(),
                            );
                            tx.live_only();
                            return;
                        }
                        let target =
                            ((drag.y - drag.data) / travel).clamp(0.0, 1.0) * position.max[1];
                        tx.scroll_to_y(drag_scroll.scroll_key, target);
                        tx.instant();
                        tx.live_only();
                    })
                    .child(
                        el().key("settings-scrollbar-thumb")
                            .live_rect(&self.thumb_rect)
                            .radius(THUMB_WIDTH / 2.0)
                            .fill(theme.color("#111111")),
                    ),
            )
            .child(
                el().key("settings-scroll-down")
                    .w_full()
                    .h(ARROW_HEIGHT)
                    .center()
                    .role("button")
                    .label("Scroll down")
                    .on_press_tx(move |_: &mut State, tx| {
                        let position = down_scroll.position.get();
                        tx.scroll_to_y(down_scroll.scroll_key, position.offset[1] + ARROW_STEP);
                        tx.live_only();
                    })
                    .child(icon(&TRIANGLE_DOWN, 10.0).color(theme.color("#555555"))),
            )
    }

    #[cfg(test)]
    pub(crate) fn position(&self) -> ScrollPos {
        self.position.get()
    }

    #[cfg(test)]
    pub(crate) fn metrics(&self) -> (f32, f32, f32) {
        (
            self.track_height.get(),
            self.opacity.get(),
            self.thumb_rect.get()[3],
        )
    }
}
