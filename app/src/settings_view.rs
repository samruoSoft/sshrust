//! App 设置弹出窗：终端配色 + 本地着色模板（用户自定义、可保存多套）。
//!
//! 设置窗内容自持状态与独立的 Db 连接，任何改动即时落库并发
//! [`SettingsEvent::TerminalAppearance`]；主窗口收到后重载外观并推入已开终端。
//! 后续更多设置项（字体/字号等）在此窗口追加分区。

use gpui::{
    App, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::*, px, rgb,
};

use crate::db::Db;
use crate::theme;
use ui_kit::base::button::{self as kit_button, ButtonKind, ButtonSize};
use ui_kit::base::text_input::{InputField, InputSize, InputType, TextField};
use ui_kit::pro::dialog;
use ui_kit::pro::scroll_area;

use gpui_terminal::{RuleStyle, StyleTemplate, STYLE_CATS, STYLE_SLOTS};

/// 设置窗 → 主窗口事件。UiTheme=界面主题切换（主窗 set_ui_theme 执行联动）。
pub enum SettingsEvent {
    TerminalAppearance,
    UiTheme(usize),
    /// 外部搜索引擎切换（主窗 set_search_engine 同步内存态）。
    SearchEngine(crate::procs::SearchEngine),
}

/// 当前模板来源：内置下标或用户模板 id。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TplId {
    Builtin(usize),
    User(i64),
}

/// settings 键：当前着色模板（"b:<内置下标>" / "u:<模板 id>"）。
pub const KEY_STYLE_CURRENT: &str = "terminal.style.current";

/// 内置模板目录（不可编辑，编辑时自动转用户副本）。
/// 返回双语 key（显示名经 i18n 取；DB 只存索引不存名）。
pub fn builtin_style(idx: usize) -> Option<(crate::i18n::Tr, StyleTemplate)> {
    match idx {
        0 => Some((crate::i18n::Tr::TplStandard, StyleTemplate::standard())),
        1 => Some((crate::i18n::Tr::TplMinimal, StyleTemplate::minimal())),
        _ => None,
    }
}

/// 从 DB 解析当前生效的着色模板（脏数据回退内置「标准」）。
pub fn load_style(db: &Db) -> StyleTemplate {
    let raw = db.get_setting(KEY_STYLE_CURRENT).unwrap_or_else(|| "b:0".into());
    match raw.split_once(':') {
        Some(("b", idx)) => idx
            .parse::<usize>()
            .ok()
            .and_then(builtin_style)
            .map(|(_, tpl)| tpl)
            .unwrap_or_else(StyleTemplate::standard),
        Some(("u", id)) => id
            .parse::<i64>()
            .ok()
            .and_then(|id| serde_json::from_str(&db.style_template_data(id)?).ok())
            .unwrap_or_else(StyleTemplate::standard),
        _ => StyleTemplate::standard(),
    }
}

/// 颜色槽位在指定终端配色下的预览 RGB（下标语义与 fork 引擎一致）。
fn slot_rgb(p: &theme::TermPalette, slot: usize) -> u32 {
    let blend = |a: u32, b: u32, t: f32| -> u32 {
        let mix = |sa: u32, sb: u32| -> u32 {
            (((a >> sa & 0xff) as f32 * t + (b >> sb & 0xff) as f32 * (1.0 - t)).round()) as u32
        };
        (mix(16, 16) << 16) | (mix(8, 8) << 8) | mix(0, 0)
    };
    match slot {
        0 => p.fg,
        1 => blend(p.fg, p.bg, 0.55),
        s if (2..=17).contains(&s) => p.ansi[s - 2],
        _ => p.fg,
    }
}

/// 字号合法范围与缺省值。
pub const FONT_SIZE_MIN: f32 = 9.0;
pub const FONT_SIZE_MAX: f32 = 28.0;
pub const FONT_SIZE_DEFAULT: f32 = 14.0;

/// 字号归一化（空/越界回默认）。
pub fn normalize_font_size(v: Option<f32>) -> f32 {
    v.unwrap_or(FONT_SIZE_DEFAULT).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
}

/// 设置窗左侧导航分组（树形路径「通用 › 系统」「外观 › 控制台」）。
/// 「系统」详情页含 语言 / 窗口（背景模糊）/ 界面主题 三节，节间分割线。
/// 后续设置项在此追加：新条目 + 对应详情页。
const NAV: &[(crate::i18n::Tr, &[crate::i18n::Tr])] = &[
    (crate::i18n::Tr::NavGeneral, &[crate::i18n::Tr::NavSystem]),
    (
        crate::i18n::Tr::NavAppearance,
        &[crate::i18n::Tr::NavConsole],
    ),
];

/// 导航条目当前支持的详情页。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    System,
    Console,
}

fn page_of(group: usize, item: usize) -> Option<Page> {
    match (group, item) {
        (0, 0) => Some(Page::System),
        (1, 0) => Some(Page::Console),
        _ => None,
    }
}

/// 常见等宽字体候选（实际可选项 = 候选 ∩ 系统已装字体 ∪ 当前使用项）。
const MONO_CANDIDATES: &[&str] = &[
    "Menlo",
    "Monaco",
    "Courier New",
    "SF Mono",
    "JetBrains Mono",
    "Fira Code",
    "Hack",
    "Sauce Code Pro",
    "Cascadia Code",
    "IBM Plex Mono",
];

pub struct SettingsView {
    db: Db,
    /// 终端配色当前选择（系列随界面主题）。
    term_theme_idx: usize,
    /// 用户模板列表（id, 名称）。
    user_templates: Vec<(i64, String)>,
    current: TplId,
    /// 当前编辑中的规则集（与 DB 持久化保持同步）。
    draft: StyleTemplate,
    /// 终端字体（全局设置，不随模板）。
    font_family: String,
    font_size: f32,
    /// 字体可选项（候选 ∩ 系统字体，含当前项）。
    font_choices: Vec<String>,
    /// 当前详情页（左侧导航选中项）。
    page: Page,
    /// 界面语言设置值（chips 选中态；改动即时落库 + set_global）。
    lang: crate::i18n::Lang,
    /// 背景透明度设置值（chips 选中态；改动即时落库 + set_global，全窗口生效）。
    bg_fx: theme::BgFx,
    /// 外部搜索引擎（chips 选中态；改动落库 + 事件同步主窗）。
    search_engine: crate::procs::SearchEngine,
    /// 已应用到本设置窗的背景效果（render 对账：拖动时本窗即时预览）。
    applied_fx: theme::BgFx,
    name_f: Entity<TextField>,
    scroll: Entity<scroll_area::ScrollAreaState>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let db = Db::open();
        let ui_theme_idx = cx
            .try_global::<theme::ThemeGlobal>()
            .map(|g| g.0)
            .unwrap_or(0);
        let series = theme::ui_series(ui_theme_idx);
        let term_theme_idx = db
            .get_setting(series.setting_key())
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&i| theme::term_for(i).series == series)
            .unwrap_or_else(|| theme::term_default(series));
        let user_templates = db
            .list_style_templates()
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect();
        let current = match db.get_setting(KEY_STYLE_CURRENT) {
            Some(raw) => match raw.split_once(':') {
                Some(("b", idx)) => idx
                    .parse::<usize>()
                    .ok()
                    .filter(|&i| builtin_style(i).is_some())
                    .map(TplId::Builtin)
                    .unwrap_or(TplId::Builtin(0)),
                Some(("u", id)) => id
                    .parse::<i64>()
                    .ok()
                    .filter(|id| db.style_template_data(*id).is_some())
                    .map(TplId::User)
                    .unwrap_or(TplId::Builtin(0)),
                _ => TplId::Builtin(0),
            },
            None => TplId::Builtin(0),
        };
        let draft = load_style(&db);
        let font_family = db
            .get_setting("terminal.font.family")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "Menlo".into());
        let font_size = normalize_font_size(
            db.get_setting("terminal.font.size").and_then(|v| v.parse::<f32>().ok()),
        );
        // 字体可选项：候选名单 ∩ 系统已装字体；当前使用项保底在列。
        let installed: std::collections::HashSet<String> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .collect();
        let mut font_choices: Vec<String> = MONO_CANDIDATES
            .iter()
            .filter(|f| installed.contains(**f))
            .map(|f| f.to_string())
            .collect();
        if !font_choices.iter().any(|f| f == &font_family) {
            font_choices.insert(0, font_family.clone());
        }
        let name_f = cx.new(|cx| {
            TextField::new(crate::i18n::t(cx, crate::i18n::Tr::PhTplName), InputType::Text, cx)
        });
        let scroll = cx.new(|_| scroll_area::ScrollAreaState::new(scroll_area::next_owner()));
        cx.observe(&scroll, |_, _, cx| cx.notify()).detach();
        let lang = crate::i18n::Lang::parse(
            &db.get_setting(crate::i18n::KEY_LANG).unwrap_or_default(),
        );
        let bg_fx = theme::BgFx::load(&db);
        // 外部搜索引擎（进程详情搜索用）。
        let search_engine = crate::procs::SearchEngine::parse(
            &db.get_setting(crate::procs::SETTING_SEARCH_ENGINE).unwrap_or_default(),
        );
        // 语言切换：本窗（导航/分区/按钮）即时重绘；占位字一并更新。
        cx.observe_global::<crate::i18n::I18nGlobal>(|this, cx| {
            let ph = crate::i18n::t(cx, crate::i18n::Tr::PhTplName);
            this.name_f.update(cx, |f, cx| f.set_placeholder(ph, cx));
            cx.notify();
        })
        .detach();
        // 主题/背景效果切换：表面色与有效透明度即时跟随。
        cx.observe_global::<theme::ThemeGlobal>(|_, cx| cx.notify()).detach();
        cx.observe_global::<theme::BgGlobal>(|_, cx| cx.notify()).detach();
        Self {
            db,
            term_theme_idx,
            user_templates,
            current,
            draft,
            font_family,
            font_size,
            font_choices,
            // 默认仍落在控制台页（既有习惯；语言/窗口/主题在「通用 › 系统」页下）。
            page: Page::Console,
            lang,
            bg_fx,
            applied_fx: bg_fx,
            search_engine,
            name_f,
            scroll,
        }
    }

    fn persist_selection(&mut self) {
        let v = match self.current {
            TplId::Builtin(i) => format!("b:{i}"),
            TplId::User(id) => format!("u:{id}"),
        };
        self.db.set_setting(KEY_STYLE_CURRENT, &v);
    }

    /// 当前模板数据落库（仅用户模板需要持久化）。
    fn persist_draft(&mut self) {
        if let TplId::User(id) = self.current {
            let name = self
                .user_templates
                .iter()
                .find(|(tid, _)| *tid == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            let data = serde_json::to_string(&self.draft).unwrap_or_default();
            self.db.update_style_template(id, &name, &data);
        }
    }

    fn reload_user_templates(&mut self) {
        self.user_templates = self
            .db
            .list_style_templates()
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect();
    }

    /// 取不重名的模板名（「新模板」「新模板 2」…）。
    fn unique_name(&self, base: &str) -> String {
        let taken: Vec<String> = self.user_templates.iter().map(|(_, n)| n.clone()).collect();
        if !taken.iter().any(|n| n == base) {
            return base.to_string();
        }
        for k in 2.. {
            let cand = format!("{base} {k}");
            if !taken.iter().any(|n| n == &cand) {
                return cand;
            }
        }
        unreachable!()
    }

    /// 规则变更入口：内置模板先自动转「副本」再落库；任何变更即时生效。
    /// 副本名按当前语言生成（用户数据，落库后不随语言切换回翻）。
    fn edit_rule(&mut self, cat: usize, mutate: impl FnOnce(&mut RuleStyle), cx: &mut Context<Self>) {
        if let TplId::Builtin(b) = self.current {
            let (bkey, _) = builtin_style(b).unwrap_or((crate::i18n::Tr::TplFallback, StyleTemplate::standard()));
            let bname = crate::i18n::t(cx, bkey);
            let name = self.unique_name(&crate::i18n::tlf(
                crate::i18n::current(cx),
                crate::i18n::Tr::CopySuffix,
                &[bname],
            ));
            let data = serde_json::to_string(&self.draft).unwrap_or_default();
            let id = self.db.insert_style_template(&name, &data);
            self.current = TplId::User(id);
            self.reload_user_templates();
            self.persist_selection();
        }
        if let Some(r) = self.draft.rules.get_mut(cat) {
            mutate(r);
        }
        self.persist_draft();
        cx.emit(SettingsEvent::TerminalAppearance);
        cx.notify();
    }

    /// 选择模板（内置或已存用户模板）。
    fn select_tpl(&mut self, id: TplId, cx: &mut Context<Self>) {
        self.current = id;
        self.draft = load_style(&self.db);
        self.persist_selection();
        cx.emit(SettingsEvent::TerminalAppearance);
        cx.notify();
    }

    /// 切换界面语言：写库 + 更新全局镜像；本窗与主窗（含弹出窗/表单等
    /// observe_global 的视图）即时重绘，无需重启。
    fn set_lang(&mut self, lang: crate::i18n::Lang, cx: &mut Context<Self>) {
        self.lang = lang;
        self.db.set_setting(crate::i18n::KEY_LANG, lang.as_str());
        cx.set_global(crate::i18n::I18nGlobal(crate::i18n::resolve(lang)));
        cx.notify();
    }

    /// 切换背景透明度/模糊：写库 + 更新全局镜像；主窗/弹出窗/表单等
    /// observe_global 的视图即时重绘，窗口背景模式在各自 render 里对账。
    fn set_bg_blur(&mut self, blur: bool, cx: &mut Context<Self>) {
        let fx = theme::BgFx { blur };
        self.bg_fx = fx;
        fx.save(&mut self.db);
        cx.set_global(theme::BgGlobal(fx));
        cx.notify();
    }

    /// 切换外部搜索引擎：写库 + 本窗选中态 + 事件同步主窗（搜索按需读取）。
    fn set_search_engine(&mut self, e: crate::procs::SearchEngine, cx: &mut Context<Self>) {
        if self.search_engine != e {
            self.search_engine = e;
            self.db
                .set_setting(crate::procs::SETTING_SEARCH_ENGINE, e.as_str());
            cx.emit(SettingsEvent::SearchEngine(e));
        }
        cx.notify();
    }

    fn section_label(text: &str, pal: &theme::Palette) -> impl IntoElement {
        div()
            .text_size(px(12.))
            .text_color(rgb(pal.muted))
            .pb_1()
            .child(text.to_string())
    }

    /// 「通用 › 系统」详情页：语言节 —分割线— 窗口节（仅背景模糊）
    /// —分割线— 界面主题节（chips 即时生效，原侧栏 ◐ 入口移入）。
    fn system_page(&self, pal: &theme::PalFx, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let loc = crate::i18n::current(cx);

        // —— 语言 chips ——
        let mut lang_chips = div().flex().flex_wrap().gap(px(4.));
        for (i, lang) in crate::i18n::LANG_OPTIONS.iter().copied().enumerate() {
            let selected = self.lang == lang;
            lang_chips = lang_chips.child(
                div()
                    .id(gpui::SharedString::from(format!("lang-{i}")))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(pal.border)))
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.set_lang(lang, cx);
                    }))
                    .child(crate::i18n::lang_label(lang, loc)),
            );
        }

        // —— 背景模糊开关（开/关 chips） ——
        let mut blur_chips = div().flex().flex_wrap().gap(px(4.));
        for (on, label) in [
            (true, crate::i18n::tl(loc, crate::i18n::Tr::ToggleOn)),
            (false, crate::i18n::tl(loc, crate::i18n::Tr::ToggleOff)),
        ] {
            let selected = self.bg_fx.blur == on;
            blur_chips = blur_chips.child(
                div()
                    .id(gpui::SharedString::from(if on { "bg-blur-on" } else { "bg-blur-off" }))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(pal.border)))
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.set_bg_blur(on, cx);
                    }))
                    .child(label),
            );
        }

        // —— 界面主题 chips（当前选择读 ThemeGlobal；点击经事件回主窗切换） ——
        let cur_theme = cx
            .try_global::<theme::ThemeGlobal>()
            .map(|g| g.0)
            .unwrap_or(0);
        let mut theme_chips = div().flex().flex_wrap().gap(px(4.));
        for (i, (name, _)) in theme::THEMES.iter().enumerate() {
            let selected = i == cur_theme;
            let name = *name;
            theme_chips = theme_chips.child(
                div()
                    .id(gpui::SharedString::from(format!("ui-theme-{i}")))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(pal.border)))
                    .on_click(cx.listener(move |_this, _ev, _w, cx| {
                        cx.emit(SettingsEvent::UiTheme(i));
                    }))
                    .child(name),
            );
        }

        // —— 搜索引擎 chips（点击落库 + 事件同步主窗的搜索入口） ——
        let mut engine_chips = div().flex().flex_wrap().gap(px(4.));
        for engine in crate::procs::SearchEngine::ALL {
            let selected = self.search_engine == engine;
            let label = engine.label();
            engine_chips = engine_chips.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "search-engine-{}",
                        engine.as_str()
                    )))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(pal.border)))
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        this.set_search_engine(engine, cx);
                    }))
                    .child(label),
            );
        }

        let resolved = crate::i18n::resolve(self.lang);
        div()
            .id("settings-system")
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .min_h(gpui::relative(1.))
            .p_6()
            .text_size(px(13.))
            // —— 语言节 ——
            .child(Self::section_label(
                crate::i18n::tl(loc, crate::i18n::Tr::SecLanguage),
                pal,
            ))
            .child(lang_chips)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(pal.dim))
                    .child(crate::i18n::tlf(
                        loc,
                        crate::i18n::Tr::LangCurrent,
                        &[crate::i18n::locale_label(resolved)],
                    )),
            )
            // —— 分割线 ——
            .child(div().h(px(1.)).bg(rgb(pal.divider())))
            // —— 窗口节（仅背景模糊；透明度由 模糊×主题系列 推导） ——
            .child(Self::section_label(
                crate::i18n::tl(loc, crate::i18n::Tr::SecWindow),
                pal,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(rgb(pal.muted))
                            .child(crate::i18n::tl(loc, crate::i18n::Tr::SecBgBlur)),
                    )
                    .child(blur_chips),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(pal.dim))
                    .child(crate::i18n::tl(loc, crate::i18n::Tr::BgBlurHint)),
            )
            // —— 分割线 ——
            .child(div().h(px(1.)).bg(rgb(pal.divider())))
            // —— 界面主题节 ——
            .child(Self::section_label(
                crate::i18n::tl(loc, crate::i18n::Tr::MenuUiTheme),
                pal,
            ))
            .child(theme_chips)
            // —— 分割线 ——
            .child(div().h(px(1.)).bg(rgb(pal.divider())))
            // —— 搜索引擎节（进程详情的外部搜索） ——
            .child(Self::section_label(
                crate::i18n::tl(loc, crate::i18n::Tr::SecSearchEngine),
                pal,
            ))
            .child(engine_chips)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(pal.dim))
                    .child(crate::i18n::tl(loc, crate::i18n::Tr::SearchEngineHint)),
            )
    }
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 背景效果对账：设置窗自身也跟随（拖 chips 即时预览本窗）。
        let fx = theme::bg_fx(cx);
        if fx != self.applied_fx {
            self.applied_fx = fx;
            window.set_background_appearance(fx.window_appearance());
            // 重开模糊会重建 NSVisualEffectView，需按主题系列重新 tint。
            ui_kit::platform::set_blur_tint(!theme::is_light(cx));
        }
        let pal = theme::current(cx);
        let kit = pal.kit();
        let loc = crate::i18n::current(cx);
        let series = theme::ui_series(
            cx.try_global::<theme::ThemeGlobal>().map(|g| g.0).unwrap_or(0),
        );

        // ----- 终端配色（仅当前系列，点击选择） -----
        let cur_pal_idx = self.term_theme_idx;
        let mut palettes = div().flex().flex_col().gap(px(2.));
        for (idx, name, p) in theme::term_themes_in(series) {
            let selected = idx == cur_pal_idx;
            let cell = |c: u32| div().size(px(8.)).rounded_sm().bg(rgb(c));
            palettes = palettes.child(
                div()
                    .id(gpui::SharedString::from(format!("pal-{idx}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(26.))
                    .px(px(6.))
                    .rounded_md()
                    .cursor_pointer()
                    .bg(pal.surface(if selected { pal.card } else { pal.bg }))
                    .hover(|s| s.bg(pal.surface(pal.card)))
                    .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                        let s = theme::term_for(idx).series;
                        this.db.set_setting(s.setting_key(), &format!("{idx}"));
                        this.term_theme_idx = idx;
                        cx.emit(SettingsEvent::TerminalAppearance);
                        cx.notify();
                    }))
                    .child(if selected {
                        div().text_color(rgb(pal.accent)).child("✓").into_any_element()
                    } else {
                        div().w(px(12.)).into_any_element()
                    })
                    .child(div().flex_1().child(name.to_string()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(1.5))
                            .rounded_md()
                            .bg(rgb(p.bg))
                            .border_1()
                            .border_color(gpui::rgba(0x80808066))
                            .px(px(3.))
                            .child(cell(p.ansi[1]))
                            .child(cell(p.ansi[2]))
                            .child(cell(p.ansi[3]))
                            .child(cell(p.ansi[4]))
                            .child(cell(p.ansi[5]))
                            .child(cell(p.ansi[6])),
                    ),
            );
        }

        // ----- 模板 chips -----
        let mut chips = div().flex().flex_wrap().gap(px(4.));
        for b in 0.. {
            match builtin_style(b) {
                Some((name_key, _)) => {
                    let selected = self.current == TplId::Builtin(b);
                    chips = chips.child(
                        div()
                            .id(gpui::SharedString::from(format!("tpl-b{b}")))
                            .flex_none()
                            .h(px(24.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                            .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                            .hover(|s| s.bg(rgb(if selected { pal.accent } else { pal.border })))
                            .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                                this.select_tpl(TplId::Builtin(b), cx);
                            }))
                            .child(crate::i18n::tl(loc, name_key)),
                    );
                }
                None => break,
            }
        }
        for (id, name) in self.user_templates.clone() {
            let selected = self.current == TplId::User(id);
            chips = chips.child(
                div()
                    .id(gpui::SharedString::from(format!("tpl-u{id}")))
                    .flex_none()
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(if selected { pal.accent } else { pal.border })))
                    .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                        this.select_tpl(TplId::User(id), cx);
                    }))
                    .child(name),
            );
        }

        // ----- 模板操作行（名称输入 + 新建/另存为/删除） -----
        let is_user = matches!(self.current, TplId::User(_));
        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div().flex_1().min_w_0().child(
                    InputField::new(self.name_f.clone()).size(InputSize::Sm).theme(kit),
                ),
            )
            .child(kit_button::button(
                "tpl-new",
                crate::i18n::tl(loc, crate::i18n::Tr::BtnTplNew),
                ButtonKind::Secondary,
                ButtonSize::Sm,
                true,
                kit,
                cx,
                |s: &mut SettingsView, _ev, _w, cx| {
                    let name = s.unique_name(crate::i18n::t(cx, crate::i18n::Tr::TplNewBase));
                    let data = serde_json::to_string(&s.draft).unwrap_or_default();
                    let id = s.db.insert_style_template(&name, &data);
                    s.reload_user_templates();
                    s.select_tpl(TplId::User(id), cx);
                },
            ))
            .child(kit_button::button(
                "tpl-save-as",
                crate::i18n::tl(loc, crate::i18n::Tr::BtnTplSaveAs),
                ButtonKind::Secondary,
                ButtonSize::Sm,
                true,
                kit,
                cx,
                |s: &mut SettingsView, _ev, _w, cx| {
                    let name = s.name_f.read(cx).value().trim().to_string();
                    let name = if name.is_empty() {
                        s.unique_name(crate::i18n::t(cx, crate::i18n::Tr::TplNewBase))
                    } else {
                        name
                    };
                    let data = serde_json::to_string(&s.draft).unwrap_or_default();
                    let id = s.db.insert_style_template(&name, &data);
                    s.reload_user_templates();
                    s.select_tpl(TplId::User(id), cx);
                },
            ))
            .when(is_user, |row| {
                row.child(kit_button::button(
                    "tpl-del",
                    crate::i18n::tl(loc, crate::i18n::Tr::Delete),
                    ButtonKind::Danger,
                    ButtonSize::Sm,
                    true,
                    kit,
                    cx,
                    |s: &mut SettingsView, _ev, _w, cx| {
                        if let TplId::User(id) = s.current {
                            s.db.delete_style_template(id);
                            s.reload_user_templates();
                            s.current = TplId::Builtin(0);
                            s.persist_selection();
                            s.draft = load_style(&s.db);
                            cx.emit(SettingsEvent::TerminalAppearance);
                            cx.notify();
                        }
                    },
                ))
            });

        // ----- 规则列表 -----
        let term_pal = theme::term_for(self.term_theme_idx);
        let mut rules = div().flex().flex_col().gap(px(1.));
        for cat in 0..STYLE_CATS.len() {
            let cat_label = crate::i18n::tl(loc, crate::i18n::style_cat(STYLE_CATS[cat].0));
            let rule = self.draft.rules.get(cat).copied().unwrap_or(RuleStyle {
                enabled: false,
                slot: 0,
                bold: false,
                underline: false,
            });

            // 开关
            let mut toggle = div()
                .id(gpui::SharedString::from(format!("rule-on-{cat}")))
                .size(px(14.))
                .flex_none()
                .rounded(px(4.))
                .cursor_pointer()
                .border_1()
                .border_color(rgb(if rule.enabled { pal.accent } else { pal.border }));
            if rule.enabled {
                toggle = toggle.bg(rgb(pal.accent));
            }
            let toggle = toggle
                .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                    this.edit_rule(cat, |r| r.enabled = !r.enabled, cx);
                }));

            // 色槽位（18 块）
            let mut swatches = div().flex().items_center().gap(px(2.));
            for (s, _) in STYLE_SLOTS.iter().enumerate() {
                let color = slot_rgb(term_pal, s);
                let selected = rule.slot == s as u8;
                let mut sw = div()
                    .id(gpui::SharedString::from(format!("rule-{cat}-slot-{s}")))
                    .size(px(13.))
                    .flex_none()
                    .rounded(px(3.))
                    .cursor_pointer()
                    .bg(rgb(color))
                    .opacity(if rule.enabled { 1.0 } else { 0.35 });
                sw = if selected {
                    sw.border_2().border_color(rgb(pal.text))
                } else {
                    sw.border_1().border_color(gpui::rgba(0x80808055))
                };
                let slot = s as u8;
                swatches = swatches.child(
                    sw.on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                        this.edit_rule(cat, |r| {
                            r.slot = slot;
                            r.enabled = true;
                        }, cx);
                    })),
                );
            }

            // B / U 标记按钮
            let flag = |bold: bool, active: bool, id: gpui::SharedString| -> gpui::AnyElement {
                div()
                    .id(id)
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .bg(if active { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if active { pal.accent_dark } else { pal.muted }))
                    .child(if bold { "B" } else { "U" })
                    .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                        let c = cat;
                        this.edit_rule(c, |r| {
                            if bold {
                                r.bold = !r.bold;
                            } else {
                                r.underline = !r.underline;
                            }
                        }, cx);
                    }))
                    .into_any_element()
            };
            rules = rules.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(24.))
                    .child(toggle)
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(rgb(if rule.enabled { pal.text } else { pal.dim }))
                            .child(cat_label),
                    )
                    .child(div().flex_1().min_w_0().child(swatches))
                    .child(flag(true, rule.bold, gpui::SharedString::from(format!("rule-{cat}-b"))))
                    .child(flag(false, rule.underline, gpui::SharedString::from(format!("rule-{cat}-u")))),
            );
        }

        // ----- 字体（全局设置，不随模板） -----
        let mut font_chips = div().flex().flex_wrap().gap(px(4.));
        for fam in self.font_choices.clone() {
            let selected = fam == self.font_family;
            let label = fam.clone();
            font_chips = font_chips.child(
                div()
                    .id(gpui::SharedString::from(format!("font-fam-{fam}")))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(pal.border)))
                    .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                        this.db.set_setting("terminal.font.family", &fam);
                        this.font_family = fam.clone();
                        cx.emit(SettingsEvent::TerminalAppearance);
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        let mut size_chips = div().flex().flex_wrap().gap(px(4.));
        for size in [10., 11., 12., 13., 14., 15., 16., 18., 20., 24.] {
            let selected = (self.font_size - size).abs() < 0.5;
            size_chips = size_chips.child(
                div()
                    .id(gpui::SharedString::from(format!("font-size-{size}")))
                    .h(px(24.))
                    .w(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.card) })
                    .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                    .hover(|s| s.bg(rgb(pal.border)))
                    .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                        this.db.set_setting("terminal.font.size", &format!("{size}"));
                        this.font_size = size;
                        cx.emit(SettingsEvent::TerminalAppearance);
                        cx.notify();
                    }))
                    .child(format!("{}", size as i32)),
            );
        }

        // ----- 详情页内容（按导航选择） -----
        let body = match self.page {
            Page::System => self.system_page(&pal, cx),
            Page::Console => div()
                .id("settings-body")
                .flex()
                .flex_col()
                .gap_4()
                .w_full()
                .min_h(gpui::relative(1.))
                .p_6()
                .text_size(px(13.))
                .child(Self::section_label(crate::i18n::tl(loc, crate::i18n::Tr::SecFont), &pal))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(44.))
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(rgb(pal.muted))
                                .child(crate::i18n::tl(loc, crate::i18n::Tr::SecFont)),
                        )
                        .child(font_chips),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(44.))
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(rgb(pal.muted))
                                .child(crate::i18n::tl(loc, crate::i18n::Tr::SecFontSize)),
                        )
                        .child(size_chips),
                )
                .child(div().h(px(1.)).bg(rgb(pal.divider())))
                .child(Self::section_label(crate::i18n::tl(loc, crate::i18n::Tr::SecTermPalette), &pal))
                .child(palettes)
                .child(div().h(px(1.)).bg(rgb(pal.divider())))
                .child(Self::section_label(crate::i18n::tl(loc, crate::i18n::Tr::SecStyleTemplates), &pal))
                .child(chips)
                .child(actions)
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(pal.dim))
                        .child(crate::i18n::tl(loc, crate::i18n::Tr::TplHint)),
                )
                .child(rules),
        };

        // ----- 左侧导航树（分组 + 条目；文案按当前语言取） -----
        let mut nav = div().flex().flex_col().gap(px(1.));
        for (gi, (gname, items)) in NAV.iter().enumerate() {
            nav = nav.child(
                div()
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(rgb(pal.muted))
                    .child(crate::i18n::tl(loc, *gname)),
            );
            for (ii, item) in items.iter().enumerate() {
                let Some(page) = page_of(gi, ii) else { continue };
                let selected = self.page == page;
                nav = nav.child(
                    div()
                        .id(gpui::SharedString::from(format!("nav-{gi}-{ii}")))
                        .h(px(28.))
                        .pl(px(24.))
                        .pr(px(8.))
                        .mx(px(4.))
                        .flex()
                        .items_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .bg(if selected { rgb(pal.accent) } else { pal.surface(pal.bg) })
                        .text_color(rgb(if selected { pal.accent_dark } else { pal.text }))
                        .hover(|s| s.bg(pal.surface(pal.card)))
                        .on_click(cx.listener(move |this: &mut SettingsView, _ev, _w, cx| {
                            this.page = page;
                            cx.notify();
                        }))
                        .child(crate::i18n::tl(loc, *item)),
                );
            }
        }

        // ----- 右侧详情区（面包屑 + 滚动内容；面包屑随选中页与语言） -----
        let (crumb_group, crumb_item) = match self.page {
            Page::System => (crate::i18n::Tr::NavGeneral, crate::i18n::Tr::NavSystem),
            Page::Console => (crate::i18n::Tr::NavAppearance, crate::i18n::Tr::NavConsole),
        };
        let right = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .text_size(px(12.))
                    .text_color(rgb(pal.muted))
                    .child(format!(
                        "{} › {}",
                        crate::i18n::tl(loc, crumb_group),
                        crate::i18n::tl(loc, crumb_item)
                    )),
            )
            .child(div().h(px(1.)).flex_none().bg(rgb(pal.divider())))
            .child(scroll_area::ScrollArea::new(self.scroll.clone(), body).theme(kit));

        div()
            .flex()
            .flex_row()
            // 定高（表单窗同款）：根必须 size_full，右列/ScrollArea 才能拿到
            // 有界高度形成溢出滚动；min_h 会被内容自然高撑开导致裁切不滚。
            .size_full()
            .bg(pal.surface(pal.bg))
            .text_size(px(13.))
            .text_color(rgb(pal.text))
            .font_family("System UI")
            .child(
                div()
                    .w(px(180.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .pt(px(8.))
                    .border_r_1()
                    .border_color(rgb(pal.divider()))
                    .child(nav),
            )
            .child(right)
    }
}

/// 打开设置窗口；ui 为主窗口实体（外观变化事件回推刷新已开终端）。
/// 窗体主题跟随 app 当前主题。
pub fn open_settings(ui: &Entity<crate::app_ui::AppUi>, cx: &mut App) {
    let spec = dialog::DialogSpec {
        title: crate::i18n::t(cx, crate::i18n::Tr::SettingsTitle).into(),
        width: 780.,
        height: 700.,
        min_width: 640.,
        min_height: 520.,
        surface_alpha: Some(theme::surface_fx(cx).alpha),
        surface_blur: theme::bg_fx(cx).blur,
        ..Default::default()
    };
    let view = cx.new(|cx| SettingsView::new(cx));
    let ui = ui.clone();
    dialog::open_dialog_ex::<SettingsView, SettingsEvent>(
        view,
        spec,
        None,
        crate::theme::current(cx).kit(),
        Some(std::rc::Rc::new(move |ev, _closer, app| {
            let _ = ui.update(app, |ui, cx| match ev {
                SettingsEvent::TerminalAppearance => ui.reload_terminal_appearance(cx),
                SettingsEvent::UiTheme(idx) => ui.set_ui_theme(*idx, cx),
                SettingsEvent::SearchEngine(e) => ui.set_search_engine(*e, cx),
            });
        })),
        cx,
    );
    // 开窗建立的模糊视图需要按当前主题系列 tint（幂等）。
    ui_kit::platform::set_blur_tint(!theme::is_light(cx));
}
