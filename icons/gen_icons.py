#!/usr/bin/env python3
"""Генерирует иконку Лама Чат: скруглённый квадрат с градиентом и буквой Л."""
from PIL import Image, ImageDraw, ImageFont
import os

def make(size):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # Скруглённый квадрат (squircle-подобный радиус)
    r = int(size * 0.22)
    d.rounded_rectangle([0, 0, size - 1, size - 1], radius=r, fill=(124, 108, 255, 255))

    # Градиент: накладываем полупрозрачные слои
    grad = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    top = (124, 108, 255)   # фиолетовый
    bot = (157, 139, 255)   # светло-сиреневый
    for y in range(size):
        t = y / size
        col = tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3)) + (255,)
        gd.line([(0, y), (size, y)], fill=col)
    mask = Image.new("L", (size, size), 0)
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([0, 0, size - 1, size - 1], radius=r, fill=255)
    img.paste(grad, (0, 0), mask)

    # Буква Л
    d = ImageDraw.Draw(img)
    letter = "Л"
    font_size = int(size * 0.62)
    font = None
    for path in [
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/SFNSText.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSDisplay.ttf",
    ]:
        if os.path.exists(path):
            try:
                font = ImageFont.truetype(path, font_size)
                if hasattr(font, "getbbox") and font.getbbox(letter)[3] > 0:
                    break
            except Exception:
                continue
    if font is None:
        font = ImageFont.load_default()

    bbox = d.textbbox((0, 0), letter, font=font)
    w = bbox[2] - bbox[0]
    h = bbox[3] - bbox[1]
    x = (size - w) / 2 - bbox[0]
    y = (size - h) / 2 - bbox[1]
    d.text((x, y), letter, font=font, fill=(255, 255, 255, 255))

    return img

os.makedirs("icons", exist_ok=True)
make(32).save("icons/32x32.png")
make(128).save("icons/128x128.png")
make(256).save("icons/128x128@2x.png")
make(512).save("icons/icon_512.png")
make(1024).save("icons/icon_1024.png")
print("PNG иконки готовы")
