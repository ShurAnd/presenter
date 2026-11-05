# -*- coding: utf-8 -*-
"""Готовит тестовые файлы Presentator 3.0.

Запуск (Windows, из корня репозитория):
    py -m venv .venv-testfiles
    .venv-testfiles\\Scripts\\python -m pip install pillow python-pptx reportlab imageio-ffmpeg
    .venv-testfiles\\Scripts\\python scripts\\make_test_files.py

Файлы кладутся в tests/fixtures/3.0. Скрипт можно запускать повторно: старые
файлы перезаписываются. PowerPoint и установленный ffmpeg не нужны: ffmpeg
берётся из пакета imageio-ffmpeg.
"""

from __future__ import annotations

import random
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.text import PP_ALIGN
from pptx.util import Inches, Pt
from reportlab.lib.pagesizes import A4, landscape
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas as rl_canvas

try:
    import imageio_ffmpeg
except ImportError:  # pragma: no cover - подсказка при запуске без зависимостей
    imageio_ffmpeg = None


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "tests" / "fixtures" / "3.0"

BACKGROUND = (18, 32, 58)
ACCENT = (91, 156, 255)
GREEN = (52, 199, 89)
INK = (232, 238, 247)
MUTED = (147, 161, 179)


def arial(bold: bool = False) -> str:
    names = ["arialbd.ttf", "segoeuib.ttf", "tahomabd.ttf"] if bold else ["arial.ttf", "segoeui.ttf", "tahoma.ttf"]
    for name in names:
        path = Path(r"C:\Windows\Fonts") / name
        if path.is_file():
            return str(path)
    raise FileNotFoundError("не найден шрифт Arial/Segoe UI в C:\\Windows\\Fonts")


def pil_font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(arial(bold), size)


def gradient(width: int, height: int, top: tuple[int, int, int], bottom: tuple[int, int, int]) -> Image.Image:
    img = Image.new("RGB", (width, height))
    draw = ImageDraw.Draw(img)
    for y in range(height):
        k = y / max(1, height - 1)
        color = tuple(int(top[i] + (bottom[i] - top[i]) * k) for i in range(3))
        draw.line([(0, y), (width, y)], fill=color)
    return img


def slide_image(width: int, height: int, title: str, subtitle: str) -> Image.Image:
    img = gradient(width, height, (22, 42, 76), (10, 16, 28))
    draw = ImageDraw.Draw(img)
    # Декоративные круги и полоса.
    draw.ellipse([width * 0.72, -height * 0.25, width * 1.15, height * 0.55], outline=ACCENT, width=max(3, width // 300))
    draw.ellipse([-width * 0.18, height * 0.68, width * 0.35, height * 1.2], outline=GREEN, width=max(3, width // 300))
    draw.rounded_rectangle([width * 0.06, height * 0.16, width * 0.09, height * 0.84], radius=width // 200, fill=ACCENT)
    draw.text((width * 0.12, height * 0.26), title, font=pil_font(max(28, width // 22), True), fill=INK)
    draw.text((width * 0.12, height * 0.42), subtitle, font=pil_font(max(18, width // 46)), fill=MUTED)
    return img


def make_images() -> None:
    font_big = pil_font(64, True)
    font_mid = pil_font(30)

    base = slide_image(1920, 1080, "Presentator 3.0", "картинка 1920×1080 · проверка вписывания и увеличения")
    draw = ImageDraw.Draw(base)
    draw.rounded_rectangle([120, 760, 1800, 940], radius=28, fill=(24, 46, 84), outline=ACCENT, width=4)
    draw.text((160, 805), "Если вы видите эту надпись целиком — картинка вписана правильно", font=font_mid, fill=INK)
    base.save(OUT / "image_1920x1080.png")
    base.save(OUT / "image_1920x1080.jpg", quality=90)
    base.save(OUT / "image_1920x1080.bmp")
    base.save(OUT / "image_1920x1080.webp", quality=90)

    four_three = slide_image(1600, 1200, "4:3", "проверка полей по краям экрана 16:9")
    four_three.save(OUT / "image_1600x1200.jpg", quality=90)

    portrait = slide_image(1080, 1920, "Портрет", "вертикальная картинка вписывается целиком")
    portrait.save(OUT / "image_portrait_1080x1920.png")

    # Анимированные кадры: движущийся круг, номер кадра и секунда.
    frames: list[Image.Image] = []
    for n in range(24):
        img = gradient(1280, 720, (16, 30, 54), (8, 12, 22))
        d = ImageDraw.Draw(img)
        x = 120 + n * 44
        d.ellipse([x, 300, x + 120, 420], fill=ACCENT)
        d.ellipse([1100 - n * 34, 470, 1200 - n * 34, 570], fill=GREEN)
        d.text((60, 50), f"Анимация · кадр {n + 1}/24", font=pil_font(42, True), fill=INK)
        d.text((60, 620), "GIF / WebP / APNG", font=pil_font(28), fill=MUTED)
        frames.append(img)
    frames[0].save(OUT / "anim_24_frames.gif", save_all=True, append_images=frames[1:], duration=80, loop=0, optimize=True)
    frames[0].save(OUT / "anim_24_frames.webp", save_all=True, append_images=frames[1:], duration=80, loop=0, quality=80, method=4)
    frames[0].save(OUT / "anim_24_frames.png", save_all=True, append_images=frames[1:], duration=80, loop=0)

    # Фон сцены: 30 кадров, «дышащий» градиент и бегущая строка.
    bg_frames: list[Image.Image] = []
    for n in range(30):
        img = gradient(1280, 720, (10, 24 + n, 48), (6, 10, 20))
        d = ImageDraw.Draw(img)
        y = 250 + int(30 * (n / 29))
        d.rounded_rectangle([80, y, 1200, y + 170], radius=30, fill=(18, 38, 70), outline=ACCENT, width=3)
        d.text((120, y + 40), "Добро пожаловать", font=pil_font(64, True), fill=INK)
        d.text((120, y + 118), f"фон сцены · кадр {n + 1}/30", font=pil_font(26), fill=MUTED)
        bg_frames.append(img)
    bg_frames[0].save(OUT / "background_1280x720.gif", save_all=True, append_images=bg_frames[1:], duration=66, loop=0, optimize=True)
    slide_image(1920, 1080, "Фон 1920×1080", "статичная картинка фона сцены").save(OUT / "background_1920x1080.png")

    # Постер для видео в PowerPoint.
    slide_image(1280, 720, "Видео 10 секунд", "нажмите «Старт» в списке медиа").save(OUT / "poster.jpg", quality=90)


def make_pdf() -> None:
    regular, bold = arial(False), arial(True)
    pdfmetrics.registerFont(TTFont("TestArial", regular))
    pdfmetrics.registerFont(TTFont("TestArial-Bold", bold))

    path = OUT / "pdf_10_slides.pdf"
    c = rl_canvas.Canvas(str(path), pagesize=landscape(A4))
    width, height = landscape(A4)
    titles = [
        "Начало презентации",
        "Второй слайд",
        "Кириллица и цифры",
        "Проверка превью",
        "Переход по номеру",
        "Увеличение слайда",
        "Предыдущий и следующий",
        "Проверка шрифтов",
        "Почти конец",
        "КОНЕЦ",
    ]
    for n, title in enumerate(titles, start=1):
        c.setFillColorRGB(0.10, 0.20, 0.36)
        c.rect(0, height - 78, width, 78, stroke=0, fill=1)
        c.setFillColorRGB(0.92, 0.95, 1.0)
        c.setFont("TestArial-Bold", 24)
        c.drawString(40, height - 50, f"{n}. {title}")
        c.setFillColorRGB(0.15, 0.18, 0.23)
        c.setFont("TestArial", 18)
        lines = [
            f"Слайд {n} из 10. Проверяем вписывание страницы и чёткость.",
            "Кириллица: съешь ещё этих мягких французских булок, да выпей чаю.",
            "Цифры и знаки: 0123456789 · № · 100 % · 16:9 · «кавычки».",
        ]
        for i, line in enumerate(lines):
            c.drawString(40, height - 130 - i * 28, line)
        c.setFillColorRGB(0.30, 0.36, 0.45)
        c.setFont("TestArial", 12)
        c.drawRightString(width - 40, 30, f"Presentator 3.0 · страница {n} / 10")
        c.setFillColorRGB(0.36, 0.61, 1.0)
        c.rect(40, 60, 220, 8, stroke=0, fill=1)
        c.showPage()
    c.save()

    # Тяжёлый векторный PDF: много тонких линий и фигур — проверка рендера и зума.
    heavy = OUT / "pdf_heavy_vector.pdf"
    c = rl_canvas.Canvas(str(heavy), pagesize=landscape(A4))
    random.seed(42)
    c.setFillColorRGB(0.98, 0.98, 0.98)
    c.rect(0, 0, width, height, stroke=0, fill=1)
    for _ in range(12000):
        c.setStrokeColorRGB(random.random(), random.random(), random.random())
        c.setLineWidth(random.choice([0.2, 0.3, 0.5]))
        x1, y1 = random.uniform(0, width), random.uniform(0, height)
        x2, y2 = random.uniform(0, width), random.uniform(0, height)
        c.line(x1, y1, x2, y2)
    c.setFillColorRGB(0.1, 0.1, 0.15)
    c.setFont("TestArial-Bold", 30)
    c.drawString(40, height - 60, "Тяжёлый векторный PDF: 12 000 линий")
    c.save()


def textbox(slide, left, top, width, height, text, size=24, bold=False, color=INK, align=PP_ALIGN.LEFT):
    box = slide.shapes.add_textbox(Inches(left), Inches(top), Inches(width), Inches(height))
    frame = box.text_frame
    frame.clear()
    frame.word_wrap = True
    p = frame.paragraphs[0]
    p.alignment = align
    run = p.add_run()
    run.text = text
    run.font.size = Pt(size)
    run.font.bold = bold
    run.font.name = "Arial"
    run.font.color.rgb = RGBColor(*color)
    return box


def make_pptx() -> None:
    prs = Presentation()
    prs.slide_width = Inches(13.333)
    prs.slide_height = Inches(7.5)
    blank = prs.slide_layouts[6]

    # --- 5 слайдов: текст, картинка, таблица, заметки.
    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.7, 11.8, 1.2, "Presentator 3.0", size=44, bold=True)
    textbox(s, 0.8, 2.0, 11.8, 0.8, "Тестовая презентация · 5 слайдов · 16:9", size=24, color=MUTED)
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 1 · титульный", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте: открытие, превью, заголовок в списке."

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.6, 11.8, 0.8, "Что проверяем", size=34, bold=True)
    bullets = [
        "Листание вперёд и назад, пробел и кликер.",
        "Переход к слайду по номеру: наберите «4» и Enter.",
        "Превью предыдущего, текущего и следующего слайда.",
        "Увеличение: «+» до 200 % и сдвиг стрелками с Ctrl.",
        "Закрытие показа двойным Esc и возврат фона.",
    ]
    top = 1.8
    for item in bullets:
        textbox(s, 1.0, top, 11.4, 0.6, "•  " + item, size=22)
        top += 0.75
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 2 · список", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте пошаговое листание и превью."

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.6, 6.2, 0.8, "Картинка в слайде", size=32, bold=True)
    textbox(s, 0.8, 1.6, 5.6, 2.2, "Картинка вставлена в презентацию, а не ссылкой — на сервере она будет видна.", size=20)
    s.shapes.add_picture(str(OUT / "image_1600x1200.jpg"), Inches(6.8), Inches(1.4), height=Inches(4.8))
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 3 · картинка", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте, что картинка не растянута."

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.6, 11.8, 0.8, "Таблица", size=32, bold=True)
    table = s.shapes.add_table(3, 4, Inches(0.9), Inches(1.8), Inches(11.4), Inches(2.6)).table
    data = [
        ["Формат", "Проверка", "Ожидание", "Статус"],
        ["PDF", "10 страниц", "листается и увеличивается", "ок"],
        ["PPTX", "5 слайдов", "анимации и заметки", "ок"],
    ]
    for r, row in enumerate(data):
        for c, value in enumerate(row):
            cell = table.cell(r, c)
            cell.text = value
            for p in cell.text_frame.paragraphs:
                for run in p.runs:
                    run.font.size = Pt(16)
                    run.font.name = "Arial"
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 4 · таблица", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте таблицу и кириллицу."

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 2.4, 11.8, 1.2, "КОНЕЦ", size=64, bold=True, align=PP_ALIGN.CENTER)
    textbox(s, 0.8, 3.8, 11.8, 0.8, "Последний слайд · «Вперёд» не должен закрывать показ", size=22, color=MUTED, align=PP_ALIGN.CENTER)
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 5 · конец", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте: на последнем слайде «Вперёд» игнорируется."

    prs.save(OUT / "pptx_5_slides.pptx")

    # --- Медиа: видео и звук внутри слайда.
    prs = Presentation()
    prs.slide_width = Inches(13.333)
    prs.slide_height = Inches(7.5)
    blank = prs.slide_layouts[6]

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.7, 11.8, 1.0, "Видео и звук в презентации", size=38, bold=True)
    textbox(s, 0.8, 1.9, 11.8, 0.8, "Слайд 2 — видео, слайд 3 — звук. Проверьте список медиа и рамки на превью.", size=20, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте, что на этом слайде медиа нет."

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.6, 11.8, 0.8, "Видео на слайде", size=32, bold=True)
    s.shapes.add_movie(
        str(OUT / "video_10s.mp4"),
        Inches(0.9),
        Inches(1.6),
        Inches(8.4),
        Inches(4.7),
        poster_frame_image=str(OUT / "poster.jpg"),
        mime_type="video/mp4",
    )
    textbox(s, 9.6, 1.8, 3.2, 3.6, "Запуск, пауза и стоп — из списка «Видео на слайде» и щелчком по рамке на превью.", size=18)
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 2 · видео", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте: play/pause/stop, позиция и длительность 10 с."

    s = prs.slides.add_slide(blank)
    textbox(s, 0.8, 0.6, 11.8, 0.8, "Звук на слайде", size=32, bold=True)
    s.shapes.add_movie(
        str(OUT / "music_30s.mp3"),
        Inches(1.0),
        Inches(2.0),
        Inches(3.2),
        Inches(1.8),
        mime_type="audio/mpeg",
    )
    textbox(s, 4.8, 2.2, 7.4, 1.6, "Звук вставлен в презентацию. В списке медиа он должен быть отдельной строкой.", size=20)
    textbox(s, 0.8, 6.4, 11.8, 0.5, "Слайд 3 · звук", size=14, color=MUTED)
    s.notes_slide.notes_text_frame.text = "Проверьте: звук виден в списке и запускается."

    prs.save(OUT / "pptx_media.pptx")


def make_linked_media_pptx() -> None:
    """PPTX с внешней ссылкой на отсутствующий файл — проверка предупреждения.

    python-pptx вставляет медиа внутрь пакета. Здесь ссылка переписывается на
    внешнюю (TargetMode="External") и внутренний файл удаляется из пакета.
    """
    source = OUT / "pptx_media.pptx"
    target = OUT / "pptx_linked_video.pptx"
    if not source.is_file():
        return
    shutil.copyfile(source, target)
    tmp = OUT / "_linked_tmp"
    if tmp.exists():
        shutil.rmtree(tmp)
    with zipfile.ZipFile(target) as z:
        z.extractall(tmp)
    rels = sorted((tmp / "ppt" / "slides" / "_rels").glob("slide2.xml.rels"))
    if not rels:
        shutil.rmtree(tmp)
        return
    rel = rels[0]
    text = rel.read_text(encoding="utf-8")
    import re

    # 1. Внутреннюю связь video заменяем внешней ссылкой на отсутствующий файл.
    match = re.search(r'<Relationship[^>]+Type="[^"]*/video"[^>]*/>', text)
    if not match:
        shutil.rmtree(tmp)
        return
    external = (
        '<Relationship Id="rIdLinkedVideo" '
        'Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/video" '
        'Target="file:///C:/presentator_missing/video.mp4" TargetMode="External"/>'
    )
    text = text.replace(match.group(0), external)
    # 2. Связь p14:media на внутренний файл больше не нужна — иначе PowerPoint
    #    увидит ссылку на удалённую часть и предложит восстановить файл.
    text = re.sub(r'<Relationship[^>]+Type="[^"]*/media"[^>]*/>', '', text)
    rel.write_text(text, encoding="utf-8")

    # 3. В самом слайде: убираем p14:media и направляем videoFile на внешнюю связь.
    slide = tmp / "ppt" / "slides" / "slide2.xml"
    s = ""
    if slide.is_file():
        s = slide.read_text(encoding="utf-8")
        s = re.sub(r"<p:extLst>.*?</p:extLst>", "", s, flags=re.S)
        s = re.sub(r'(<a:videoFile[^>]*r:link=")[^"]+(")', r"\1rIdLinkedVideo\2", s)
        slide.write_text(s, encoding="utf-8")

    # 4. Удаляем внутренние видеофайлы и проверяем, что не осталось висячих ссылок.
    media = tmp / "ppt" / "media"
    if media.is_dir():
        for item in media.iterdir():
            if item.suffix.lower() in {".mp4", ".m4v", ".mov"}:
                item.unlink()

    ids = set(re.findall(r'Id="([^"]+)"', text))
    refs = {x for x in re.findall(r'r:(?:id|embed|link)="([^"]+)"', s) if x}
    dangling = refs - ids
    if dangling:
        print(f"  ! pptx_linked_video: висячие ссылки {sorted(dangling)}")

    # Пересобираем пакет.
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
        for item in tmp.rglob("*"):
            if item.is_file():
                z.write(item, item.relative_to(tmp).as_posix())
    shutil.rmtree(tmp)


def ffmpeg_exe() -> str | None:
    if imageio_ffmpeg is None:
        return None
    return imageio_ffmpeg.get_ffmpeg_exe()


def run_ffmpeg(ffmpeg: str, args: list[str], what: str) -> bool:
    result = subprocess.run([ffmpeg, "-y", "-hide_banner", "-loglevel", "error", *args], capture_output=True, text=True)
    if result.returncode != 0:
        print(f"  ! {what}: {result.stderr.strip().splitlines()[-1] if result.stderr.strip() else 'ошибка ffmpeg'}")
        return False
    return True


def make_media() -> None:
    ffmpeg = ffmpeg_exe()
    if not ffmpeg:
        print("  ! imageio-ffmpeg не установлен — видео и сжатый звук пропущены")
        # WAV всё равно можно сделать средствами Python.
        make_wav(OUT / "music_5s.wav")
        (OUT / "video_bad.mp4").write_bytes(random_bytes(64 * 1024))
        return

    # Видео 10 секунд: тестовый узор + тон 440 Гц.
    run_ffmpeg(
        ffmpeg,
        [
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-t", "10", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart",
            str(OUT / "video_10s.mp4"),
        ],
        "video_10s.mp4",
    )
    # WebM/VP9 — запасной формат для libmpv.
    run_ffmpeg(
        ffmpeg,
        [
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30",
            "-t", "5", "-c:v", "libvpx-vp9", "-b:v", "1M", "-an",
            str(OUT / "video_5s.webm"),
        ],
        "video_5s.webm",
    )
    # Музыка: разные тона, чтобы треки различались на слух.
    run_ffmpeg(ffmpeg, ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=30", "-c:a", "libmp3lame", "-b:a", "128k", str(OUT / "music_30s.mp3")], "music_30s.mp3")
    run_ffmpeg(ffmpeg, ["-f", "lavfi", "-i", "sine=frequency=523.25:sample_rate=44100:duration=10", "-c:a", "flac", str(OUT / "music_10s.flac")], "music_10s.flac")
    run_ffmpeg(ffmpeg, ["-f", "lavfi", "-i", "sine=frequency=659.25:sample_rate=44100:duration=10", "-c:a", "libvorbis", "-q:a", "4", str(OUT / "music_10s.ogg")], "music_10s.ogg")
    run_ffmpeg(ffmpeg, ["-f", "lavfi", "-i", "sine=frequency=349.23:sample_rate=44100:duration=10", "-c:a", "aac", "-b:a", "128k", str(OUT / "music_10s.m4a")], "music_10s.m4a")
    make_wav(OUT / "music_5s.wav")
    (OUT / "video_bad.mp4").write_bytes(random_bytes(64 * 1024))


def random_bytes(size: int) -> bytes:
    rnd = random.Random(7)
    return bytes(rnd.randrange(256) for _ in range(size))


def make_wav(path: Path) -> None:
    """5 секунд синуса 440 Гц, PCM 16 бит — без ffmpeg."""
    import math
    import struct
    import wave

    rate = 44100
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        frames = bytearray()
        for i in range(rate * 5):
            value = int(12000 * math.sin(2 * math.pi * 440 * i / rate))
            frames.extend(struct.pack("<h", value))
        w.writeframes(bytes(frames))


def validate() -> None:
    print("\nПроверка файлов:")
    from pptx import Presentation

    for name in ["pptx_5_slides.pptx", "pptx_media.pptx", "pptx_linked_video.pptx"]:
        path = OUT / name
        if not path.is_file():
            print(f"  - {name}: нет")
            continue
        prs = Presentation(str(path))
        print(f"  ✓ {name}: {len(prs.slides)} слайдов, {path.stat().st_size // 1024} КБ")

    for name in ["image_1920x1080.png", "image_1920x1080.jpg", "image_1920x1080.bmp", "image_1920x1080.webp", "anim_24_frames.gif", "anim_24_frames.webp", "anim_24_frames.png", "background_1280x720.gif"]:
        path = OUT / name
        if path.is_file():
            with Image.open(path) as img:
                print(f"  ✓ {name}: {img.width}×{img.height}, {getattr(img, 'n_frames', 1)} кадр(ов)")

    for name in ["pdf_10_slides.pdf", "pdf_heavy_vector.pdf"]:
        path = OUT / name
        if path.is_file():
            head = path.read_bytes()[:5]
            print(f"  {'✓' if head == b'%PDF-' else '!'} {name}: {path.stat().st_size // 1024} КБ")

    ffmpeg = ffmpeg_exe()
    for name in ["video_10s.mp4", "video_5s.webm", "music_30s.mp3", "music_5s.wav", "music_10s.flac", "music_10s.ogg", "music_10s.m4a"]:
        path = OUT / name
        if not path.is_file():
            continue
        if ffmpeg:
            r = subprocess.run([ffmpeg, "-v", "error", "-i", str(path), "-f", "null", "-"], capture_output=True, text=True)
            print(f"  {'✓' if r.returncode == 0 else '!'} {name}: {path.stat().st_size // 1024} КБ")
        else:
            print(f"  ✓ {name}: {path.stat().st_size // 1024} КБ")


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    OUT.mkdir(parents=True, exist_ok=True)
    print(f"Каталог: {OUT}")
    print("Картинки и анимации…")
    make_images()
    print("PDF…")
    make_pdf()
    print("Видео и звук…")
    make_media()
    print("PowerPoint…")
    make_pptx()
    make_linked_media_pptx()
    validate()
    total = sum(p.stat().st_size for p in OUT.iterdir() if p.is_file())
    print(f"\nГотово: {len([p for p in OUT.iterdir() if p.is_file()])} файлов, {total / 1024 / 1024:.1f} МБ")
    return 0


if __name__ == "__main__":
    sys.exit(main())
