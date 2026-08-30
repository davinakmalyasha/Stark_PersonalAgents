"""
Stage 1: Pydantic input schemas for tool arguments.

Opt-in. A tool that wants strict input validation attaches its schema via
`ToolDefinition.input_schema`. Tools without a schema behave identically
to today.

This module is the *only* place we declare input schemas. When Stage 1 is
complete and we want to expand coverage, add a class here and a single
line in `tools/_registry.py`.
"""

from __future__ import annotations

from typing import Optional

from pydantic import BaseModel, Field, field_validator


class ReadFileInput(BaseModel):
    """Strict input contract for `read_file`.

    The model already accepts many of these, but until now the registry
    did not validate the model's tool-call arguments — only the
    `pre_validate_tool_args` post-hoc type check. This schema runs at
    the registry boundary and returns `[INPUT_INVALID] ...` on failure.
    """

    file_path: str = Field(..., min_length=1, description="Absolute or workspace-relative file path.")
    offset: Optional[int] = Field(None, ge=0, description="Line offset to start reading at.")
    limit: Optional[int] = Field(None, ge=1, le=10000, description="Max lines to return.")

    @field_validator("file_path")
    @classmethod
    def _no_null_bytes(cls, v: str) -> str:
        if "\x00" in v:
            raise ValueError("file_path must not contain null bytes")
        return v


# A tiny smoke-test schema for Stage 1 verification.
class _PingInput(BaseModel):
    name: str = Field(..., min_length=1, max_length=64)


# Registry of {tool_name: schema} attached at startup.
TOOL_INPUT_SCHEMAS: dict[str, type] = {
    "read_file": ReadFileInput,
}
