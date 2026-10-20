#!/usr/bin/env python3
"""生成 SSHRust 应用图标母版（1024×1024 PNG）。

设计（2026-10-20 定稿：机器人吉祥物）：
- 深空渐变 squircle 背景（对应 app 默认 Tokyo-Night 深色系）
- 居中机器人吉祥物（用户选定原图白底抠除，scripts/icon-robot.png）
- 底部软阴影增加落感

被 scripts/package.sh 调用；也可独立运行：make_icon.py [输出路径]。
"""
import pathlib
import sys

from PIL import Image, ImageDraw, ImageFilter

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

    # —— 机器人吉祥物（scripts/icon-robot.png：白底原图已抠底）——
    robot = Image.open(pathlib.Path(__file__).with_name("icon-robot.png")).convert('RGBA')
    # 目标高 ≈ 画布 68%，等比缩放
    target_h = 700
    scale = target_h / robot.height
    robot = robot.resize((int(robot.width * scale), target_h), Image.LANCZOS)
    # 底部软阴影（黑色椭圆模糊，增加落感）
    shadow = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    sw = int(robot.width * 0.46)
    sd.ellipse([512 - sw, 812 - 44, 512 + sw, 812 + 44], fill=(0, 0, 0, 110))
    shadow = shadow.filter(ImageFilter.GaussianBlur(22))
    img.alpha_composite(shadow)
    # 居中贴入（视觉重心略上提 6px）
    img.alpha_composite(robot, (512 - robot.width // 2, 512 - robot.height // 2 - 6))

    img.save(OUT)
    print(OUT)


if __name__ == "__main__":
    main()
