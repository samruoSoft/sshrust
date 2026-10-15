//! 通用滚动容器组件（真实滚动 + 虚拟滚动）。
//!
//! 两种模式：
//! - 真实滚动：子内容渲染在可滚动容器里（overflow_y/x_scroll），
//!   滚轮由 gpui 驱动 `ScrollHandle`，thumb 拖拽由组件回写句柄。
//! - 虚拟滚动：调用方自己管理视口（终端 grid、虚拟表格等），
//!   组件按受控的 (offset, max, page, track_px) 只画滚动条；
//!   拖拽通过 `on_goto`（0=内容顶部语义）、滚轮通过 `on_scroll_delta`
//!   （可选）交给调用方。
//!
//! 可见性：非交互状态完全不渲染滚动条；容器 hover 时显示 8px；
//! 滚动条区域 hover 或拖拽中加粗到 10px。
//!
//! 轴：None / X / Y / Auto（Auto=任一轴溢出才出现对应滚动条）。
//! 虚拟滚动以 Y 轴为主，可用 `.virtual_x(...)` 附加 X 轴（TextArea 即此用法）。
//!
//! 业务事件（右键菜单等）请挂在传入的 child 上，不要与本组件的容器叠加。
//! 主题经 `.theme(Theme)` 注入（默认 Theme::DARK）。

use gpui::{
    prelude::*, AnyElement, App, CursorStyle, DragMoveEvent, Entity, MouseButton, Point,
    RenderOnce, ScrollHandle, ScrollWheelEvent, Styled, Window, div, px, rgba,
};
use std::rc::Rc;

use crate::base::DragGhost;
use crate::Theme;

/// 滚动轴配置。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollAxis {
    /// 不滚动、不显示滚动条。
    None,
    /// 仅横向。
    X,
    /// 仅纵向。
    Y,
    /// 两轴都可滚动，各自溢出时才出现对应滚动条。
    Auto,
}

/// 全局 owner 发号器：组件内嵌滚动实例（如 TextArea）取唯一 id，
/// 避免与其他滚动条的拖拽标记互相触发。
static NEXT_OWNER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1000);

pub fn next_owner() -> usize {
    NEXT_OWNER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 滚动条拖拽标记。gpui 的 `on_drag_move` 按标记类型全局分发，
/// 多个滚动条实例共用本类型时，回调里必须比对 owner 过滤。
#[derive(Clone, Copy)]
pub struct ScrollDragMarker(pub usize);

/// 每个滚动容器持有一个状态实体：hover 视觉状态、拖拽锚点、
/// 真实模式的滚动句柄（虚拟模式下闲置）。
pub struct ScrollAreaState {
    owner: usize,
    hovered: bool,
    bar_hover_y: bool,
    bar_hover_x: bool,
    handle: ScrollHandle,
    /// (按下时鼠标 y, 按下时偏移)，偏移为「0=内容顶部」正数域。
    drag_y: Option<(f32, f32)>,
    drag_x: Option<(f32, f32)>,
}

impl ScrollAreaState {
    pub fn new(owner: usize) -> Self {
        Self {
            owner,
            hovered: false,
            bar_hover_y: false,
            bar_hover_x: false,
            handle: ScrollHandle::new(),
            drag_y: None,
            drag_x: None,
        }
    }

    pub fn owner(&self) -> usize {
        self.owner
    }

    /// 清除拖拽锚点（鼠标释放时调用）。返回是否有变化。
    pub fn end_drag(&mut self) -> bool {
        self.drag_y.take().is_some() | self.drag_x.take().is_some()
    }
}

/// thumb 几何（纯函数，便于测试）。全部采用「0=内容顶部」正数域。
/// 返回 (thumb_h, travel, top)；None 表示该轴不需要滚动条。
pub fn thumb_metrics(track: f32, page: f32, max: f32, off: f32) -> Option<(f32, f32, f32)> {
    // track < 24 时 clamp(24., track) 会因 min>max panic，过小不显示。
    if max <= 0.5 || track < 24. || page < 1. {
        return None;
    }
    let thumb_h = (track * page / (page + max)).clamp(24., track);
    let travel = track - thumb_h;
    let top = (off / max * travel).clamp(0., travel);
    Some((thumb_h, travel, top))
}

struct VirtualY {
    offset: f32,
    max: f32,
    page: f32,
    track_h: f32,
    // Rc 共享：on_drag_move 闭包与组件本体每帧各持一份引用。
    on_goto: Rc<dyn Fn(f32, &mut Window, &mut App)>,
    on_delta: Option<Rc<dyn Fn(f32, &mut Window, &mut App)>>,
}

struct VirtualX {
    offset: f32,
    max: f32,
    track_w: f32,
    on_goto: Rc<dyn Fn(f32, &mut Window, &mut App)>,
}

/// 滚动容器。`ScrollArea::new` 为真实滚动；`ScrollArea::virtual_y` 为虚拟滚动。
#[derive(IntoElement)]
pub struct ScrollArea {
    state: Entity<ScrollAreaState>,
    axis: ScrollAxis,
    child: AnyElement,
    virtual_y: Option<VirtualY>,
    virtual_x: Option<VirtualX>,
    /// 滚动条与容器右缘的让位距离（如侧栏右缘的宽度拖柄）。
    right_inset: f32,
    theme: Theme,
}

impl ScrollArea {
    /// 真实滚动：内容真实渲染并随滚动偏移移动。
    pub fn new(state: Entity<ScrollAreaState>, child: impl IntoElement) -> Self {
        Self {
            state,
            axis: ScrollAxis::Y,
            child: child.into_any_element(),
            virtual_y: None,
            virtual_x: None,
            right_inset: 0.,
            theme: Theme::DARK,
        }
    }

    /// 虚拟滚动（Y 轴）：调用方自管视口，组件只画滚动条并回调交互。
    /// `offset/max/page` 同单位即可（行数或像素），0=内容顶部；
    /// `track_h` 为滚动条轨道像素高度；`on_goto(top 域偏移)` 为拖拽目标。
    pub fn virtual_y(
        state: Entity<ScrollAreaState>,
        child: impl IntoElement,
        offset: f32,
        max: f32,
        page: f32,
        track_h: f32,
        on_goto: impl Fn(f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            state,
            axis: ScrollAxis::Y,
            child: child.into_any_element(),
            virtual_y: Some(VirtualY {
                offset,
                max,
                page,
                track_h,
                on_goto: Rc::new(on_goto),
                on_delta: None,
            }),
            virtual_x: None,
            right_inset: 0.,
            theme: Theme::DARK,
        }
    }

    pub fn axis(mut self, axis: ScrollAxis) -> Self {
        self.axis = axis;
        self
    }

    pub fn bar_right_inset(mut self, inset: f32) -> Self {
        self.right_inset = inset;
        self
    }

    /// 注入配色（滚动条 idle/active 取 text/accent 半透明合成）。
    pub fn theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// 虚拟模式可选：附加 X 轴虚拟滚动（横向滚动条）。
    /// `offset/max` 同单位（像素），0=内容最左；`track_w` 为轨道像素宽；
    /// `on_goto(left 域偏移)` 为拖拽目标。仅在虚拟模式下生效，真实模式忽略。
    pub fn virtual_x(
        mut self,
        offset: f32,
        max: f32,
        track_w: f32,
        on_goto: impl Fn(f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        if self.virtual_y.is_some() {
            self.virtual_x = Some(VirtualX { offset, max, track_w, on_goto: Rc::new(on_goto) });
        }
        self
    }

    /// 虚拟模式可选：接管滚轮（dy 像素，gpui 约定正值=向内容顶部）。
    /// 真实模式下忽略；不设置则组件不监听滚轮（如终端自接滚轮的场景）。
    pub fn on_scroll_delta(
        mut self,
        cb: impl Fn(f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        if let Some(v) = &mut self.virtual_y {
            v.on_delta = Some(Rc::new(cb));
        }
        self
    }
}

fn thumb_colors(theme: Theme) -> (u32, u32) {
    // 半透明合成：0xRRGGBB → 0xRRGGBBAA。
    let idle = (theme.text << 8) | 0x66;
    let active = (theme.accent << 8) | 0xd0;
    (idle, active)
}

impl RenderOnce for ScrollArea {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let st = self.state.read(cx);
        let owner = st.owner;
        let axis = self.axis;
        let is_virtual = self.virtual_y.is_some();

        // —— Y 轴几何（0=内容顶部正数域）——
        // 真实模式 gpui 的 offset 为负数域，取反归一。
        let (y_off, y_max, y_page, y_track_h): (f32, f32, f32, f32) =
            if let Some(v) = &self.virtual_y {
                (v.offset, v.max, v.page, v.track_h)
            } else {
                let h = &st.handle;
                let off: f32 = h.offset().y.into();
                let max: f32 = h.max_offset().height.into();
                let vp: f32 = h.bounds().size.height.into();
                (-off, max, vp, vp)
            };
        let y_enabled = !is_virtual && matches!(axis, ScrollAxis::Y | ScrollAxis::Auto);

        // —— X 轴几何（真实模式 / 虚拟 X 附加）——
        let (x_off, x_max, x_vp): (f32, f32, f32) = if let Some(vx) = &self.virtual_x {
            (vx.offset, vx.max, vx.track_w)
        } else {
            (
                st.handle.offset().x.into(),
                st.handle.max_offset().width.into(),
                st.handle.bounds().size.width.into(),
            )
        };
        let x_enabled =
            (!is_virtual && matches!(axis, ScrollAxis::X | ScrollAxis::Auto)) || self.virtual_x.is_some();

        let active_y = st.drag_y.is_some() || st.bar_hover_y;
        let active_x = st.drag_x.is_some() || st.bar_hover_x;
        let visible = st.hovered || active_y || active_x;

        let y_m = if visible && (y_enabled || is_virtual) {
            thumb_metrics(y_track_h, y_page, y_max, y_off)
        } else {
            None
        };
        let x_m = if visible && x_enabled {
            thumb_metrics(x_vp, x_vp, x_max, x_off)
        } else {
            None
        };
        // 虚拟 X 的拖拽回调（真实模式为 None，走句柄回写）。
        let x_on_goto = self.virtual_x.as_ref().map(|vx| vx.on_goto.clone());
        let x_cur_off = if self.virtual_x.is_some() { x_off } else {
            let o: f32 = self.state.read(cx).handle.offset().x.into();
            o
        };

        // —— 内容层 ——
        // overflow_x/y_scroll 在 gpui 0.2.2 中写同一个 style 字段，两轴同开
        // 会互相覆盖，Auto 时改用 overflow_scroll。
        let mut content = div()
            .id(("scroll-content", owner))
            .size_full()
            .overflow_hidden();
        if !is_virtual {
            if y_enabled && x_enabled {
                content = content.overflow_scroll();
            } else if y_enabled {
                content = content.overflow_y_scroll();
            } else if x_enabled {
                content = content.overflow_x_scroll();
            }
            if y_enabled || x_enabled {
                let handle = self.state.read(cx).handle.clone();
                content = content
                    .track_scroll(&handle)
                    // 滚动本身不触发重渲染，通知 state 让 thumb 跟随。
                    .on_scroll_wheel({
                        let s = self.state.clone();
                        move |_e: &ScrollWheelEvent, _w: &mut Window, cx: &mut App| {
                            s.update(cx, |_, cx| cx.notify());
                        }
                    });
            }
        } else if let Some(v) = &self.virtual_y {
            if let Some(cb) = v.on_delta.clone() {
                content = content.on_scroll_wheel(move |e: &ScrollWheelEvent, w, cx| {
                    let dy: f32 = e.delta.pixel_delta(px(14.)).y.into();
                    if dy.abs() > 0.01 {
                        cb(dy, w, cx);
                    }
                });
            }
        }
        let content = content.child(self.child);

        // 滚轮沿 X 溢出时纵向滚轮按住 Shift 也能滚（gpui 内建行为），这里不额外处理。

        let (idle_col, active_col) = thumb_colors(self.theme);
        let state_for_up = self.state.clone();

        let mut root = div()
            .id(("scroll-area", owner))
            .relative()
            .size_full()
            .on_hover({
                let s = self.state.clone();
                move |h: &bool, _w: &mut Window, cx: &mut App| {
                    s.update(cx, |st, cx| {
                        st.hovered = *h;
                        cx.notify();
                    });
                }
            })
            .child(content)
            // 拖拽释放的兜底清理：gpui 在鼠标释放后不再派发 move 事件，
            // on_drag_move 的 !dragging 分支收不到（纯点击 thumb 不拖也一样），
            // 锚点残留会让滚动条卡在激活态。canvas 的 paint 阶段每帧注册
            // 一次全局 mouse-up 监听（API 语义即逐帧注册），任意位置释放都能清。
            .child(gpui::canvas(
                |_b, _w, _cx| {},
                move |_b, _t, window: &mut Window, _cx: &mut App| {
                    let s = state_for_up.clone();
                    window.on_mouse_event(
                        move |_e: &gpui::MouseUpEvent, _phase, _w: &mut Window, cx: &mut App| {
                            s.update(cx, |st, cx| {
                                if st.end_drag() {
                                    cx.notify();
                                }
                            });
                        },
                    );
                },
            )
            .absolute()
            .size_0());

        // —— Y 滚动条 overlay ——
        if let Some((thumb_h, travel, top)) = y_m {
            let w = if active_y { 10. } else { 8. };
            let color = if active_y { active_col } else { idle_col };
            // 锚定用的当前偏移（真实模式读句柄；虚拟模式取本帧受控值）。
            let cur_off = if is_virtual {
                self.virtual_y.as_ref().map(|v| v.offset).unwrap_or(0.)
            } else {
                let o: f32 = self.state.read(cx).handle.offset().y.into();
                -o
            };

            let mut on_goto: Option<Rc<dyn Fn(f32, &mut Window, &mut App)>> = None;
            if let Some(v) = &self.virtual_y {
                on_goto = Some(v.on_goto.clone());
            }

            root = root.child(
                div()
                    .id(("scroll-track-y", owner))
                    .absolute()
                    .top_0()
                    .right(px(self.right_inset))
                    .h(px(y_track_h))
                    .w(px(14.))
                    .flex()
                    .justify_end()
                    .on_hover({
                        let s = self.state.clone();
                        move |h: &bool, _w: &mut Window, cx: &mut App| {
                            s.update(cx, |st, cx| {
                                st.bar_hover_y = *h;
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        div()
                            .id(("scroll-thumb-y", owner))
                            .debug_selector(move || "scroll-thumb-y".to_string())
                            .mt(px(top))
                            .mr(px(2.))
                            .w(px(w))
                            .h(px(thumb_h))
                            .rounded_full()
                            .bg(rgba(color))
                            // 不加 occlude：它会把自己从父 track 的 hover 命中里
                            // 遮住（移上 thumb 时 track 的 bar_hover 失效、thumb
                            // 掉回 idle 色）。thumb 本就是该区域最上层，命中不受影响。
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let s = self.state.clone();
                                move |e, _w: &mut Window, cx: &mut App| {
                                    let y: f32 = e.position.y.into();
                                    s.update(cx, |st, cx| {
                                        st.drag_y = Some((y, cur_off));
                                        cx.notify();
                                    });
                                }
                            })
                            .on_drag(ScrollDragMarker(owner), |_, _, _, cx| {
                                cx.new(|_| DragGhost)
                            })
                            .on_drag_move({
                                let s = self.state.clone();
                                let ratio = if travel > 0. { y_max / travel } else { 0. };
                                move |e: &DragMoveEvent<ScrollDragMarker>,
                                      w: &mut Window,
                                      cx: &mut App| {
                                    if e.drag(cx).0 != owner {
                                        return;
                                    }
                                    if !e.event.dragging() {
                                        s.update(cx, |st, cx| {
                                            st.drag_y = None;
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    let y: f32 = e.event.position.y.into();
                                    let Some((sy, so)) = s.read(cx).drag_y else {
                                        return;
                                    };
                                    let new_off = (so + (y - sy) * ratio).clamp(0., y_max);
                                    if let Some(cb) = &on_goto {
                                        cb(new_off, w, cx);
                                    } else {
                                        let cur_x = s.read(cx).handle.offset().x;
                                        s.update(cx, |st, cx| {
                                            st.handle
                                                .set_offset(Point::new(cur_x, px(-new_off)));
                                            cx.notify();
                                        });
                                    }
                                }
                            }),
                    ),
            );
        }

        // —— X 滚动条 overlay（仅真实模式）——
        if let Some((thumb_w, travel_x, left)) = x_m {
            let h = if active_x { 10. } else { 8. };
            let color = if active_x { active_col } else { idle_col };
            root = root.child(
                div()
                    .id(("scroll-track-x", owner))
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w(px(x_vp))
                    .h(px(14.))
                    .flex()
                    .on_hover({
                        let s = self.state.clone();
                        move |h: &bool, _w: &mut Window, cx: &mut App| {
                            s.update(cx, |st, cx| {
                                st.bar_hover_x = *h;
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        div()
                            .id(("scroll-thumb-x", owner))
                            .debug_selector(move || "scroll-thumb-x".to_string())
                            .ml(px(left))
                            .mb(px(2.))
                            .w(px(thumb_w))
                            .h(px(h))
                            .rounded_full()
                            .bg(rgba(color))
                            // 同 Y 轴：不加 occlude（hover 命中原因见上）。
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let s = self.state.clone();
                                let anchor_off = x_cur_off;
                                move |e, _w: &mut Window, cx: &mut App| {
                                    let x: f32 = e.position.x.into();
                                    s.update(cx, |st, cx| {
                                        st.drag_x = Some((x, anchor_off));
                                        cx.notify();
                                    });
                                }
                            })
                            .on_drag(ScrollDragMarker(owner), |_, _, _, cx| {
                                cx.new(|_| DragGhost)
                            })
                            .on_drag_move({
                                let s = self.state.clone();
                                let ratio = if travel_x > 0. { x_max / travel_x } else { 0. };
                                move |e: &DragMoveEvent<ScrollDragMarker>,
                                      _w: &mut Window,
                                      cx: &mut App| {
                                    if e.drag(cx).0 != owner {
                                        return;
                                    }
                                    if !e.event.dragging() {
                                        s.update(cx, |st, cx| {
                                            st.drag_x = None;
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    let x: f32 = e.event.position.x.into();
                                    let Some((sx, so)) = s.read(cx).drag_x else {
                                        return;
                                    };
                                    let new_off = (so + (x - sx) * ratio).clamp(0., x_max);
                                    if let Some(cb) = &x_on_goto {
                                        cb(new_off, _w, cx);
                                        return;
                                    }
                                    let cur_y = s.read(cx).handle.offset().y;
                                    s.update(cx, |st, cx| {
                                        st.handle
                                            .set_offset(Point::new(px(-new_off), cur_y));
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
            );
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_metrics_basic() {
        // track 100, page 50, max 50 → content 100, thumb = 100*50/100 = 50
        let (h, travel, top) = thumb_metrics(100., 50., 50., 0.).unwrap();
        assert_eq!(h, 50.);
        assert_eq!(travel, 50.);
        assert_eq!(top, 0.);
        // 滚到一半：top = 25/50*50
        let (_, _, top) = thumb_metrics(100., 50., 50., 25.).unwrap();
        assert_eq!(top, 25.);
        // 滚到底
        let (_, _, top) = thumb_metrics(100., 50., 50., 50.).unwrap();
        assert_eq!(top, 50.);
    }

    #[test]
    fn thumb_metrics_min_and_guards() {
        // thumb 下限 24（min>max panic 守卫）
        let (h, _, _) = thumb_metrics(100., 5., 1000., 0.).unwrap();
        assert_eq!(h, 24.);
        // 无溢出 / 轨道过小 / page 异常 → 不显示
        assert!(thumb_metrics(100., 100., 0., 0.).is_none());
        assert!(thumb_metrics(20., 50., 50., 0.).is_none());
        assert!(thumb_metrics(100., 0., 50., 0.).is_none());
    }

    #[test]
    fn thumb_metrics_clamps_out_of_range_offset() {
        let (_, _, top) = thumb_metrics(100., 50., 50., -10.).unwrap();
        assert_eq!(top, 0.);
        let (_, travel, top) = thumb_metrics(100., 50., 50., 999.).unwrap();
        assert_eq!(top, travel);
    }
}
