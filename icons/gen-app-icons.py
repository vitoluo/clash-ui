# 需要安装Cairo 图形库，推荐使用 conda
# conda install cairo
import shutil
from pathlib import Path

try:
    import cairosvg
    from PIL import Image
except ModuleNotFoundError as error:
    raise SystemExit("请先安装依赖：python -m pip install cairosvg pillow") from error


project_root = Path(__file__).resolve().parent.parent
source_svg = project_root / "assets" / "icons" / "white.svg"
output_png = project_root / "icons" / "app.png"
secondary_png = project_root / "assets" / "app.png"
output_ico = project_root / "icons" / "app.ico"
png_size = 512
ico_sizes = ((16, 16), (32, 32), (64, 64), (128, 128), (256, 256))


def main() -> None:
    if not source_svg.exists():
        raise FileNotFoundError(f"找不到 SVG 源文件：{source_svg}")

    output_png.parent.mkdir(parents=True, exist_ok=True)
    secondary_png.parent.mkdir(parents=True, exist_ok=True)
    cairosvg.svg2png(
        url=str(source_svg),
        write_to=str(output_png),
        output_width=png_size,
        output_height=png_size,
    )
    shutil.copyfile(output_png, secondary_png)

    with Image.open(output_png) as image:
        image = image.convert("RGBA")
        image.save(output_ico, format="ICO", sizes=ico_sizes)

    print(f"已生成 PNG：{output_png}、{secondary_png}，尺寸：{png_size}×{png_size}")
    print(f"已生成 ICO：{output_ico}，尺寸：{', '.join(f'{w}×{h}' for w, h in ico_sizes)}")


if __name__ == "__main__":
    main()
