
import math
import re
from pathlib import Path


colors = {
    "blue": "#1E90FF",
    "gray": "#9CA3AF",
    "green": "#00C853",
    "white": "#FFFFFF",
}

canvas_size = 512
background_size = 500
center = canvas_size / 2

# 超椭圆背景参数（n=3，尺寸为 500×500）
n = 3
a = background_size / 2
b = background_size / 2
points = []
segments = 64
for i in range(segments + 1):
    t = 2 * math.pi * i / segments
    cos_t = math.cos(t)
    sin_t = math.sin(t)
    x = a * math.copysign(abs(cos_t) ** (2 / n), cos_t)
    y = b * math.copysign(abs(sin_t) ** (2 / n), sin_t)
    points.append((center + x, center + y))

path_d = f"M {points[0][0]:.2f} {points[0][1]:.2f}"
for point in points[1:]:
    path_d += f" L {point[0]:.2f} {point[1]:.2f}"
path_d += " Z"

# 地球图标参数
earth_radius = 256 - 76
bezier_control_x = 2 * earth_radius / 3

output_dir = Path(__file__).resolve().parent.parent / "assets" / "icons"
output_dir.mkdir(parents=True, exist_ok=True)

for name, color in colors.items():
    svg_template = f'''<svg width="{canvas_size}" height="{canvas_size}" viewBox="0 0 {canvas_size} {canvas_size}" fill="none" xmlns="http://www.w3.org/2000/svg">
  <!-- 黑色 n=3 超椭圆背景 -->
  <path d="{path_d}" fill="#000000"/>

  <!-- 地球图标，居中 -->
  <g transform="translate({center:.0f}, {center:.0f})">
    <!-- 地球轮廓圆 -->
    <circle cx="0" cy="0" r="{earth_radius:.2f}" stroke="{color}" stroke-width="24"/>

    <!-- 左侧经线（二次贝塞尔弧线） -->
    <path d="M 0 -{earth_radius:.2f} Q -{bezier_control_x:.2f} 0 0 {earth_radius:.2f}" stroke="{color}" stroke-width="24" stroke-linecap="round"/>

    <!-- 右侧经线（二次贝塞尔弧线） -->
    <path d="M 0 -{earth_radius:.2f} Q {bezier_control_x:.2f} 0 0 {earth_radius:.2f}" stroke="{color}" stroke-width="24" stroke-linecap="round"/>

    <!-- 纬线（赤道，被两经线分成三等分） -->
    <line x1="-{earth_radius:.2f}" y1="0" x2="{earth_radius:.2f}" y2="0" stroke="{color}" stroke-width="24" stroke-linecap="round"/>
  </g>
</svg>'''

    # 移除 SVG 注释后写入生成文件
    svg_content = re.sub(r"\s*<!--.*?-->\s*", "\n", svg_template, flags=re.DOTALL)
    output_file = output_dir / f"{name}.svg"
    output_file.write_text(svg_content, encoding="utf-8")
    print(f"已生成 {output_file}，文件大小：{len(svg_content)} 字节")
