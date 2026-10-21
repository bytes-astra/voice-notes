"""生成 VoiceNotes 应用图标源图（1024×1024）。"""
from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
SCALE = 4  # 超采样后缩小，边缘更干净

img = Image.new("RGBA", (SIZE * SCALE, SIZE * SCALE), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)
s = SIZE * SCALE

# 圆角方形底：靛蓝→紫的斜向渐变
radius = int(s * 0.235)
margin = int(s * 0.055)
left, top = margin, margin
right, bottom = s - margin, s - margin

gradient = Image.new("RGBA", (right - left, bottom - top))
pixels = gradient.load()
for y in range(bottom - top):
    for x in range(right - left):
        t = (x / (right - left) + y / (bottom - top)) / 2
        r = int(91 + (139 - 91) * t)
        g = int(91 + (92 - 91) * t)
        b = int(214 + (246 - 214) * t)
        pixels[x, y] = (r, g, b, 255)

mask = Image.new("L", gradient.size, 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, gradient.size[0] - 1, gradient.size[1] - 1], radius=radius, fill=255)
img.paste(gradient, (left, top), mask)

# 顶部高光
glow = Image.new("RGBA", img.size, (0, 0, 0, 0))
ImageDraw.Draw(glow).ellipse(
    [int(s * 0.12), int(-s * 0.22), int(s * 0.88), int(s * 0.42)],
    fill=(255, 255, 255, 46),
)
glow = glow.filter(ImageFilter.GaussianBlur(int(s * 0.05)))
full_mask = Image.new("L", img.size, 0)
ImageDraw.Draw(full_mask).rounded_rectangle([left, top, right, bottom], radius=radius, fill=255)
img = Image.alpha_composite(img, Image.composite(glow, Image.new("RGBA", img.size, (0, 0, 0, 0)), full_mask))

draw = ImageDraw.Draw(img)
white = (255, 255, 255, 252)

# 麦克风胶囊
cap_w, cap_h = int(s * 0.175), int(s * 0.30)
cx, cy = s // 2, int(s * 0.435)
draw.rounded_rectangle(
    [cx - cap_w // 2, cy - cap_h // 2, cx + cap_w // 2, cy + cap_h // 2],
    radius=cap_w // 2,
    fill=white,
)

# 麦克风托架
arc_w = int(s * 0.052)
draw.arc(
    [cx - int(s * 0.175), cy - int(s * 0.10), cx + int(s * 0.175), cy + int(s * 0.25)],
    start=0,
    end=180,
    fill=white,
    width=arc_w,
)
# 支架竖杆与底座
draw.rounded_rectangle(
    [cx - arc_w // 2, cy + int(s * 0.24), cx + arc_w // 2, cy + int(s * 0.325)],
    radius=arc_w // 2,
    fill=white,
)
draw.rounded_rectangle(
    [cx - int(s * 0.10), cy + int(s * 0.315), cx + int(s * 0.10), cy + int(s * 0.315) + arc_w],
    radius=arc_w // 2,
    fill=white,
)

img = img.resize((SIZE, SIZE), Image.LANCZOS)
img.save("src-tauri/icon-source.png")
print("wrote src-tauri/icon-source.png")
