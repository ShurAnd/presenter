#!/usr/bin/env python3
"""Ищет в строках интерфейса символы, которых может не быть в шрифте egui.

Запуск из корня репозитория:
    python scripts/scan_ui_symbols.py

Проверяются только боевые файлы (`src/*.rs`, кроме тестов). Код возврата 1,
если найден подозрительный символ. Список проверенных символов (`▶`, `■`, `◀`)
можно расширять по мере визуальной проверки.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "src"
SAFE = set(range(0x20, 0x7F)) | set(range(0x400, 0x500)) | {ord(c) for c in "—–«»…·№"}
RENDERED = {ord(c) for c in "▶■◀"}  # проверены на снимках экрана
TEST_ONLY = {ord(c) for c in "🚀🦊"}  # встречаются только в тестовых данных


def is_test(name: str) -> bool:
    return name.endswith("_tests.rs") or name in {"tests.rs", "regression_tests.rs"}


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    found: dict[str, list[str]] = {}
    for file in sorted(ROOT.glob("*.rs")):
        if is_test(file.name):
            continue
        text = file.read_text(encoding="utf-8")
        for match in re.finditer(r'"(?:[^"\\]|\\.)*"', text):
            line = text.count("\n", 0, match.start()) + 1
            for char in match.group(0):
                code = ord(char)
                if code > 127 and code not in SAFE and code not in RENDERED and code not in TEST_ONLY:
                    found.setdefault(char, []).append(f"{file.name}:{line}")

    if not found:
        print("OK: подозрительных символов в строках интерфейса нет")
        return 0

    for char, places in sorted(found.items(), key=lambda item: ord(item[0])):
        print(f"U+{ord(char):04X} {char!r}: {', '.join(sorted(set(places)))}")
    print("\nЗамените символ на текст/ASCII или нарисуйте значок кодом.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
