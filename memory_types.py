from typing import Any
from pydantic import BaseModel, Field, field_validator

class MemorySegmentUpdate(BaseModel):
    """Structured update schema for a specific memory division."""
    updates: dict[str, Any] = Field(default_factory=dict, description="Key-value pairs to add or update in this division's memory database.")
    deletions: list[str] = Field(default_factory=list, description="Keys to delete/remove from this division's database to resolve conflicts.")

    @field_validator("updates", mode="before")
    @classmethod
    def coerce_updates(cls, v: Any) -> Any:
        if isinstance(v, list):
            coerced = {}
            for item in v:
                if isinstance(item, dict):
                    if "key" in item and "value" in item:
                        coerced[str(item["key"])] = item["value"]
                    elif "name" in item and "value" in item:
                        coerced[str(item["name"])] = item["value"]
                    else:
                        for k, val in item.items():
                            coerced[str(k)] = val
                elif isinstance(item, str):
                    coerced[item] = True
            return coerced
        return v

class SupervisorMemoryUpdate(BaseModel):
    """Structured schema for memory refinement updates across all divisions (RBAC)."""
    global_segment: MemorySegmentUpdate = Field(description="Updates for global user preferences, profile, and name.")
    it_segment: MemorySegmentUpdate = Field(description="Updates for IT infrastructure, databases, servers, frameworks, and file structure.")
    design_segment: MemorySegmentUpdate = Field(description="Updates for BA specs, color themes, UX flows, and client design rules.")
    security_segment: MemorySegmentUpdate = Field(description="Updates for security requirements, permissions, and safe configurations.")
    lessons_learned_updates: list[str] = Field(default_factory=list, description="A list of new workspace-level lessons learned, rules of thumb, or pitfalls to avoid based on errors, fixes, or feedback in this run.")
    global_lessons_learned_updates: list[str] = Field(default_factory=list, description="A list of new global-level lessons learned, rules of thumb, or pitfalls to avoid that are high-impact and universal (rare).")
    new_request_to_add: str = Field("", description="The client request to add to the history log, summarized in a short sentence.")

