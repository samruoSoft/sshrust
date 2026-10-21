//! 新增/编辑服务器的弹出独立窗口。
//!
//! 表单的全部状态（字段实体/认证模式/反馈条/测试中）都在本视图内；
//! 落库与开连接通过 WeakEntity<AppUi> 回主窗口完成（Db 只归 AppUi 管）。
//! 关闭（红绿灯/⌘W/Esc）即放弃，无副作用。

use gpui::{
    App, Context, Entity, IntoElement, ParentElement, Render, Styled, WeakEntity, Window, actions,
    div, prelude::*, px, rgb,
};

use sshrust_core::{AuthMethod, ConnectParams, Transport};

use crate::app_ui::{AppUi, CloseTab};
use crate::db::{ConnectMode, ServerData};
use crate::hosts::StoredAuth;
use crate::icons;
use crate::theme;
use ui_kit::base::button::{self as kit_button, ButtonKind, ButtonSize};
use ui_kit::base::form as kit_form;
use ui_kit::base::text_area::{TextArea, TextAreaField};
use ui_kit::base::text_input::{InputField, InputSize, InputType, SubmitInput, TextField};
use ui_kit::pro::dialog::{DialogCloser, DialogFooter};
use ui_kit::pro::scroll_area;

use crate::icon_picker::{IconPicker, IconPickerEvent};

actions!(form_service_edit, [CancelForm]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Password,
    Key,
}

/// 保存时交给主窗口落库的表单产物。
pub(crate) struct ServerFormOutput {
    pub editing: Option<i64>,
    pub parent: i64,
    /// 显式图标（None = 按名称自动匹配）。
    pub icon: Option<String>,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: StoredAuth,
    pub mode: ConnectMode,
}

/// 打开表单时的初始数据：None = 新增；Some((节点 id, 名称, 数据, 节点图标)) = 编辑。
/// 节点图标空串 = 未显式设置（按名称自动匹配）。
pub type FormLoad = Option<(i64, String, ServerData, String)>;

pub struct ServiceEditForm {
    main: WeakEntity<AppUi>,
    editing: Option<i64>,
    parent: i64,
    /// 内容滚动（窗口矮于表单自然高度时兜底；kit scroll_area 真实模式）。
    scroll: Entity<scroll_area::ScrollAreaState>,
    /// 显式图标（None = 按名称自动匹配）；由图标选择对话框回调写回。
    icon: Option<String>,
    host_f: Entity<TextField>,
    port_f: Entity<TextField>,
    user_f: Entity<TextField>,
    alias_f: Entity<TextField>,
    pass_f: Entity<TextField>,
    key_f: Entity<TextArea>,
    auth_mode: AuthMode,
    /// 连接方式：IP 直连 / iroh P2P（地址字段随之切换：主机名 ↔ NodeID 多行）。
    conn_mode: ConnectMode,
    /// NodeID 编辑（多行、自动换行）：64 hex 单行放不下。
    node_f: Entity<TextArea>,
    error: Option<String>,
    notice: Option<(bool, String)>,
    testing: bool,
}

impl ServiceEditForm {
    pub fn new(
        parent: i64,
        load: FormLoad,
        main: WeakEntity<AppUi>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scroll = cx.new(|_| scroll_area::ScrollAreaState::new(scroll_area::next_owner()));
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        // 界面语言切换：更新带语言占位字并重绘（host/port/user 占位字为语言中性的示例值）。
        cx.observe_global::<crate::i18n::I18nGlobal>(|this, cx| {
            let ph_alias = crate::i18n::t(cx, crate::i18n::Tr::PhAlias);
            let ph_key = crate::i18n::t(cx, crate::i18n::Tr::PhKey);
            this.alias_f.update(cx, |f, cx| f.set_placeholder(ph_alias, cx));
            this.key_f.update(cx, |f, cx| f.set_placeholder(ph_key, cx));
            cx.notify();
        })
        .detach();
        // 背景透明度/主题变化：即时重绘（窗口背景模式为开窗快照，重开生效）。
        cx.observe_global::<crate::theme::BgGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<crate::theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        let this = Self {
            main,
            editing: None,
            parent,
            scroll,
            icon: None,
            host_f: cx.new(|cx| TextField::new("192.168.1.10", InputType::Text, cx)),
            port_f: cx.new(|cx| TextField::new("22", InputType::Number, cx)),
            user_f: cx.new(|cx| TextField::new("root123", InputType::Text, cx)),
            alias_f: cx
                .new(|cx| TextField::new(crate::i18n::t(cx, crate::i18n::Tr::PhAlias), InputType::Text, cx)),
            pass_f: cx.new(|cx| TextField::new("", InputType::Password, cx)),
            key_f: cx.new(|cx| {
                TextArea::new(
                    crate::i18n::t(cx, crate::i18n::Tr::PhKey),
                    6,
                    cx,
                )
            }),
            auth_mode: AuthMode::Password,
            conn_mode: ConnectMode::Ip,
            node_f: cx.new(|cx| TextArea::new("", 2, cx)),
            error: None,
            notice: None,
            testing: false,
        };
        let mut this = this;
        this.apply_load(load, cx);
        this
    }

    /// 载入/重置（复用已开窗口时也走这里）。
    fn apply_load(&mut self, load: FormLoad, cx: &mut Context<Self>) {
        self.error = None;
        self.notice = None;
        self.testing = false;
        match load {
            Some((id, name, data, node_icon)) => {
                self.editing = Some(id);
                self.icon = if node_icon.is_empty() { None } else { Some(node_icon) };
                self.host_f
                    .update(cx, |f, cx| f.set_value(data.host.clone(), cx));
                self.port_f
                    .update(cx, |f, cx| f.set_value(data.port.to_string(), cx));
                self.user_f
                    .update(cx, |f, cx| f.set_value(data.username.clone(), cx));
                self.alias_f.update(cx, |f, cx| f.set_value(name, cx));
                self.conn_mode = data.mode;
                self.node_f
                    .update(cx, |f, cx| f.set_value(data.host.clone(), cx));
                match &data.auth {
                    StoredAuth::Password(pw) => {
                        self.auth_mode = AuthMode::Password;
                        let pw = pw.clone().unwrap_or_default();
                        self.pass_f.update(cx, |f, cx| f.set_value(pw, cx));
                        self.key_f.update(cx, |f, cx| f.set_value("", cx));
                    }
                    StoredAuth::Key { key, .. } => {
                        self.auth_mode = AuthMode::Key;
                        let key = key.clone();
                        self.key_f.update(cx, |f, cx| f.set_value(key, cx));
                        self.pass_f.update(cx, |f, cx| f.set_value("", cx));
                    }
                }
            }
            None => {
                self.editing = None;
                for f in [
                    &self.host_f,
                    &self.port_f,
                    &self.user_f,
                    &self.alias_f,
                    &self.pass_f,
                ] {
                    f.update(cx, |field, cx| field.set_value("", cx));
                }
                self.key_f.update(cx, |f, cx| f.set_value("", cx));
                self.node_f.update(cx, |f, cx| f.set_value("", cx));
                self.auth_mode = AuthMode::Password;
                self.conn_mode = ConnectMode::Ip;
            }
        }
        cx.notify();
    }

    /// 图标选择回调写回（None = 回到按名称自动匹配）。
    pub fn set_icon(&mut self, icon: Option<String>, cx: &mut Context<Self>) {
        self.icon = icon;
        cx.notify();
    }

    /// 别名行尾的图标方格：显示当前图标（None=“自动”），点击弹选择器。
    fn icon_square(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let kit = crate::theme::current(cx).kit();
        let auto_label = crate::i18n::t(cx, crate::i18n::Tr::Auto);
        let (border, inner) = match &self.icon {
            Some(name) => (
                kit.accent,
                gpui::svg()
                    .path(crate::icons::asset_path(name))
                    .size(px(16.))
                    .text_color(rgb(kit.text))
                    .into_any_element(),
            ),
            None => (
                kit.border,
                div()
                    .text_size(px(9.))
                    .text_color(rgb(kit.dim))
                    .child(auto_label)
                    .into_any_element(),
            ),
        };
        div()
            .id("icon-pick-square")
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .rounded_sm()
            .border_1()
            .border_color(rgb(border))
            .bg(kit.surface(kit.panel))
            .hover(|s| s.border_color(rgb(kit.accent)))
            .child(inner)
            .on_click(cx.listener(|this, _ev: &gpui::ClickEvent, _w, cx| {
                this.open_icon_picker(cx);
            }))
            .into_any_element()
    }

    /// 打开图标选择对话框（DialogWindow + footer 插槽 + 事件回调）：
    /// 确定后经回调写回本表单并自动关选择器，取消仅关选择器。
    fn open_icon_picker(&mut self, cx: &mut Context<Self>) {
        let picker = cx.new(|cx| IconPicker::new(self.icon.clone(), cx));
        // 开窗建立的模糊视图需要按当前主题系列 tint（幂等）。
        ui_kit::platform::set_blur_tint(!crate::theme::is_light(cx));
        let spec = ui_kit::pro::dialog::DialogSpec {
            title: crate::i18n::t(cx, crate::i18n::Tr::IconPickerTitle).into(),
            width: 560.,
            height: 520.,
            min_width: 460.,
            min_height: 400.,
            surface_alpha: Some(crate::theme::surface_fx(cx).alpha),
            surface_blur: crate::theme::bg_fx(cx).blur,
            ..Default::default()
        };
        let this = cx.entity();
        let footer = DialogFooter::new(
            move |p: &Entity<IconPicker>,
                  closer: &DialogCloser<IconPicker, IconPickerEvent>,
                  cx: &App| {
                let kit = crate::theme::current(cx).kit();
                let cancel_closer = closer.clone();
                div()
                    .flex()
                    .gap_3()
                    .w_full()
                    .child(kit_button::button_for(
                        "icon-picker-cancel",
                        crate::i18n::t(cx, crate::i18n::Tr::Cancel),
                        ButtonKind::Secondary,
                        ButtonSize::Md,
                        true,
                        kit,
                        p.clone(),
                        move |_p, _e, _w, cx| cancel_closer.close(cx),
                    ))
                    .child(div().flex_1())
                    .child(kit_button::button_for(
                        "icon-picker-ok",
                        crate::i18n::t(cx, crate::i18n::Tr::Ok),
                        ButtonKind::Primary,
                        ButtonSize::Md,
                        true,
                        kit,
                        p.clone(),
                        move |p, _e, _w, cx| p.confirm(cx),
                    ))
                    .into_any_element()
            },
        );
        ui_kit::pro::dialog::open_dialog_ex(
            picker,
            spec,
            Some(footer),
            crate::theme::current(cx).kit(),
            Some(std::rc::Rc::new(
                move |ev: &IconPickerEvent,
                      closer: &DialogCloser<IconPicker, IconPickerEvent>,
                      cx: &mut App| {
                    let pick = match ev {
                        IconPickerEvent::Auto => None,
                        IconPickerEvent::Picked(n) => Some(n.clone()),
                    };
                    let _ = this.update(cx, |f, cx| f.set_icon(pick, cx));
                    closer.close(cx);
                },
            )),
            cx,
        );
    }

    /// 打开时预设的错误提示（缺口令连接引导）。
    pub fn preset_error(&mut self, msg: String, cx: &mut Context<Self>) {
        self.notice = None;
        self.error = Some(msg);
        cx.notify();
    }

    /// 复用已开窗口时重载状态（新增/编辑切换）。
    pub fn load_for(&mut self, parent: i64, load: FormLoad, cx: &mut Context<Self>) {
        self.parent = parent;
        self.apply_load(load, cx);
    }

    // ---------- 连接（新 Tab） ----------

    /// 当前地址值：IP 模式 = 主机名；iroh 模式 = NodeID（压掉换行/空白）。
    fn addr_value(&self, cx: &App) -> String {
        match self.conn_mode {
            ConnectMode::Ip => self.host_f.read(cx).value().trim().to_string(),
            ConnectMode::Iroh => self
                .node_f
                .read(cx)
                .value()
                .split_whitespace()
                .collect::<String>(),
        }
    }

    /// 连接目标的展示名：别名优先；IP = 主机名；iroh = NodeID 前 8 位。
    fn display_target(&self, cx: &App) -> String {
        let alias = self.alias_f.read(cx).value().trim().to_string();
        if !alias.is_empty() {
            return alias;
        }
        let host = self.addr_value(cx);
        match self.conn_mode {
            ConnectMode::Ip => host,
            ConnectMode::Iroh => host.chars().take(8).collect(),
        }
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let params = match self.collect_params(cx) {
            Ok(p) => p,
            Err(msg) => {
                self.notice = None;
                self.error = Some(msg);
                cx.notify();
                return;
            }
        };
        let title = self.display_target(cx);
        let icon = icons::icon_for_name(&title, "server");
        if let Some(main) = self.main.upgrade() {
            let _ = main.update(cx, |ui, cx| {
                ui.open_connecting_tab(title, params, Some(icon), cx);
                ui.activate_main_window(cx);
            });
        }
        // 对话框语义：连接已受理即关窗（校验失败在上面提前 return 保持打开）。
        window.remove_window();
        cx.notify();
    }

    // ---------- 测试连接 ----------

    /// 用当前表单内容发起一次真实连接（不落库、不开终端），
    /// 结果回显到反馈条；12 秒超时防黑洞主机。
    fn test(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.testing {
            return;
        }
        let params = match self.collect_params(cx) {
            Ok(p) => p,
            Err(msg) => {
                self.notice = None;
                self.error = Some(msg);
                cx.notify();
                return;
            }
        };
        let target = match self.conn_mode {
            ConnectMode::Ip => format!(
                "{}:{}",
                self.host_f.read(cx).value().trim(),
                self.port_f.read(cx).value().trim()
            ),
            ConnectMode::Iroh => self.display_target(cx),
        };
        self.error = None;
        self.notice = Some((true, crate::i18n::tf(cx, crate::i18n::Tr::Testing, &[&target])));
        self.testing = true;
        // 超时文案在 spawn 前按当前语言备好（tokio 任务内无 cx）。
        let timeout_msg =
            crate::i18n::t(cx, crate::i18n::Tr::TestTimeout).to_string();
        cx.notify();
        cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = super::RUNTIME
                    .get()
                    .expect("tokio runtime")
                    .spawn(async move {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(12),
                            sshrust_core::session::open_session(params),
                        )
                        .await
                        {
                            Ok(Ok(opened)) => {
                                // 纯测试：连上即关，不留会话。
                                opened.session.close();
                                Ok(())
                            }
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_) => Err(timeout_msg),
                        }
                    })
                    .await
                    .expect("join test session");
                let _ = this.update(cx, |form: &mut Self, cx: &mut Context<Self>| {
                    form.testing = false;
                    match result {
                        Ok(()) => {
                            form.error = None;
                            form.notice = Some((true, crate::i18n::tf(cx, crate::i18n::Tr::TestOk, &[&target])));
                        }
                        Err(msg) => {
                            form.notice = None;
                            form.error = Some(crate::i18n::tf(cx, crate::i18n::Tr::TestFail, &[&msg]));
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();
    }

    // ---------- 保存 ----------

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(out) = self.collect_output(cx) else {
            cx.notify();
            return;
        };
        if let Some(main) = self.main.upgrade() {
            let _ = main.update(cx, |ui, cx| {
                ui.persist_server_form(out, cx);
            });
        }
        window.remove_window();
    }

    fn collect_output(&mut self, cx: &App) -> Option<ServerFormOutput> {
        let (port, transport) = match self.conn_mode {
            ConnectMode::Ip => {
                let Ok(port) = self.port_f.read(cx).value().trim().parse::<u16>() else {
                    self.error = Some(crate::i18n::t(cx, crate::i18n::Tr::ErrPortInvalid).into());
                    return None;
                };
                (port, Transport::Tcp)
            }
            ConnectMode::Iroh => (0, self.iroh_transport(cx)),
        };
        let host = self.addr_value(cx);
        if host.is_empty() {
            self.error = Some(crate::i18n::t(cx, crate::i18n::Tr::ErrHostEmpty).into());
            return None;
        }
        if matches!(self.conn_mode, ConnectMode::Iroh) && !is_node_id(&host) {
            self.error = Some(crate::i18n::t(cx, crate::i18n::Tr::ErrNodeId).into());
            return None;
        }
        let _ = transport;
        let user = self.user_f.read(cx).value();
        let alias = self.alias_f.read(cx).value().trim().to_string();
        let auth = match self.auth_mode {
            AuthMode::Password => {
                let pw = self.pass_f.read(cx).value();
                StoredAuth::Password(if pw.is_empty() { None } else { Some(pw) })
            }
            AuthMode::Key => {
                let key = self.key_f.read(cx).value();
                StoredAuth::Key {
                    key,
                    passphrase: None,
                }
            }
        };
        let name = if alias.is_empty() {
            match self.conn_mode {
                ConnectMode::Ip => host.clone(),
                ConnectMode::Iroh => host.chars().take(8).collect(),
            }
        } else {
            alias
        };
        Some(ServerFormOutput {
            editing: self.editing,
            parent: self.parent,
            icon: self.icon.clone(),
            name,
            host,
            port,
            username: user,
            auth,
            mode: self.conn_mode,
        })
    }

    /// iroh 模式的 relay/令牌：全局设置（设置 › 通用 › 网络）读取；
    /// 空 → None = n0 公共设施。主窗口实体不可达时同样回退 n0。
    fn iroh_transport(&self, cx: &App) -> Transport {
        let (relay, token) = self
            .main
            .upgrade()
            .map(|ui| crate::db::iroh_global(&ui.read(cx).db))
            .unwrap_or((None, None));
        Transport::Iroh { relay, token }
    }

    /// 连接/测试用参数（比保存严格：用户名与密钥格式必校）。
    fn collect_params(&self, cx: &App) -> Result<ConnectParams, String> {
        let host = self.addr_value(cx);
        if host.is_empty() {
            return Err(crate::i18n::t(cx, crate::i18n::Tr::ErrHostEmpty).into());
        }
        let (port, transport) = match self.conn_mode {
            ConnectMode::Ip => {
                let port: u16 = self
                    .port_f
                    .read(cx)
                    .value()
                    .trim()
                    .parse()
                    .map_err(|_| crate::i18n::t(cx, crate::i18n::Tr::ErrPortRange).to_string())?;
                (port, Transport::Tcp)
            }
            ConnectMode::Iroh => {
                if !is_node_id(&host) {
                    return Err(crate::i18n::t(cx, crate::i18n::Tr::ErrNodeId).into());
                }
                (0, self.iroh_transport(cx))
            }
        };
        let username = self.user_f.read(cx).value();
        if username.trim().is_empty() {
            return Err(crate::i18n::t(cx, crate::i18n::Tr::ErrUserEmpty).into());
        }
        let auth = match self.auth_mode {
            AuthMode::Password => AuthMethod::Password(self.pass_f.read(cx).value()),
            AuthMode::Key => {
                let key = self.key_f.read(cx).value();
                if !key.contains("PRIVATE KEY") {
                    return Err(crate::i18n::t(cx, crate::i18n::Tr::ErrKeyMissing).into());
                }
                AuthMethod::Key {
                    key,
                    passphrase: None,
                }
            }
        };
        Ok(ConnectParams {
            host,
            port,
            username,
            auth,
            cols: 100,
            rows: 30,
            transport,
        })
    }
}

/// NodeID 合法性：64 位十六进制。
fn is_node_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Render for ServiceEditForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::current(cx);
        let kit = pal.kit();
        let editing = self.editing.is_some();
        let tr = |k: crate::i18n::Tr| crate::i18n::t(cx, k);
        let field =
            |f: &Entity<TextField>| InputField::new(f.clone()).size(InputSize::Md).theme(kit);
        let auth_field = match self.auth_mode {
            AuthMode::Password => {
                kit_form::labeled_row(tr(crate::i18n::Tr::LabelPassword), field(&self.pass_f), kit)
            }
            AuthMode::Key => kit_form::labeled_block(
                tr(crate::i18n::Tr::LabelPrivateKey),
                TextAreaField::new(self.key_f.clone()).theme(kit),
                kit,
            ),
        };

        // 表单体直接铺满窗口（独立窗口无需再套卡片）：
        // 宽随窗口、高至少一屏（窗口高时弹性空隙把按钮压底），
        // 内容超过一屏时由外层 scroll_area 兜底滚动。
        let body =
            div()
                .id("form-body")
                .flex()
                .flex_col()
                .gap_4()
                .w_full()
                .min_h(gpui::relative(1.))
                .p_6()
                .child(
                    div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().w(px(72.)).flex_none().text_color(rgb(pal.muted)).child(tr(crate::i18n::Tr::LabelAlias)))
                    .child(div().flex_1().min_w_0().child(field(&self.alias_f)))
                    .child(self.icon_square(cx)),
            )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(72.)).flex_none().text_color(rgb(pal.muted)).child(tr(crate::i18n::Tr::LabelConnMode)))
                        .child(kit_form::segmented(
                            "conn-mode",
                            &[
                                (tr(crate::i18n::Tr::ConnModeIp), ConnectMode::Ip),
                                (tr(crate::i18n::Tr::ConnModeIroh), ConnectMode::Iroh),
                            ],
                            self.conn_mode,
                            kit,
                            cx,
                            |this, mode, _window, cx| {
                                if this.conn_mode != mode {
                                    // 地址值在单/多行字段间搬运，切换后无需重贴。
                                    match mode {
                                        ConnectMode::Ip => {
                                            let v = this.node_f.read(cx).value();
                                            this.host_f.update(cx, |f, cx| f.set_value(v.trim().to_string(), cx));
                                        }
                                        ConnectMode::Iroh => {
                                            let v = this.host_f.read(cx).value();
                                            this.node_f.update(cx, |f, cx| f.set_value(v, cx));
                                        }
                                    }
                                    this.conn_mode = mode;
                                }
                                cx.notify();
                            },
                        )),
                )
                .child(match self.conn_mode {
                    // 主机名单行；NodeID（64 hex 单行放不下）多行 + 自动换行。
                    ConnectMode::Ip => kit_form::labeled_row(
                        tr(crate::i18n::Tr::LabelHost),
                        field(&self.host_f),
                        kit,
                    )
                    .into_any_element(),
                    ConnectMode::Iroh => kit_form::labeled_block(
                        tr(crate::i18n::Tr::LabelNodeId),
                        TextAreaField::new(self.node_f.clone()).theme(kit),
                        kit,
                    )
                    .into_any_element(),
                })
                .children(match self.conn_mode {
                    ConnectMode::Ip => vec![
                        kit_form::labeled_row(tr(crate::i18n::Tr::LabelPort), field(&self.port_f), kit)
                            .into_any_element(),
                    ],
                    ConnectMode::Iroh => vec![
                        div()
                            // 长文案须可收缩（见 settings_view 网络页同类修复）：
                            // 英文单行测量宽超列宽会撑破 flex 协商，连带输入框
                            // w_full 失效塌成小方块。
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(pal.dim))
                            .child(tr(crate::i18n::Tr::IrohFormHint))
                            .into_any_element(),
                    ],
                })
                .child(kit_form::labeled_row(tr(crate::i18n::Tr::LabelUsername), field(&self.user_f), kit))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(72.)).text_color(rgb(pal.muted)).child(tr(crate::i18n::Tr::LabelAuth)))
                        .child(kit_form::segmented(
                            "auth-mode",
                            &[
                                (tr(crate::i18n::Tr::AuthPassword), AuthMode::Password),
                                (tr(crate::i18n::Tr::AuthKey), AuthMode::Key),
                            ],
                            self.auth_mode,
                            kit,
                            cx,
                            |this, mode, _window, cx| {
                                this.auth_mode = mode;
                                cx.notify();
                            },
                        )),
                )
                .child(auth_field)
                // 窗口高于内容时吸收空隙，把操作行压到底部。
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(div().flex_1().child(kit_button::button(
                            "btn-test",
                            if self.testing {
                                tr(crate::i18n::Tr::BtnTesting)
                            } else {
                                tr(crate::i18n::Tr::BtnTest)
                            },
                            ButtonKind::Secondary,
                            ButtonSize::Md,
                            !self.testing,
                            kit,
                            cx,
                            |this, _ev: &gpui::ClickEvent, window, cx| this.test(window, cx),
                        )))
                        .child(div().flex_1().child(kit_button::button(
                            "btn-save-host",
                            if editing {
                                tr(crate::i18n::Tr::BtnSaveEdit)
                            } else {
                                tr(crate::i18n::Tr::BtnSaveNew)
                            },
                            ButtonKind::Primary,
                            ButtonSize::Md,
                            true,
                            kit,
                            cx,
                            |this, _ev: &gpui::ClickEvent, window, cx| this.save(window, cx),
                        ))),
                )
                .children(self.error.clone().map(|msg| {
                    div()
                        .text_color(rgb(pal.error))
                        .text_size(px(12.))
                        .child(msg)
                        .into_any_element()
                }))
                .children(self.notice.as_ref().map(|(ok, msg)| {
                    div()
                        .text_color(rgb(if *ok { pal.ok } else { pal.error }))
                        .text_size(px(12.))
                        .child(msg.clone())
                        .into_any_element()
                }))
                .child(div().text_size(px(11.)).text_color(rgb(pal.dim)).child(
                    tr(crate::i18n::Tr::FormHint),
                ))
                .child(div().flex_1());

        div()
            .id("form-window-root")
            .key_context("form_service_edit")
            .size_full()
            .flex()
            .bg(pal.surface(pal.bg))
            // 颜色/字号继承根：独立窗口不再挂在 AppUi 根下，必须自带，
            // 否则文字回落 gpui 默认黑色。
            .text_color(rgb(pal.text))
            .text_size(px(13.))
            .font_family("System UI")
            .on_action(cx.listener(|_this, _: &CancelForm, window, _cx| {
                window.remove_window();
            }))
            .on_action(cx.listener(|_this, _: &CloseTab, window, _cx| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &SubmitInput, window, cx| {
                // 回车即连接（与主窗口表单时代一致）。
                this.connect(window, cx);
            }))
            .child(scroll_area::ScrollArea::new(self.scroll.clone(), body).theme(kit))
    }
}
