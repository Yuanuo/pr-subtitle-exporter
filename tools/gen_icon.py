"""生成应用图标：assets/app.ico（多尺寸）+ assets/icon.png（256，窗口图标）。

图形：圆角方块渐变底 + 两条字幕条（白 + 琥珀色），4x 超采样抗锯齿。
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from png import write_rgba_png  # noqa: E402

SS = 4  # 超采样倍数
BG_TOP = (0x46, 0x7B, 0xF5)
BG_BOTTOM = (0x25, 0x3E, 0xA6)
BAR_WHITE = (0xFF, 0xFF, 0xFF)
BAR_AMBER = (0xFF, 0xC8, 0x57)


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def inside_round_rect(x, y, x0, y0, x1, y1, r):
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    # 四角做圆角判定
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    dx = x - cx
    dy = y - cy
    return dx * dx + dy * dy <= r * r


def render(size):
    """返回 size x size 的 RGBA 行列表。"""
    n = size * SS
    # 画在 n x n 的超采样画布上
    canvas = [[(0, 0, 0, 0)] * n for _ in range(n)]

    pad = n * 0.045
    x0, y0, x1, y1 = pad, pad, n - pad, n - pad
    radius = n * 0.22

    # 两条字幕条
    bar_h = n * 0.095
    bar_r = bar_h / 2
    bar1_y = n * 0.425 - bar_h / 2
    bar2_y = n * 0.585 - bar_h / 2
    bar1_x0, bar1_x1 = n * 0.215, n * 0.785
    bar2_x0, bar2_x1 = n * 0.215, n * 0.655

    for y in range(n):
        # 背景纵向渐变
        t = (y - y0) / max(1.0, (y1 - y0))
        bg = lerp(BG_TOP, BG_BOTTOM, max(0.0, min(1.0, t)))
        row = canvas[y]
        for x in range(n):
            if inside_round_rect(x + 0.5, y + 0.5, x0, y0, x1, y1, radius):
                color = bg
                if inside_round_rect(x + 0.5, y + 0.5, bar1_x0, bar1_y, bar1_x1, bar1_y + bar_h, bar_r):
                    color = BAR_WHITE
                elif inside_round_rect(x + 0.5, y + 0.5, bar2_x0, bar2_y, bar2_x1, bar2_y + bar_h, bar_r):
                    color = BAR_AMBER
                row[x] = (color[0], color[1], color[2], 255)

    # 降采样（盒式平均，保留 alpha）
    out = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            r = g = b = a = 0
            for dy in range(SS):
                line = canvas[y * SS + dy]
                for dx in range(SS):
                    px = line[x * SS + dx]
                    if px[3]:
                        r += px[0] * px[3]
                        g += px[1] * px[3]
                        b += px[2] * px[3]
                        a += px[3]
            if a:
                row += bytes((r // a, g // a, b // a, a // (SS * SS)))
            else:
                row += b"\x00\x00\x00\x00"
        out.append(row)
    return out


def ico_entry_bmp(size, rows):
    """32 位 BMP 图标条目：BITMAPINFOHEADER + BGRA 自下而上 + AND 掩码。"""
    header = struct.pack(
        "<IiiHHIIiiII",
        40,          # biSize
        size,        # biWidth
        size * 2,    # biHeight（XOR + AND）
        1,           # biPlanes
        32,          # biBitCount
        0,           # biCompression
        size * size * 4 + ((size + 31) // 32 * 4) * size,
        0, 0, 0, 0,
    )
    body = bytearray()
    for y in range(size - 1, -1, -1):
        row = rows[y]
        for x in range(size):
            r, g, b, a = row[x * 4:x * 4 + 4]
            body += bytes((b, g, r, a))
    # AND 掩码：alpha 为 0 的像素置 1
    mask = bytearray()
    for y in range(size - 1, -1, -1):
        row = rows[y]
        bits = bytearray(((size + 31) // 32) * 4)
        for x in range(size):
            if row[x * 4 + 3] == 0:
                bits[x // 8] |= 0x80 >> (x % 8)
        mask += bits
    return header + bytes(body) + bytes(mask)


def ico_entry_png(size, rows):
    tmp = f"_tmp_{size}.png"
    write_rgba_png(tmp, size, size, rows)
    data = open(tmp, "rb").read()
    os.remove(tmp)
    return data


def build_ico(sizes, out_path):
    entries = []
    for size in sizes:
        rows = render(size)
        blob = ico_entry_png(size, rows) if size >= 128 else ico_entry_bmp(size, rows)
        entries.append((size, blob))
        if size == 256:
            write_rgba_png(os.path.join(os.path.dirname(out_path), "icon.png"), size, size, rows)

    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dir_entries = bytearray()
    blobs = bytearray()
    for size, blob in entries:
        dir_entries += struct.pack(
            "<BBBBHHII",
            0 if size >= 256 else size,
            0 if size >= 256 else size,
            0, 0, 1, 32, len(blob), offset,
        )
        blobs += blob
        offset += len(blob)
    with open(out_path, "wb") as f:
        f.write(header + bytes(dir_entries) + bytes(blobs))
    print(f"wrote {out_path}: {[s for s, _ in entries]}")


if __name__ == "__main__":
    base = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    assets = os.path.join(base, "assets")
    os.makedirs(assets, exist_ok=True)
    build_ico([16, 24, 32, 48, 64, 128, 256], os.path.join(assets, "app.ico"))
