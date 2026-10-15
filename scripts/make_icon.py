#!/usr/bin/env python3
"""生成 SSHRust 应用图标母版（1024×1024 PNG）。

深空渐变 squircle 背景（对应 app 默认 Tokyo-Night 深色系）+ 白色终端提示符 ">_"。
被 scripts/package.sh 调用；也可独立运行：make_icon.py [输出路径]。
"""
import sys

from PIL import Image, ImageDraw

S = 1024
OUT = sys.argv[1] if len(sys.argv) > 1 else "target/iconset/master.png"


def main() -> None:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # 深空渐变背景：#1a1b26 → #24283b
    top, bot = (26, 27, 38), (36, 40, 59)
    grad = Image.new("RGBA", (S, S))
    gd = ImageDraw.Draw(grad)
    for y in range(S):
        t = y / (S - 1)
        c = tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,)
        gd.line([(0, y), (S, y)], fill=c)

    # macOS 图标圆角画布（squircle 近似：圆角 22.5%）
    pad = int(S * 0.02)
    radius = int(S * 0.225)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [pad, pad, S - pad, S - pad], radius=radius, fill=255
    )
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    d.rounded_rectangle(
        [pad, pad, S - pad, S - pad], radius=radius, outline=(255, 255, 255, 28), width=3
    )

    # 终端提示符 ">_"（白，圆头线，几何贴近 lucide terminal）
    fg = (235, 239, 255, 255)
    lw = 66

    def line(p1: tuple[int, int], p2: tuple[int, int]) -> None:
        d.line([p1, p2], fill=fg, width=lw)
        r0 = lw // 2
        for (x, y) in (p1, p2):
            d.ellipse([x - r0, y - r0, x + r0, y + r0], fill=fg)

    cx, cy = S // 2, S // 2
    line((cx - 200, cy - 170), (cx - 40, cy))
    line((cx - 40, cy), (cx - 200, cy + 170))
    line((cx + 40, cy + 170), (cx + 260, cy + 170))

    img.save(OUT)
    print(OUT)


if __name__ == "__main__":
    main()
