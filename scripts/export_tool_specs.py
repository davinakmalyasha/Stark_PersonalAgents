from __future__ import annotations

import argparse
import difflib
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "backend-rs" / "specs" / "tool-specs.json"
CONTRACT_VERSION = 1

if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))


def load_schemas() -> ModuleType:
    path = ROOT / "tools" / "_schemas.py"
    spec = importlib.util.spec_from_file_location("_stark_tool_schemas", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def build_contract(module: ModuleType) -> dict:
    tools: list[dict] = []
    for definition in module.TOOL_DEFINITIONS:
        name = str(definition["name"])
        tools.append(
            {
                "name": name,
                "description": definition.get("description", ""),
                "parameters": definition.get("parameters")
                or {"type": "object", "properties": {}, "required": []},
                "exposed": definition.get("exposed") is not False,
                "required": sorted(module._TOOL_REQUIRED_ARGS.get(name, set())),
                "doc": module._TOOL_DOCS.get(name),
            }
        )

    names = {tool["name"] for tool in tools}
    missing_docs = [tool["name"] for tool in tools if tool["doc"] is None]
    orphan_docs = sorted(name for name in module._TOOL_DOCS if name not in names)
    scope_map = {
        str(scope): list(tool_names)
        for scope, tool_names in module._SCOPE_MAP.items()
    }

    return {
        "contract_version": CONTRACT_VERSION,
        "source": "tools/_schemas.py",
        "tool_count": len(tools),
        "tools": tools,
        "scope_map": scope_map,
        "missing_docs": missing_docs,
        "orphan_docs": orphan_docs,
    }


def render(contract: dict) -> str:
    return json.dumps(contract, ensure_ascii=False, indent=2) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Export the frozen Stark tool contract from tools/_schemas.py."
    )
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail if the checked-in contract differs; never write.",
    )
    args = parser.parse_args()

    contract = build_contract(load_schemas())
    text = render(contract)
    out: Path = args.out

    if args.check:
        if not out.exists():
            print(f"missing contract: {out}", file=sys.stderr)
            return 1
        existing = out.read_text(encoding="utf-8")
        if existing != text:
            diff = difflib.unified_diff(
                existing.splitlines(keepends=True),
                text.splitlines(keepends=True),
                fromfile=str(out),
                tofile="generated",
            )
            sys.stderr.writelines(diff)
            print(
                f"stale contract: {out}; run scripts/export_tool_specs.py",
                file=sys.stderr,
            )
            return 1
        print(
            f"OK: {out} ({contract['tool_count']} tools, "
            f"{len(contract['scope_map'])} scopes)"
        )
        return 0

    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    print(
        f"Wrote {out} ({contract['tool_count']} tools, "
        f"{len(contract['scope_map'])} scopes)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
