#!/usr/bin/env python3
"""Collect model-generated RSScript candidates for the offline eval corpus.

This is a caller-owned runner. The scorer (`cargo run -p rsscript-xtask --
agent-eval`) still never touches a model: this script only produces candidate
files and `candidate.v1.json` sidecars in the layout the scorer accepts, so the
scoring step stays deterministic and offline.

Generation modes
----------------
`prompt_only`    the task prompt plus the task's `.rssi` interfaces.
`language_card`  the same, prefixed with `docs/generated/language-card.md` and
                 `AGENT.md`.
`repair_loop`    starts from the `language_card` prompt, then feeds `rss check
                 --json` diagnostics back for up to `--max-turns` turns.

Everything is stdlib-only and idempotent: a task that already has a candidate is
skipped unless `--force` is passed.

Example
-------
    python3 tools/collect-model-samples.py --model sonnet --mode prompt_only
    python3 tools/collect-model-samples.py --model sonnet --mode repair_loop
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TASKS_DIR = ROOT / "evals" / "tasks"
EVALS = ROOT / "evals"
LANGUAGE_CARD = ROOT / "docs" / "generated" / "language-card.md"
AGENT_GUIDE = ROOT / "AGENT.md"

MODES = ("prompt_only", "language_card", "repair_loop")

CODE_FENCE = re.compile(r"```(?:rsscript|rss)?[ \t]*\n(.*?)```", re.DOTALL)

OUTPUT_CONTRACT = (
    "Reply with exactly one fenced code block containing the complete "
    "RSScript program and nothing else. No prose before or after the block, "
    "no explanation, no alternatives. The file must be self-contained and "
    "must compile as written."
)


# ---------------------------------------------------------------------------
# corpus


def load_tasks() -> list[dict]:
    tasks = []
    for path in sorted(TASKS_DIR.glob("*.toml")):
        with path.open("rb") as handle:
            task = tomllib.load(handle)
        task["_path"] = path
        tasks.append(task)
    return tasks


def interface_paths(task: dict) -> list[Path]:
    return [EVALS / relative for relative in task.get("interfaces", [])]


def interface_block(task: dict) -> str:
    parts = []
    for path in interface_paths(task):
        parts.append(
            f"Declared host interface `{path.name}` (the ONLY host functions "
            f"available to you):\n\n```rsscript\n{path.read_text().strip()}\n```"
        )
    return "\n\n".join(parts)


def seed_block(task: dict) -> str:
    """Repair/review tasks refer to "the candidate"; show it."""
    if "generation" in task.get("tags", []):
        return ""
    candidate = EVALS / task["candidate"]
    if not candidate.is_file():
        return ""
    return (
        "The candidate under discussion is:\n\n"
        f"```rsscript\n{candidate.read_text().strip()}\n```"
    )


def build_prompt(task: dict, mode: str) -> str:
    sections: list[str] = []
    if mode in ("language_card", "repair_loop"):
        sections.append(
            "You are writing RSScript. The following two documents are the "
            "authoritative generation guidance for this language.\n\n"
            "=== BEGIN docs/generated/language-card.md ===\n"
            f"{LANGUAGE_CARD.read_text().strip()}\n"
            "=== END docs/generated/language-card.md ===\n\n"
            "=== BEGIN AGENT.md ===\n"
            f"{AGENT_GUIDE.read_text().strip()}\n"
            "=== END AGENT.md ==="
        )
    sections.append(f"Task: {task['prompt'].strip()}")
    seed = seed_block(task)
    if seed:
        sections.append(seed)
    interfaces = interface_block(task)
    if interfaces:
        sections.append(interfaces)
    sections.append(OUTPUT_CONTRACT)
    return "\n\n".join(sections)


# ---------------------------------------------------------------------------
# model


def call_model(prompt: str, model: str, timeout: int) -> tuple[str | None, int, str]:
    """Return (text, duration_ms, error). `text` is None when the call failed.

    The CLI is invoked with no tools and from an *empty* working directory, so
    the generating model cannot read the corpus it is being measured against:
    the only RSScript it sees is what this script puts in the prompt.
    """
    command = [
        "claude",
        "-p",
        prompt,
        "--model",
        model,
        "--output-format",
        "text",
        "--allowedTools",
        "",
    ]
    started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix="rss-eval-") as sandbox:
            completed = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=timeout,
                cwd=sandbox,
                check=False,
            )
    except FileNotFoundError:
        return None, 0, "the `claude` CLI is not on PATH"
    except subprocess.TimeoutExpired:
        elapsed = int((time.monotonic() - started) * 1000)
        return None, elapsed, f"timed out after {timeout}s"
    elapsed = int((time.monotonic() - started) * 1000)
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip()[:800]
        return None, elapsed, f"exit {completed.returncode}: {detail}"
    return completed.stdout, elapsed, ""


def extract_code(reply: str) -> str | None:
    blocks = CODE_FENCE.findall(reply or "")
    if blocks:
        # The program is the longest fenced block; a model occasionally emits a
        # tiny illustrative block first.
        return max(blocks, key=len).strip() + "\n"
    stripped = (reply or "").strip()
    # Accept an unfenced reply only when it plainly looks like a program.
    if stripped.startswith(("fn ", "struct ", "sum ", "class ", "protocol ", "resource ", "async fn ", "//")):
        return stripped + "\n"
    return None


# ---------------------------------------------------------------------------
# checking


CHECK_LOCK = threading.Lock()


def run_check(source_path: Path, task: dict) -> tuple[bool, list[dict]]:
    command = [
        "cargo",
        "run",
        "-q",
        "-p",
        "rsscript-cli",
        "--bin",
        "rss",
        "--",
        "check",
        "--json",
        str(source_path),
    ]
    for path in interface_paths(task):
        command += ["--interface", str(path)]
    # `cargo run` takes a workspace lock; serialise so parallel sampling does
    # not turn into a queue of blocked builds.
    with CHECK_LOCK:
        completed = subprocess.run(
            command, capture_output=True, text=True, cwd=ROOT, check=False
        )
    text = completed.stdout.strip()
    if not text:
        return completed.returncode == 0, []
    try:
        diagnostics = json.loads(text)
    except json.JSONDecodeError:
        return completed.returncode == 0, []
    if not isinstance(diagnostics, list):
        return completed.returncode == 0, []
    errors = [d for d in diagnostics if d.get("severity") == "error"]
    return not errors, diagnostics


def compact_diagnostics(diagnostics: list[dict]) -> str:
    rows = []
    for diagnostic in diagnostics:
        if diagnostic.get("severity") != "error":
            continue
        rows.append(
            {
                "code": diagnostic.get("code"),
                "severity": diagnostic.get("severity"),
                "summary": diagnostic.get("summary"),
                "line": (diagnostic.get("primary_span") or {}).get("line"),
                "label": (diagnostic.get("primary_span") or {}).get("label"),
                "causes": diagnostic.get("causes"),
                "fixes": [f.get("title") for f in diagnostic.get("fixes") or []],
            }
        )
    return json.dumps(rows, indent=2)


# ---------------------------------------------------------------------------
# sampling


def sample_task(task: dict, mode: str, args, out_dir: Path) -> dict:
    task_id = task["id"]
    task_dir = out_dir / task_id
    source_path = task_dir / "candidate.rss"
    sidecar_path = out_dir / f"{task_id}.json"
    log_path = task_dir / "transcript.json"

    if source_path.is_file() and not args.force:
        return {"task_id": task_id, "status": "skipped"}

    task_dir.mkdir(parents=True, exist_ok=True)
    prompt = build_prompt(task, mode)
    transcript: list[dict] = []
    total_ms = 0
    turns = 0

    text, duration_ms, error = call_model(prompt, args.model, args.timeout)
    total_ms += duration_ms
    if text is None:
        transcript.append({"turn": 1, "error": error, "duration_ms": duration_ms})
        log_path.write_text(json.dumps(transcript, indent=2) + "\n")
        return {"task_id": task_id, "status": "model_error", "error": error}

    code = extract_code(text)
    if code is None:
        transcript.append(
            {"turn": 1, "error": "no code block in reply", "reply": text[:2000]}
        )
        log_path.write_text(json.dumps(transcript, indent=2) + "\n")
        return {"task_id": task_id, "status": "no_code_block"}

    source_path.write_text(code)
    ok, diagnostics = run_check(source_path, task)
    transcript.append(
        {
            "turn": 1,
            "duration_ms": duration_ms,
            "check_ok": ok,
            "codes": sorted({d["code"] for d in diagnostics if d.get("severity") == "error"}),
        }
    )

    if mode == "repair_loop":
        while not ok and turns + 1 < args.max_turns:
            turns += 1
            repair_prompt = (
                "The RSScript program you produced does not compile. Here is "
                "`rss check --json` output for it, which is the authoritative "
                "compiler feedback:\n\n"
                f"```json\n{compact_diagnostics(diagnostics)}\n```\n\n"
                "Here is the program you produced:\n\n"
                f"```rsscript\n{source_path.read_text().strip()}\n```\n\n"
                f"Original task: {task['prompt'].strip()}\n\n"
                + (interface_block(task) + "\n\n" if interface_block(task) else "")
                + "Fix every reported error and "
                + OUTPUT_CONTRACT
            )
            text, duration_ms, error = call_model(
                repair_prompt, args.model, args.timeout
            )
            total_ms += duration_ms
            if text is None:
                transcript.append(
                    {"turn": turns + 1, "error": error, "duration_ms": duration_ms}
                )
                break
            code = extract_code(text)
            if code is None:
                transcript.append(
                    {"turn": turns + 1, "error": "no code block in reply"}
                )
                break
            source_path.write_text(code)
            ok, diagnostics = run_check(source_path, task)
            transcript.append(
                {
                    "turn": turns + 1,
                    "duration_ms": duration_ms,
                    "check_ok": ok,
                    "codes": sorted(
                        {
                            d["code"]
                            for d in diagnostics
                            if d.get("severity") == "error"
                        }
                    ),
                }
            )

    sidecar = {
        "schema": "rsscript.eval.candidate.v1",
        "task_id": task_id,
        "source": f"{task_id}/candidate.rss",
        "mode": mode,
        "model": args.model,
        "model_version": args.model_version,
        "temperature": None,
        "attempt": 1,
        "generation_duration_ms": total_ms,
        "repair_turns": turns,
    }
    sidecar_path.write_text(json.dumps(sidecar, indent=2) + "\n")
    log_path.write_text(json.dumps(transcript, indent=2) + "\n")
    return {
        "task_id": task_id,
        "status": "ok" if ok else "check_failed",
        "turns": turns,
        "duration_ms": total_ms,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="sonnet")
    parser.add_argument("--model-version", default="unknown")
    parser.add_argument(
        "--mode", action="append", choices=MODES, help="repeatable; default: all"
    )
    parser.add_argument("--task", action="append", help="repeatable task id filter")
    parser.add_argument("--limit", type=int, default=0, help="max tasks per mode")
    parser.add_argument("--timeout", type=int, default=120, help="seconds per call")
    parser.add_argument("--max-turns", type=int, default=3, help="repair_loop turns")
    parser.add_argument("--force", action="store_true")
    parser.add_argument(
        "--jobs", type=int, default=1, help="concurrent model calls per mode"
    )
    parser.add_argument(
        "--out",
        default=None,
        help="sample root (default: evals/samples/<model>)",
    )
    parser.add_argument(
        "--dump-prompt",
        metavar="TASK_ID",
        help="print the built prompt for one task and exit (no model call)",
    )
    args = parser.parse_args()

    if args.dump_prompt:
        tasks = {task["id"]: task for task in load_tasks()}
        task = tasks.get(args.dump_prompt)
        if task is None:
            print(f"error: unknown task `{args.dump_prompt}`", file=sys.stderr)
            return 2
        mode = (args.mode or ["prompt_only"])[0]
        sys.stdout.write(build_prompt(task, mode))
        return 0

    if shutil.which("claude") is None:
        print("error: the `claude` CLI is not on PATH", file=sys.stderr)
        return 2

    modes = args.mode or list(MODES)
    tasks = load_tasks()
    if args.task:
        wanted = set(args.task)
        tasks = [task for task in tasks if task["id"] in wanted]
    if args.limit:
        tasks = tasks[: args.limit]
    if not tasks:
        print("error: no tasks selected", file=sys.stderr)
        return 2

    root = Path(args.out) if args.out else EVALS / "samples" / args.model
    failures = 0
    for mode in modes:
        out_dir = root / mode
        out_dir.mkdir(parents=True, exist_ok=True)
        with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
            results = list(
                pool.map(lambda task: sample_task(task, mode, args, out_dir), tasks)
            )
        for result in results:
            status = result["status"]
            if status in ("model_error", "no_code_block"):
                failures += 1
            extra = ""
            if "turns" in result:
                extra = f" turns={result['turns']} ms={result['duration_ms']}"
            if result.get("error"):
                extra = f" {result['error']}"
            print(f"[{mode}] {result['task_id']}: {status}{extra}", flush=True)
            sys.stdout.flush()

    if failures:
        print(f"{failures} task(s) produced no candidate", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
