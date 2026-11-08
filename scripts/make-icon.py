"""Genera el icono de NovaScore (1024x1024). Uso: python scripts/make-icon.py icono.png (requiere Pillow); luego: cd app && npx tauri icon icono.png (y borrar icons/android e icons/ios)."""
import math
import sys
from PIL import Image, ImageDraw

S = 4
N = 1024 * S


def p(v):
    return v * S


def poly(d, pts, fill):
    d.polygon([(x * S, y * S) for x, y in pts], fill=fill)


# --- fondo: cuadrado redondeado con degradado diagonal
grad = Image.new("RGB", (N, N))
gp = grad.load()
c0, c1 = (0x6f, 0x9d, 0xff), (0x25, 0x2f, 0x7a)
for y in range(0, N, 1):
    for x in range(0, N, 1):
        pass
# degradado por filas diagonales (rápido): usar línea a línea
gd = ImageDraw.Draw(grad)
steps = 2 * N
for i in range(steps):
    t = i / (steps - 1)
    col = tuple(int(c0[k] + (c1[k] - c0[k]) * t) for k in range(3))
    gd.line([(i, 0), (0, i)], fill=col, width=2)

mask = Image.new("L", (N, N), 0)
ImageDraw.Draw(mask).rounded_rectangle([p(40), p(40), p(984), p(984)], radius=p(215), fill=255)
img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
img.paste(grad, (0, 0), mask)

# --- capa de dibujo
layer = Image.new("RGBA", (N, N), (0, 0, 0, 0))
d = ImageDraw.Draw(layer)

# pentagrama
for i in range(5):
    y = 420 + i * 68
    d.rounded_rectangle([p(150), p(y - 7), p(874), p(y + 7)], radius=p(7), fill=(255, 255, 255, 120))

WHITE = (255, 255, 255, 255)


def head(cx, cy):
    """Cabeza de nota ovalada inclinada."""
    w, h, ang = 92, 66, math.radians(-22)
    pts = []
    for k in range(120):
        a = 2 * math.pi * k / 120
        x, y = w * math.cos(a), h * math.sin(a)
        pts.append((cx + x * math.cos(ang) - y * math.sin(ang), cy + x * math.sin(ang) + y * math.cos(ang)))
    poly(d, pts, WHITE)


n1, n2 = (392, 624), (640, 556)  # centros de las cabezas (sobre líneas/espacios)
head(*n1)
head(*n2)
# plicas (a la derecha de cada cabeza) hasta la barra
sx1, sx2 = n1[0] + 78, n2[0] + 78
top1, top2 = 262, 208
for sx, cy, top in ((sx1, n1[1], top1), (sx2, n2[1], top2)):
    d.rectangle([p(sx - 11), p(top), p(sx + 11), p(cy - 6)], fill=WHITE)
# barra que une las dos corcheas
poly(d, [(sx1 - 11, top1 - 4), (sx2 + 11, top2 - 4), (sx2 + 11, top2 + 56), (sx1 - 11, top1 + 56)], WHITE)

# destello "nova" (estrella de cuatro puntas), amarillo cálido
def star(cx, cy, r, inner=0.28, fill=(255, 214, 74, 255)):
    pts = []
    for k in range(8):
        a = math.pi / 4 * k - math.pi / 2
        rr = r if k % 2 == 0 else r * inner
        pts.append((cx + rr * math.cos(a), cy + rr * math.sin(a)))
    poly(d, pts, fill)


star(842, 214, 90)
star(918, 340, 38)
star(722, 120, 30)

# sombra suave bajo el dibujo
shadow = Image.new("RGBA", (N, N), (0, 0, 0, 0))
sh = layer.copy()
alpha = sh.split()[3].point(lambda a: int(a * 0.28))
shadow.paste((10, 14, 50, 255), (0, 0), alpha)
from PIL import ImageFilter
shadow = shadow.filter(ImageFilter.GaussianBlur(p(10)))
shifted = Image.new("RGBA", (N, N), (0, 0, 0, 0))
shifted.paste(shadow, (0, p(14)))
img = Image.alpha_composite(img, shifted)
img = Image.alpha_composite(img, layer)

out = img.resize((1024, 1024), Image.LANCZOS)
out.save(sys.argv[1])
print("ok", sys.argv[1])
