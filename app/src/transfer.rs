//! 目录面板·文件传输区（SFTP）：状态模型 + 传输区渲染 + 入队链路。
//!
//! 传输走 core 的 [`sshrust_core::Session::enqueue_transfer`]（同一 SSH 连接的
//! sftp 子系统通道，队列串行）；本模块持有每 tab 的任务列表
//! （[`AppUi::transfer_data`]），进度经 [`TransferTicket`] 快照轮询，
//! 速度按 1s ticker 相邻采样差分（与系统信息面板同法）。
//!
//! 入口：目录树右键（文件=下载 / 目录或空白=上传到该目录）+ 传输区标题行
//! 上传按钮（当前 cwd）。上传前用 `sftp_exists` 批量查冲突，有则弹一次
//! 覆盖确认（复用删除确认的 modal 模式）；下载侧系统保存面板自带覆盖确认。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use gpui::{prelude::*, px, relative, rgb, AnyElement, Context, div};

use sshrust_core::{TransferDirection, TransferRequest, TransferStatus, TransferTicket};

use crate::app_ui::AppUi;
use crate::icons;
use crate::theme::{fmt_bytes, PalFx};

/// 下载保存目录记忆键（上次保存位置，保存面板默认目录）。
pub(crate) const SETTING_SAVE_DIR: &str = "transfer.save.dir";

/// 一条传输任务：票据 + UI 侧速度差分样本与完成回执标记。
pub(crate) struct TransferItem {
    pub ticket: TransferTicket,
    /// 上次速度采样（字节, 时刻）。
    last: (u64, Instant),
    /// 显示速度（字节/秒；None = 未在跑或首拍未到）。
    pub speed: Option<f64>,
    /// 完成回执是否已处理（上传完成 → 刷新目录树，只做一次）。
    settled: bool,
}

impl TransferItem {
    fn new(ticket: TransferTicket) -> Self {
        Self { ticket, last: (0, Instant::now()), speed: None, settled: false }
    }
}

/// 上传覆盖确认模态（目标目录已有同名文件）。
pub(crate) struct ConfirmOverwrite {
    pub modal: ui_kit::base::modal::ModalHandle,
    /// 待上传本地文件（冲突与不冲突一并继续）。
    pub files: Vec<PathBuf>,
    /// 远端目标目录（绝对路径）。
    pub remote_dir: String,
    /// 冲突文件数（正文展示）。
    pub conflicts: usize,
    /// 所属 tab（入队定位会话）。
    pub tab_id: u64,
}

impl AppUi {
    // ---------- ticker（1s）：速度差分 + 完成回执 ----------

    /// 速度差分（相邻采样）+ 上传完成 → 目标目录缓存失效并补取（目录树见新文件）。
    pub(crate) fn tick_transfers(&mut self, cx: &mut Context<Self>) {
        let mut refresh_dir = false;
        let tab_ids: Vec<u64> = self.transfer_data.keys().copied().collect();
        for tab_id in tab_ids {
            let Some(items) = self.transfer_data.get_mut(&tab_id) else { continue };
            let mut touched = false;
            for it in items.iter_mut() {
                let s = it.ticket.snapshot();
                if matches!(s.status, TransferStatus::Running) {
                    let now = Instant::now();
                    let dt = now.duration_since(it.last.1).as_secs_f64();
                    if dt >= 0.9 {
                        it.speed = Some((s.done.saturating_sub(it.last.0)) as f64 / dt);
                        it.last = (s.done, now);
                    }
                } else {
                    it.speed = None;
                }
                // 完成回执：上传 → 目标目录缓存失效（该 tab 的目录树补取新清单）。
                if !it.settled && it.ticket.is_finished() {
                    it.settled = true;
                    if s.direction == TransferDirection::Upload {
                        let parent = parent_dir(&s.remote);
                        if let Some(st) = self.dir_data.get_mut(&tab_id) {
                            st.cache.remove(&parent);
                        }
                        touched = true;
                    }
                }
            }
            refresh_dir |= touched;
        }
        if refresh_dir {
            self.maybe_fetch_dir(cx);
            self.refresh_dir_tree(cx);
        }
    }

    // ---------- 入队链路 ----------

    /// 上传入口：rfd 多选面板（**异步 sheet 模式**——同步版在主线程跑嵌套
    /// runModal 会重入 gpui 事件循环直接闪退）→ 批量查远端同名冲突 →
    /// 无冲突直接入队，有冲突弹覆盖确认。
    pub(crate) fn start_upload(&mut self, remote_dir: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.active_live_tab() else { return };
        let tab_id = tab.id;
        let Some(session) = self.live_session(tab_id) else { return };
        let title = crate::i18n::tl(self.loc, crate::i18n::Tr::MenuUploadHere).to_string();
        let remember = self.db.get_setting(SETTING_SAVE_DIR).filter(|d| !d.is_empty());
        let remote_dir = remote_dir.to_string();
        cx.spawn(async move |this: gpui::WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
            let mut dlg = rfd::AsyncFileDialog::new().set_title(title);
            if let Some(dir) = remember {
                dlg = dlg.set_directory(dir);
            }
            let picked = dlg.pick_files().await;
            let files: Vec<PathBuf> = picked
                .map(|fs| fs.into_iter().map(|f| f.path().to_path_buf()).collect())
                .unwrap_or_default();
            if files.is_empty() {
                return;
            }
            let _ = this.update(cx, |ui, cx| {
                ui.upload_files_to(tab_id, session, files, &remote_dir, cx);
            });
        })
        .detach();
    }

    /// 已选定的本地文件 → 上传到当前 cwd（拖放路径共用；含冲突检查）。
    pub(crate) fn upload_picked_files(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(tab) = self.active_live_tab() else { return };
        let tab_id = tab.id;
        let Some(session) = self.live_session(tab_id) else { return };
        let Some(remote_dir) =
            self.dir_data.get(&tab_id).and_then(|st| st.cwd.clone())
        else {
            self.show_notice(crate::i18n::t(cx, crate::i18n::Tr::DirNoInfo).to_string(), cx);
            return;
        };
        self.upload_files_to(tab_id, session, files, &remote_dir, cx);
    }

    /// 上传一组文件到指定远端目录：异步批量查同名冲突 → 弹覆盖确认或直接入队。
    fn upload_files_to(
        &mut self,
        tab_id: u64,
        session: sshrust_core::Session,
        files: Vec<PathBuf>,
        remote_dir: &str,
        cx: &mut Context<Self>,
    ) {
        let remote_dir = remote_dir.to_string();
        cx.spawn(async move |this: gpui::WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
            // 批量查冲突（顺序查，SFTP 单通道）。
            let mut conflicts = 0usize;
            for f in &files {
                let remote = format!("{}/{}", remote_dir.trim_end_matches('/'), file_name_of(f));
                let session = session.clone();
                let ran = crate::RUNTIME
                    .get()
                    .expect("tokio runtime")
                    .spawn(async move { session.sftp_exists(&remote).await })
                    .await;
                match ran {
                    Ok(Ok(true)) => conflicts += 1,
                    // 查询失败不拦上传（服务器不支持 sftp 时由传输自身报错更直观）。
                    _ => {}
                }
            }
            let _ = this.update(cx, |ui, cx| {
                if conflicts > 0 {
                    ui.confirm_overwrite = Some(ConfirmOverwrite {
                        modal: ui_kit::base::modal::ModalHandle::new(cx),
                        files,
                        remote_dir,
                        conflicts,
                        tab_id,
                    });
                } else {
                    ui.enqueue_uploads(tab_id, files, &remote_dir, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 下载入口：rfd 保存面板（异步 sheet 模式；预填文件名、记忆上次目录）→ 入队。
    pub(crate) fn start_download(&mut self, remote_path: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.active_live_tab() else { return };
        let tab_id = tab.id;
        let name = remote_path.rsplit('/').next().unwrap_or("file").to_string();
        let title = crate::i18n::tl(self.loc, crate::i18n::Tr::MenuDownload).to_string();
        let remember = self.db.get_setting(SETTING_SAVE_DIR).filter(|d| !d.is_empty());
        let Some(session) = self.live_session(tab_id) else { return };
        let remote = remote_path.to_string();
        cx.spawn(async move |this: gpui::WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
            let mut dlg = rfd::AsyncFileDialog::new().set_title(title).set_file_name(&name);
            if let Some(dir) = remember {
                dlg = dlg.set_directory(dir);
            }
            let Some(handle) = dlg.save_file().await else { return };
            let local = handle.path().to_path_buf();
            let parent = local.parent().map(|p| p.display().to_string());
            let ran = crate::RUNTIME
                .get()
                .expect("tokio runtime")
                .spawn(async move { session.enqueue_transfer(req_of(local, remote)).await })
                .await;
            let _ = this.update(cx, |ui, cx| {
                if let Some(dir) = parent {
                    if !dir.is_empty() {
                        ui.db.set_setting(SETTING_SAVE_DIR, &dir);
                    }
                }
                match ran {
                    Ok(Ok(ticket)) => {
                        ui.transfer_data.entry(tab_id).or_default().push(TransferItem::new(ticket));
                    }
                    Ok(Err(e)) => ui.show_notice(e, cx),
                    Err(_) => ui.show_notice("background task failed".into(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 逐个文件入队上传（覆盖确认通过 / 无冲突路径共用）。
    pub(crate) fn enqueue_uploads(
        &mut self,
        tab_id: u64,
        files: Vec<PathBuf>,
        remote_dir: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.live_session(tab_id) else { return };
        let remote_dir = remote_dir.trim_end_matches('/').to_string();
        for f in files {
            let remote = format!("{}/{}", remote_dir, file_name_of(&f));
            let session = session.clone();
            cx.spawn(async move |this: gpui::WeakEntity<AppUi>, cx: &mut gpui::AsyncApp| {
                let req = TransferRequest {
                    direction: TransferDirection::Upload,
                    local: f,
                    remote,
                };
                let ran = crate::RUNTIME
                    .get()
                    .expect("tokio runtime")
                    .spawn(async move { session.enqueue_transfer(req).await })
                    .await;
                let _ = this.update(cx, |ui, cx| {
                    match ran {
                        Ok(Ok(ticket)) => {
                            ui.transfer_data.entry(tab_id).or_default().push(TransferItem::new(ticket));
                        }
                        Ok(Err(e)) => ui.show_notice(e, cx),
                        Err(_) => ui.show_notice("background task failed".into(), cx),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    // ---------- 渲染 ----------

    /// 覆盖确认 modal（复用删除确认形态）。
    pub(crate) fn render_overwrite_modal(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let c = self.confirm_overwrite.as_ref()?;
        let kit = self.theme().kit();
        let body = div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(self.theme().muted))
                    .child(crate::i18n::tf(
                        cx,
                        crate::i18n::Tr::OverwriteConfirmBody,
                        &[&c.conflicts.to_string()],
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .justify_end()
                    .child(ui_kit::base::button::button(
                        "overwrite-cancel",
                        crate::i18n::t(cx, crate::i18n::Tr::Cancel),
                        ui_kit::base::button::ButtonKind::Secondary,
                        ui_kit::base::button::ButtonSize::Md,
                        true,
                        kit,
                        cx,
                        |this, _ev, _window, cx| {
                            this.confirm_overwrite = None;
                            cx.notify();
                        },
                    ))
                    .child(ui_kit::base::button::button(
                        "overwrite-ok",
                        crate::i18n::t(cx, crate::i18n::Tr::OverwriteConfirmOk),
                        ui_kit::base::button::ButtonKind::Danger,
                        ui_kit::base::button::ButtonSize::Md,
                        true,
                        kit,
                        cx,
                        |this, _ev, _window, cx| {
                            if let Some(c) = this.confirm_overwrite.take() {
                                let (files, dir, tab_id) = (c.files, c.remote_dir, c.tab_id);
                                this.enqueue_uploads(tab_id, files, &dir, cx);
                            }
                            cx.notify();
                        },
                    )),
            );
        Some(ui_kit::base::modal::overlay(
            &c.modal,
            crate::i18n::t(cx, crate::i18n::Tr::OverwriteConfirmTitle),
            360.,
            body.into_any_element(),
            kit,
            cx,
            |this, _window, cx| {
                this.confirm_overwrite = None;
                cx.notify();
            },
        ))
    }

    /// 传输区（钉底）：标题行（上传按钮 + 清空已完成）+ 任务列表。
    pub(crate) fn render_dir_transfer(&self, transfer_h: f32, cx: &Context<Self>) -> AnyElement {
        let pal = self.theme();
        let kit = pal.kit();
        let tab_id = self.active_live_tab().map(|t| t.id);
        let items = tab_id.and_then(|id| self.transfer_data.get(&id));
        let has_finished = items.is_some_and(|v| v.iter().any(|it| it.ticket.is_finished()));

        let mut head = div()
            .id("dir-transfer-head")
            .flex()
            .items_center()
            .gap_1()
            .h(px(crate::dir_panel::DIR_TITLE_H))
            .px_3()
            .flex_none()
            .text_size(px(11.))
            .text_color(rgb(pal.dim))
            .child(div().mr_1().child(crate::app_ui::svg_icon(icons::UPLOAD, 12., pal.dim)))
            .child(crate::i18n::t(cx, crate::i18n::Tr::TransferTitle))
            .child(div().flex_1());
        if has_finished {
            head = head.child(
                div()
                    .id("dir-transfer-clear")
                    .flex()
                    .items_center()
                    .mr_1()
                    .text_size(px(11.))
                    .text_color(rgb(pal.dim))
                    .hover(|s| s.text_color(rgb(pal.text)))
                    .child(crate::i18n::t(cx, crate::i18n::Tr::TransferClearFinished))
                    .on_click(cx.listener(|this, _ev: &gpui::ClickEvent, _w, cx| {
                        let Some(tab) = this.active_live_tab() else { return };
                        let id = tab.id;
                        if let Some(v) = this.transfer_data.get_mut(&id) {
                            v.retain(|it| !it.ticket.is_finished());
                        }
                        cx.notify();
                    })),
            );
        }
        head = head.child(
            div()
                .id("dir-transfer-upload")
                .flex()
                .items_center()
                .justify_center()
                .size(px(20.))
                .rounded_md()
                .text_size(px(12.))
                .hover(|s| s.bg(pal.surface(pal.card)))
                .child(crate::app_ui::svg_icon(icons::UPLOAD, 12., pal.dim))
                .tooltip(move |_, cx| {
                    ui_kit::base::tooltip::tip(
                        crate::i18n::t(cx, crate::i18n::Tr::TipUploadHere),
                        kit,
                        cx,
                    )
                })
                .on_click(cx.listener(|this, _ev: &gpui::ClickEvent, _w, cx| {
                    // 上传到当前 cwd（无 cwd 时不可用）。
                    let dir = this
                        .active_live_tab()
                        .and_then(|t| this.dir_data.get(&t.id))
                        .and_then(|st| st.cwd.clone());
                    if let Some(dir) = dir {
                        this.start_upload(&dir, cx);
                    }
                })),
        );

        let body = if items.is_none_or(|v| v.is_empty()) {
            // 空态。
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px_3()
                        .text_size(px(11.))
                        .text_color(rgb(pal.dim))
                        .child(crate::i18n::t(cx, crate::i18n::Tr::TransferNoTasks)),
                )
                .into_any_element()
        } else {
            ui_kit::pro::scroll_area::ScrollArea::new(
                self.transfer_scroll.clone(),
                div()
                    .id("dir-transfer-rows")
                    .w_full()
                    .flex()
                    .flex_col()
                    .py_1()
                    .children(
                        items
                            .unwrap()
                            .iter()
                            .enumerate()
                            .map(|(i, it)| self.transfer_row(i, it, &pal, cx)),
                    ),
            )
            .theme(pal.kit())
            .into_any_element()
        };

        div()
            .id("dir-transfer")
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(transfer_h))
            .flex_col()
            .overflow_hidden()
            .border_t_1()
            .border_color(rgb(pal.divider()))
            // 外部文件拖放到传输区 = 上传到当前 cwd（gpui 把系统拖放翻译为
            // ExternalPaths 内部拖拽，Submit 即 MouseUp 落到本区）。
            .drag_over::<gpui::ExternalPaths>(|s, _paths, _w, _cx| s.bg(gpui::rgba(0x7aa2f71f)))
            .on_drop::<gpui::ExternalPaths>(cx.listener(
                |this, paths: &gpui::ExternalPaths, _w, cx| {
                    let files: Vec<PathBuf> = paths.paths().to_vec();
                    if !files.is_empty() {
                        this.upload_picked_files(files, cx);
                    }
                },
            ))
            .child(head)
            .child(body)
            .into_any_element()
    }

    /// 单条传输行：方向图标 + 文件名 + 状态/字节/速度 + 进度条 + 动作钮。
    fn transfer_row(&self, i: usize, it: &TransferItem, pal: &PalFx, cx: &Context<Self>) -> AnyElement {
        let s = it.ticket.snapshot();
        let ticket_id = it.ticket.id();
        let active = matches!(s.status, TransferStatus::Queued | TransferStatus::Running);
        let status_el: AnyElement = match &s.status {
            TransferStatus::Queued => div()
                .text_color(rgb(pal.dim))
                .child(crate::i18n::t(cx, crate::i18n::Tr::TransferQueued))
                .into_any_element(),
            TransferStatus::Running => {
                let mut txt = format!("{} / {}", fmt_bytes(s.done), fmt_bytes(s.total));
                if let Some(v) = it.speed {
                    txt.push_str(&crate::i18n::tf(
                        cx,
                        crate::i18n::Tr::RateSuffix,
                        &[&fmt_bytes(v as u64)],
                    ));
                }
                div().text_color(rgb(pal.muted)).child(txt).into_any_element()
            }
            TransferStatus::Done => div()
                .flex()
                .items_center()
                .gap_1()
                .text_color(rgb(pal.ok))
                .child(crate::app_ui::svg_icon(icons::CHECK, 11., pal.ok))
                .child(crate::i18n::t(cx, crate::i18n::Tr::TransferDone))
                .into_any_element(),
            TransferStatus::Canceled => div()
                .text_color(rgb(pal.dim))
                .child(crate::i18n::t(cx, crate::i18n::Tr::TransferCanceled))
                .into_any_element(),
            TransferStatus::Error(msg) => {
                let shown: String = msg.chars().take(48).collect();
                div()
                    .text_color(rgb(pal.error))
                    .child(format!("{}: {}", crate::i18n::t(cx, crate::i18n::Tr::TransferFailed), shown))
                    .into_any_element()
            }
        };
        let pct = if s.total > 0 { s.done as f32 / s.total as f32 } else { 0. };
        let fill_color = if matches!(s.status, TransferStatus::Error(_)) { pal.error } else { pal.accent };
        div()
            .id(("dir-transfer-row", i))
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_1()
            .flex_none()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(crate::app_ui::svg_icon(
                        if s.direction == TransferDirection::Upload {
                            icons::UPLOAD
                        } else {
                            icons::DOWNLOAD
                        },
                        12.,
                        pal.muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(11.))
                            .text_color(rgb(pal.text))
                            .text_ellipsis()
                            .child(s.file_name.clone()),
                    )
                    .child(status_el)
                    .child(
                        div()
                            .id(("dir-transfer-act", i))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.))
                            .rounded_md()
                            .text_size(px(12.))
                            .text_color(rgb(pal.dim))
                            .hover(|st| st.bg(pal.surface(pal.card)))
                            .child("×")
                            .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                                // 跑动中=请求取消；已结束=移出行。
                                if active {
                                    if let Some(tab) = this.active_live_tab() {
                                        let id = tab.id;
                                        if let Some(v) = this.transfer_data.get_mut(&id) {
                                            if let Some(it) = v.iter_mut().find(|it| it.ticket.id() == ticket_id) {
                                                it.ticket.cancel();
                                            }
                                        }
                                    }
                                } else if let Some(tab) = this.active_live_tab() {
                                    let id = tab.id;
                                    if let Some(v) = this.transfer_data.get_mut(&id) {
                                        v.retain(|it| it.ticket.id() != ticket_id);
                                    }
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(rgb(pal.border))
                    .child(div().h_full().rounded_full().bg(rgb(fill_color)).w(relative(pct))),
            )
            .into_any_element()
    }
}

/// 本地文件名（列表展示与远端拼接）。
fn file_name_of(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// 下载请求构造。
fn req_of(local: PathBuf, remote: String) -> TransferRequest {
    TransferRequest { direction: TransferDirection::Download, local, remote }
}

/// 远端路径的父目录（缓存失效定位；根目录的父仍是根）。
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => path.to_string(),
    }
}

/// 入队错误 → 传输行级可见（不在此弹通知）：仅保留辅助函数供测试。
#[allow(dead_code)]
fn transfer_counts(data: &HashMap<u64, Vec<TransferItem>>) -> (usize, usize) {
    let total: usize = data.values().map(|v| v.len()).sum();
    let finished = data
        .values()
        .map(|v| v.iter().filter(|it| it.ticket.is_finished()).count())
        .sum();
    (total, finished)
}
