from typing import Literal, TypedDict

RouteKind = Literal["project_dev", "house", "general", "dfd", "orchestrator", "design"]

class RouteDecision(TypedDict, total=False):
    kind: RouteKind
    project: dict | None
    reason: str
