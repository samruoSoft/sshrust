//! 树行对齐回归测试：展开箭头画在缩进区内、不占行内容空间。
//! 复现用户报告（目录面板）：同层文件（叶，无箭头）与目录（分组，有箭头）
//! 图标没对齐，且首层文件图标比父级还靠左——根因是箭头在内容流里占 16px，
//! 而目录面板步长只有 12px。

use gpui::{div, prelude::*, px, IntoElement, ParentElement, Render, Styled, TestAppContext, Window};

use ui_kit::pro::tree::{TreeState, TreeNodeData};

struct Harness {
    tree: gpui::Entity<TreeState>,
}

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let _ = cx;
        div().id("root").w(px(240.)).h(px(300.)).child(self.tree.clone())
    }
}

fn fixture() -> Vec<TreeNodeData> {
    // root(d0, 分组)
    // ├─ file(d1, 叶)   ← 原 bug：无箭头，图标比分组左 16px
    // ├─ dir(d1, 分组)  ← 有箭头
    // │  └─ inner(d2, 叶)
    let mut dir = TreeNodeData::group(3, "dir");
    dir.children = vec![TreeNodeData::leaf(4, "inner")];
    dir.expanded = true;
    let mut root = TreeNodeData::group(1, "root");
    root.children = vec![TreeNodeData::leaf(2, "file"), dir];
    root.expanded = true;
    vec![root]
}

#[gpui::test]
fn gutter_arrow_keeps_leaf_and_group_aligned(cx: &mut TestAppContext) {
    let (_harness, cx) = cx.add_window_view(|_w, cx| Harness {
        tree: cx.new(|cx| {
            let mut t = TreeState::new(cx);
            // 目录面板同款参数：行高 24、步长 12（< 箭头+间距 16，原 bug 触发条件）
            t.set_metrics(24., 12., cx);
            t.set_nodes(fixture(), cx);
            t
        }),
    });
    cx.run_until_parked();

    fn left(cx: &mut gpui::VisualTestContext, sel: &'static str) -> f32 {
        let b = cx.debug_bounds(sel).unwrap_or_else(|| panic!("{sel} bounds missing"));
        b.origin.x.into()
    }

    // 同层（d1）叶与分组：内容起点一致——原 bug 里叶比分组左 16px。
    assert_eq!(left(cx, "tree-label-2"), left(cx, "tree-label-3"));
    // 层级步长精确：每深一级右移一个 indent，子级必在父级右侧。
    let l0 = left(cx, "tree-label-1");
    let l1 = left(cx, "tree-label-2");
    let l2 = left(cx, "tree-label-4");
    assert_eq!(l1 - l0, 12.);
    assert_eq!(l2 - l1, 12.);
    // 箭头留在缩进区内，不入侵内容起点。
    let arrow = cx.debug_bounds("tree-arrow-3").expect("arrow bounds");
    let label3 = cx.debug_bounds("tree-label-3").unwrap();
    assert!(arrow.origin.x + arrow.size.width <= label3.origin.x);
}
