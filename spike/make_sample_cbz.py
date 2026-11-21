"""Generate a CBZ of procedural JPEG pages for the browser spike."""

import argparse
from io import BytesIO
from pathlib import Path
from random import Random
from statistics import NormalDist
from zipfile import ZIP_STORED, ZipFile, ZipInfo

from PIL import Image, ImageChops, ImageDraw, ImageFont


PAGE_SIZE = (1600, 2400)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pages", required=True, type=int)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    if args.pages < 1:
        parser.error("--pages must be positive")

    width, height = PAGE_SIZE
    gradient = Image.linear_gradient("L").resize(PAGE_SIZE).point(
        [45 + round(value * 165 / 255) for value in range(256)]
    )
    random = Random(651)
    gaussian = NormalDist(mu=128, sigma=20)
    noise_levels = [round(gaussian.inv_cdf((value + 0.5) / 256)) for value in range(256)]
    font = ImageFont.load_default(size=480)

    with ZipFile(args.out, "w", compression=ZIP_STORED) as archive:
        for number in range(1, args.pages + 1):
            noise = Image.frombytes("L", PAGE_SIZE, random.randbytes(width * height))
            page = ImageChops.add(gradient, noise.point(noise_levels), offset=-128)
            ImageDraw.Draw(page).text(
                (width // 2, height // 2),
                str(number),
                anchor="mm",
                font=font,
                fill=245,
                stroke_width=8,
                stroke_fill=25,
            )
            encoded = BytesIO()
            page.save(encoded, format="JPEG", quality=90)
            archive.writestr(ZipInfo(f"{number:05d}.jpg"), encoded.getvalue())

    print(f"{args.out} {args.out.stat().st_size} bytes")


if __name__ == "__main__":
    main()
