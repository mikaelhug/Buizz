import ast
import io
import subprocess
import sys
import tokenize
from collections.abc import Callable, Iterator
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GENERATED = {"web/src/api/schema.d.ts"}
TS_SUFFIXES = {".ts", ".tsx", ".mts", ".cts", ".js", ".mjs", ".cjs", ".jsx"}

Finding = tuple[int, str]


def rust(text: str) -> Iterator[Finding]:
    i, line, n = 0, 1, len(text)

    def advance_to(end: int) -> None:
        nonlocal i, line
        line += text.count("\n", i, end)
        i = end

    while i < n:
        c = text[i]
        prev = text[i - 1] if i else ""
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            yield line, text[i:end]
            advance_to(end)
        elif text.startswith("/*", i):
            start_line, depth, j = line, 0, i
            while j < n:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                    if depth == 0:
                        break
                else:
                    j += 1
            yield start_line, "/*"
            advance_to(j)
        elif c in "rbc" and not (prev.isalnum() or prev == "_"):
            j = i + 1
            if c in "bc" and j < n and text[j] == "r":
                j += 1
            hashes = 0
            raw = text[j - 1] == "r"
            while raw and j < n and text[j] == "#":
                hashes, j = hashes + 1, j + 1
            if j < n and text[j] == '"' and (raw or c in "bc"):
                if raw:
                    end = text.find('"' + "#" * hashes, j + 1)
                    advance_to(n if end < 0 else end + 1 + hashes)
                else:
                    advance_to(skip_quoted(text, j, '"'))
            else:
                advance_to(i + 1)
        elif c == '"':
            advance_to(skip_quoted(text, i, '"'))
        elif c == "'":
            if text.startswith("\\", i + 1):
                end = text.find("'", i + 3)
                advance_to(n if end < 0 else end + 1)
            elif i + 2 < n and text[i + 2] == "'":
                advance_to(i + 3)
            else:
                advance_to(i + 1)
        else:
            advance_to(i + 1)


def skip_quoted(text: str, start: int, quote: str) -> int:
    j = start + 1
    while j < len(text):
        if text[j] == "\\":
            j += 2
        elif text[j] == quote:
            return j + 1
        else:
            j += 1
    return j


def python(text: str) -> Iterator[Finding]:
    for token in tokenize.generate_tokens(io.StringIO(text).readline):
        if token.type == tokenize.COMMENT and not (token.start[0] == 1 and token.string.startswith("#!")):
            yield token.start[0], token.string
    for node in ast.walk(ast.parse(text)):
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) and node.body:
            first = node.body[0]
            if (
                isinstance(first, ast.Expr)
                and isinstance(first.value, ast.Constant)
                and isinstance(first.value.value, str)
            ):
                yield first.lineno, "docstring"


def hash_comments(text: str, quotes: tuple[str, ...], allow_shebang: bool) -> Iterator[Finding]:
    for number, line in enumerate(text.splitlines(), start=1):
        if allow_shebang and line.lstrip().startswith("#!"):
            continue
        i = 0
        while i < len(line):
            opener = next((quote for quote in quotes if line.startswith(quote, i)), None)
            if opener and starts_value(line, i):
                close = line.find(opener, i + len(opener)) if len(opener) == 3 else None
                if close is not None:
                    i = len(line) if close < 0 else close + 3
                else:
                    i = skip_quoted(line, i, opener) if opener == '"' else line.find("'", i + 1) + 1 or len(line)
                continue
            if line[i] == "#" and (i == 0 or line[i - 1] in " \t"):
                yield number, line[i:]
                break
            i += 1


def starts_value(line: str, index: int) -> bool:
    before = line[:index].rstrip()
    return not before or line[index - 1] in " \t" or before[-1] in "=:-[{,("


def toml(text: str) -> Iterator[Finding]:
    yield from multiline_aware(text, lambda body: hash_comments(body, ('"""', "'''", '"', "'"), False))


def multiline_aware(text: str, scan: Callable[[str], Iterator[Finding]]) -> Iterator[Finding]:
    lines = text.splitlines()
    masked: list[str] = []
    open_quote: str | None = None
    for line in lines:
        if open_quote:
            end = line.find(open_quote)
            if end < 0:
                masked.append("")
                continue
            line = " " * (end + 3) + line[end + 3 :]
            open_quote = None
        for quote in ('"""', "'''"):
            start = line.find(quote)
            if start >= 0 and line.find(quote, start + 3) < 0 and "#" not in line[:start]:
                open_quote = quote
                line = line[:start]
                break
        masked.append(line)
    yield from scan("\n".join(masked))


def yaml(text: str) -> Iterator[Finding]:
    masked: list[str] = []
    block_indent: int | None = None
    for line in text.splitlines():
        indent = len(line) - len(line.lstrip())
        if block_indent is not None:
            if not line.strip() or indent > block_indent:
                masked.append("")
                continue
            block_indent = None
        stripped = line.rstrip()
        if stripped.endswith(("|", ">", "|-", ">-", "|+", ">+")) and ":" in stripped or stripped in ("- |", "- >"):
            block_indent = indent
        masked.append(line)
    yield from hash_comments("\n".join(masked), ('"', "'"), False)


def justfile(text: str) -> Iterator[Finding]:
    yield from multiline_aware(text, lambda body: hash_comments(body, ('"""', "'''", '"', "'", "`"), True))


def typescript(paths: list[Path]) -> Iterator[tuple[Path, int, str]]:
    if not paths:
        return
    script = ROOT / "scripts" / "ts_comments.mjs"
    result = subprocess.run(
        ["node", str(script), *map(str, paths)], capture_output=True, text=True, check=False, cwd=ROOT
    )
    if result.returncode not in (0, 1):
        sys.exit(f"ts_comments.mjs failed: {result.stderr.strip()}")
    for row in result.stdout.splitlines():
        path, number, comment = row.split(":", 2)
        yield Path(path), int(number), comment


def scanner(path: Path) -> Callable[[str], Iterator[Finding]] | None:
    if path.name == "justfile":
        return justfile
    return {".rs": rust, ".py": python, ".toml": toml, ".yml": yaml, ".yaml": yaml}.get(path.suffix)


def tracked_files() -> list[Path]:
    output = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
        capture_output=True,
        text=True,
        check=True,
        cwd=ROOT,
    ).stdout
    return [Path(name) for name in output.splitlines() if name not in GENERATED and (ROOT / name).is_file()]


def main() -> int:
    findings: list[tuple[Path, int, str]] = []
    ts_files: list[Path] = []
    for path in tracked_files():
        if path.suffix in TS_SUFFIXES:
            ts_files.append(path)
            continue
        scan = scanner(path)
        if scan:
            findings.extend((path, number, comment) for number, comment in scan((ROOT / path).read_text()))
    findings.extend(typescript(ts_files))
    for path, number, comment in sorted(findings):
        print(f"{path}:{number}: {comment.strip()}")
    if findings:
        print(f"{len(findings)} comment(s) found; the codebase has none (CLAUDE.md directive 1)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
